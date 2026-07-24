use std::{fs, path::PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::AppHandle;

use super::{
    DaemonBinding, DaemonBindingSnapshot, DaemonHistoryEntry, DaemonRunLifecycle, DaemonRunRecord,
    DaemonRunStatus, DaemonRunTrigger, LegacyDaemonRunRecord,
};
use crate::managed_agents::{atomic_write_json_restricted, managed_agents_base_dir};

const MAX_HISTORY_TERMINAL_RECORDS: usize = 200;
const MAX_HISTORY_TERMINAL_BYTES: u64 = 4 * 1024 * 1024;
const MAX_HISTORY_TERMINAL_AGE_DAYS: i64 = 90;
const MAX_DIAGNOSTIC_CHARS: usize = 512;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BindingStore {
    #[serde(default)]
    pub bindings: Vec<DaemonBinding>,
}

impl BindingStore {
    pub fn load(app: &AppHandle) -> Result<Self, String> {
        let path = binding_store_path(app)?;
        match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| format!("failed to parse daemon binding store: {error}")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(format!("failed to read daemon binding store: {error}")),
        }
    }

    pub fn save(&self, app: &AppHandle) -> Result<(), String> {
        let payload = serde_json::to_vec_pretty(self).map_err(|error| error.to_string())?;
        atomic_write_json_restricted(&binding_store_path(app)?, &payload)
    }
}

pub(crate) fn get_binding_internal(
    app: &AppHandle,
    binding_id: &str,
) -> Result<DaemonBinding, String> {
    BindingStore::load(app)?
        .bindings
        .into_iter()
        .find(|binding| binding.id == binding_id)
        .ok_or_else(|| "daemon binding not found".into())
}

#[derive(Debug)]
pub(crate) enum Reservation {
    New(DaemonRunRecord),
    ExistingTerminal(DaemonRunRecord),
    ExistingPublishing(DaemonRunRecord),
}

pub(crate) fn reserve_run(
    app: &AppHandle,
    run_id: &str,
    binding: &DaemonBinding,
    package_hash: &str,
    wake_instruction: &str,
    trigger: DaemonRunTrigger,
    scheduled_for_utc: Option<&str>,
) -> Result<Reservation, String> {
    let path = run_path(app, run_id)?;
    if read_run_path(&path)?.is_none() && binding_has_active_run(app, &binding.id)? {
        return Err("daemon binding already has an active run".into());
    }
    reserve_run_at_path(
        &path,
        run_id,
        binding,
        package_hash,
        wake_instruction,
        trigger,
        scheduled_for_utc,
    )
}

pub(super) fn reserve_run_at_path(
    path: &std::path::Path,
    run_id: &str,
    binding: &DaemonBinding,
    package_hash: &str,
    wake_instruction: &str,
    trigger: DaemonRunTrigger,
    scheduled_for_utc: Option<&str>,
) -> Result<Reservation, String> {
    let wake_hash = sha256(wake_instruction.as_bytes());
    let snapshot = DaemonBindingSnapshot::from(binding);
    let fingerprint = immutable_fingerprint(
        &snapshot,
        package_hash,
        &wake_hash,
        &trigger,
        scheduled_for_utc,
    )?;
    if let Some(existing) = read_run_path(path)? {
        if existing.fingerprint != fingerprint {
            return Err("run UUID conflicts with a different immutable daemon request".into());
        }
        if existing.lifecycle == DaemonRunLifecycle::Terminal {
            return Ok(Reservation::ExistingTerminal(existing));
        }
        if existing.lifecycle == DaemonRunLifecycle::Publishing {
            return Ok(Reservation::ExistingPublishing(existing));
        }
        return Err("matching daemon run is already reserved or in progress".into());
    }
    let record = DaemonRunRecord {
        run_id: run_id.to_string(),
        binding_id: binding.id.clone(),
        daemon_id: binding.daemon_id.clone(),
        package_hash: Some(package_hash.to_string()),
        policy_hash: Some(package_hash.to_string()),
        fingerprint,
        wake_hash,
        binding: snapshot,
        trigger,
        scheduled_for_utc: scheduled_for_utc.map(str::to_string),
        lifecycle: DaemonRunLifecycle::Reserved,
        status: None,
        reserved_at: crate::util::now_iso(),
        started_at: None,
        publishing_at: None,
        completed_at: None,
        acp_session_id: None,
        output_event_id: None,
        diagnostic: None,
    };
    write_run_path(path, &record)?;
    Ok(Reservation::New(record))
}

pub(crate) fn record_scheduler_receipt(
    app: &AppHandle,
    run_id: &str,
    binding: &DaemonBinding,
    package_hash: Option<&str>,
    scheduled_for_utc: &str,
    status: DaemonRunStatus,
    diagnostic: &str,
) -> Result<DaemonRunRecord, String> {
    let package_hash = package_hash.unwrap_or("unavailable");
    let wake = format!("Scheduled daemon occurrence at {scheduled_for_utc}");
    let wake_hash = sha256(wake.as_bytes());
    let snapshot = DaemonBindingSnapshot::from(binding);
    let trigger = DaemonRunTrigger::Schedule;
    let fingerprint = immutable_fingerprint(
        &snapshot,
        package_hash,
        &wake_hash,
        &trigger,
        Some(scheduled_for_utc),
    )?;
    let path = run_path(app, run_id)?;
    if let Some(existing) = read_run_path(&path)? {
        if existing.fingerprint == fingerprint {
            return Ok(existing);
        }
        return Err("scheduler receipt UUID conflicts with another occurrence".into());
    }
    let now = crate::util::now_iso();
    let record = DaemonRunRecord {
        run_id: run_id.to_string(),
        binding_id: binding.id.clone(),
        daemon_id: binding.daemon_id.clone(),
        package_hash: Some(package_hash.to_string()),
        policy_hash: Some(package_hash.to_string()),
        fingerprint,
        wake_hash,
        binding: snapshot,
        trigger,
        scheduled_for_utc: Some(scheduled_for_utc.to_string()),
        lifecycle: DaemonRunLifecycle::Terminal,
        status: Some(status),
        reserved_at: now.clone(),
        started_at: None,
        publishing_at: None,
        completed_at: Some(now),
        acp_session_id: None,
        output_event_id: None,
        diagnostic: Some(safe_diagnostic(diagnostic)),
    };
    write_run_path(&path, &record)?;
    prune_history(app)?;
    Ok(record)
}

pub(crate) fn binding_has_active_run(app: &AppHandle, binding_id: &str) -> Result<bool, String> {
    for path in history_files(app)? {
        let Some(record) = read_run_path(&path)? else {
            continue;
        };
        if record.binding_id == binding_id && record.lifecycle != DaemonRunLifecycle::Terminal {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn mark_run_running(
    app: &AppHandle,
    record: &mut DaemonRunRecord,
) -> Result<(), String> {
    require_lifecycle(record, DaemonRunLifecycle::Reserved)?;
    record.lifecycle = DaemonRunLifecycle::Running;
    record.started_at = Some(crate::util::now_iso());
    write_run(app, record)
}

pub(crate) fn mark_run_publishing(
    app: &AppHandle,
    record: &mut DaemonRunRecord,
    acp_session_id: Option<String>,
) -> Result<(), String> {
    require_lifecycle(record, DaemonRunLifecycle::Running)?;
    record.lifecycle = DaemonRunLifecycle::Publishing;
    record.publishing_at = Some(crate::util::now_iso());
    record.acp_session_id = acp_session_id;
    record.diagnostic = Some("publication pending marker reconciliation".into());
    write_run(app, record)
}

pub(crate) fn finalize_run(
    app: &AppHandle,
    record: &mut DaemonRunRecord,
    output_event_id: String,
) -> Result<(), String> {
    require_lifecycle(record, DaemonRunLifecycle::Publishing)?;
    record.lifecycle = DaemonRunLifecycle::Terminal;
    record.status = Some(DaemonRunStatus::Succeeded);
    record.output_event_id = Some(output_event_id);
    record.completed_at = Some(crate::util::now_iso());
    record.diagnostic = None;
    write_run(app, record)?;
    prune_history(app)
}

pub(crate) fn terminalize_run(
    app: &AppHandle,
    record: &mut DaemonRunRecord,
    status: DaemonRunStatus,
    acp_session_id: Option<String>,
    diagnostic: Option<&str>,
) -> Result<(), String> {
    if record.lifecycle == DaemonRunLifecycle::Terminal {
        return Err("terminal daemon run receipts are immutable".into());
    }
    if record.lifecycle == DaemonRunLifecycle::Publishing {
        return Err(
            "publishing daemon runs require marker reconciliation before terminalization".into(),
        );
    }
    record.lifecycle = DaemonRunLifecycle::Terminal;
    record.status = Some(status);
    record.acp_session_id = acp_session_id;
    record.completed_at = Some(crate::util::now_iso());
    record.diagnostic = diagnostic.map(safe_diagnostic);
    write_run(app, record)?;
    prune_history(app)
}

pub(crate) fn recover_interrupted_runs(app: &AppHandle) -> Result<(), String> {
    for path in history_files(app)? {
        let Some(mut record) = read_run_path(&path)? else {
            continue;
        };
        if matches!(
            record.lifecycle,
            DaemonRunLifecycle::Reserved | DaemonRunLifecycle::Running
        ) {
            record.lifecycle = DaemonRunLifecycle::Terminal;
            record.status = Some(DaemonRunStatus::Interrupted);
            record.completed_at = Some(crate::util::now_iso());
            record.diagnostic = Some("daemon run was interrupted before completion".into());
            write_run(app, &record)?;
        }
    }
    prune_history(app)
}

pub(crate) fn list_history(app: &AppHandle) -> Result<Vec<DaemonHistoryEntry>, String> {
    recover_interrupted_runs(app)?;
    let mut entries = Vec::new();
    for path in history_files(app)? {
        if let Some(record) = read_run_path(&path)? {
            entries.push(DaemonHistoryEntry::Managed(Box::new(record)));
        }
    }
    let legacy_dir = managed_agents_base_dir(app)?.join("daemon-runs");
    if legacy_dir.is_dir() {
        for entry in fs::read_dir(legacy_dir).map_err(|error| error.to_string())? {
            let path = entry.map_err(|error| error.to_string())?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let value: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).map_err(|error| error.to_string())?)
                    .map_err(|error| format!("failed to parse legacy daemon receipt: {error}"))?;
            let status = match value.get("status").and_then(|value| value.as_str()) {
                Some("succeeded") => DaemonRunStatus::Succeeded,
                Some("cancelled") => DaemonRunStatus::Cancelled,
                _ => DaemonRunStatus::Failed,
            };
            entries.push(DaemonHistoryEntry::Legacy(LegacyDaemonRunRecord {
                run_id: string_field(&value, "runId")?,
                daemon_id: string_field(&value, "daemonId")?,
                status,
                started_at: string_field(&value, "startedAt")?,
                completed_at: string_field(&value, "completedAt")?,
                acp_session_id: optional_string_field(&value, "acpSessionId"),
                output_event_id: optional_string_field(&value, "outputEventId"),
                diagnostic: optional_string_field(&value, "diagnostic")
                    .map(|v| safe_diagnostic(&v)),
            }));
        }
    }
    entries.sort_by(|left, right| history_timestamp(right).cmp(history_timestamp(left)));
    Ok(entries)
}

pub(crate) fn reconcile_publishing_success(
    app: &AppHandle,
    run_id: &str,
    event_id: String,
) -> Result<DaemonRunRecord, String> {
    let mut record = load_run(app, run_id)?.ok_or("daemon run history not found")?;
    if record.lifecycle == DaemonRunLifecycle::Terminal {
        if record.output_event_id.as_deref() == Some(&event_id) {
            return Ok(record);
        }
        return Err("terminal daemon run receipt is immutable".into());
    }
    finalize_run(app, &mut record, event_id)?;
    Ok(record)
}

fn binding_store_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(managed_agents_base_dir(app)?.join("daemon-bindings.json"))
}

fn history_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = managed_agents_base_dir(app)?.join("daemon-history");
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir)
}

fn run_path(app: &AppHandle, run_id: &str) -> Result<PathBuf, String> {
    Ok(history_dir(app)?.join(format!("{run_id}.json")))
}

fn write_run(app: &AppHandle, record: &DaemonRunRecord) -> Result<(), String> {
    let path = run_path(app, &record.run_id)?;
    write_run_path(&path, record)
}

pub(super) fn write_run_path(
    path: &std::path::Path,
    record: &DaemonRunRecord,
) -> Result<(), String> {
    if let Some(existing) = read_run_path(path)? {
        if existing.lifecycle == DaemonRunLifecycle::Terminal && existing != *record {
            return Err("terminal daemon run receipts are immutable".into());
        }
        if existing.fingerprint != record.fingerprint {
            return Err("daemon run immutable fingerprint changed".into());
        }
    }
    let payload = serde_json::to_vec_pretty(record).map_err(|error| error.to_string())?;
    atomic_write_json_restricted(path, &payload)
}

fn load_run(app: &AppHandle, run_id: &str) -> Result<Option<DaemonRunRecord>, String> {
    read_run_path(&run_path(app, run_id)?)
}

pub(super) fn read_run_path(path: &std::path::Path) -> Result<Option<DaemonRunRecord>, String> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("failed to parse daemon run history: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn history_files(app: &AppHandle) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(history_dir(app)?).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.extension().and_then(|value| value.to_str()) == Some("json") {
            paths.push(path);
        }
    }
    Ok(paths)
}

fn prune_history(app: &AppHandle) -> Result<(), String> {
    let cutoff = Utc::now() - Duration::days(MAX_HISTORY_TERMINAL_AGE_DAYS);
    let mut terminal = Vec::new();
    let mut total_bytes = 0_u64;
    for path in history_files(app)? {
        let Some(record) = read_run_path(&path)? else {
            continue;
        };
        if record.lifecycle != DaemonRunLifecycle::Terminal {
            continue;
        }
        let bytes = fs::metadata(&path)
            .map_err(|error| error.to_string())?
            .len();
        total_bytes = total_bytes.saturating_add(bytes);
        let completed = record
            .completed_at
            .as_deref()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc));
        terminal.push((path, completed, bytes));
    }
    terminal.sort_by_key(|(_, completed, _)| *completed);
    while terminal.len() > MAX_HISTORY_TERMINAL_RECORDS || total_bytes > MAX_HISTORY_TERMINAL_BYTES
    {
        let (path, _, bytes) = terminal.remove(0);
        fs::remove_file(path).map_err(|error| error.to_string())?;
        total_bytes = total_bytes.saturating_sub(bytes);
    }
    for (path, completed, _) in terminal {
        if completed.is_some_and(|value| value < cutoff) {
            fs::remove_file(path).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn immutable_fingerprint(
    binding: &DaemonBindingSnapshot,
    package_hash: &str,
    wake_hash: &str,
    trigger: &DaemonRunTrigger,
    scheduled_for_utc: Option<&str>,
) -> Result<String, String> {
    let payload =
        serde_json::to_vec(&(binding, package_hash, wake_hash, trigger, scheduled_for_utc))
            .map_err(|error| error.to_string())?;
    Ok(sha256(&payload))
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn require_lifecycle(record: &DaemonRunRecord, expected: DaemonRunLifecycle) -> Result<(), String> {
    if record.lifecycle != expected {
        return Err(format!(
            "invalid daemon run lifecycle transition from {:?}; expected {:?}",
            record.lifecycle, expected
        ));
    }
    Ok(())
}

fn safe_diagnostic(value: &str) -> String {
    value.chars().take(MAX_DIAGNOSTIC_CHARS).collect()
}

fn string_field(value: &serde_json::Value, key: &str) -> Result<String, String> {
    value
        .get(key)
        .and_then(|field| field.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("legacy daemon receipt is missing {key}"))
}

fn optional_string_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|field| field.as_str())
        .map(str::to_string)
}

fn history_timestamp(entry: &DaemonHistoryEntry) -> &str {
    match entry {
        DaemonHistoryEntry::Managed(record) => record
            .completed_at
            .as_deref()
            .or(record.publishing_at.as_deref())
            .or(record.started_at.as_deref())
            .unwrap_or(&record.reserved_at),
        DaemonHistoryEntry::Legacy(record) => &record.completed_at,
    }
}
