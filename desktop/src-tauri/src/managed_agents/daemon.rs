//! Phase-0 daemon package loading, direct managed-runtime execution, and terminal records.

use std::{path::{Path, PathBuf}, sync::atomic::Ordering, time::Duration};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    agent_readiness, atomic_write_json_restricted, known_acp_runtime, load_global_agent_config,
    load_managed_agents, load_personas, managed_agents_base_dir, record_agent_command,
    resolve_effective_agent_env, start_managed_agent_runtime_pair_lazy, AgentReadiness,
    BackendKind, ManagedAgentRuntimeKey,
};
use crate::{app_state::AppState, commands::send_managed_agent_channel_message_inner};

const MAX_DAEMON_FILE_BYTES: u64 = 256 * 1024;
const MAX_WAKE_BYTES: usize = 32 * 1024;
const MAX_CONTEXT_BYTES: usize = 64 * 1024;
const MAX_RENDERED_WAKE_BYTES: usize = MAX_WAKE_BYTES + MAX_CONTEXT_BYTES;
const MAX_DIAGNOSTIC_CHARS: usize = 512;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunManagedDaemonRequest {
    pub run_id: String,
    pub daemon_id: String,
    pub package_root: String,
    pub agent_pubkey: String,
    pub channel_id: String,
    pub relay_url: String,
    pub wake_instruction: String,
    pub cwd: Option<String>,
    pub idle_timeout_seconds: Option<u64>,
    pub hard_timeout_seconds: Option<u64>,
    pub context_snapshot: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonRunStatus {
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonRunRecord {
    pub run_id: String,
    pub daemon_id: String,
    pub acp_session_id: Option<String>,
    pub status: DaemonRunStatus,
    pub started_at: String,
    pub completed_at: String,
    pub agent_pubkey: String,
    pub channel_id: String,
    pub relay_url: String,
    pub cwd: String,
    pub context_snapshot: Option<serde_json::Value>,
    pub diagnostic: Option<String>,
    pub output_event_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ControlReady {
    pid: u32,
    base_url: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ControlRunRequest<'a> {
    run_id: Uuid,
    policy_markdown: &'a str,
    wake_instruction: &'a str,
    cwd: &'a str,
    idle_timeout_seconds: u64,
    hard_timeout_seconds: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ControlRunResponse {
    run_id: Uuid,
    session_id: Option<String>,
    status: String,
    output_markdown: Option<String>,
    diagnostic: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DaemonFrontmatter {
    id: String,
    purpose: String,
    #[serde(default)]
    watch: Vec<String>,
    routines: Vec<String>,
    #[serde(default)]
    deny: Vec<String>,
    schedule: Option<String>,
}

#[tauri::command]
pub async fn run_managed_daemon(
    request: RunManagedDaemonRequest,
    app: AppHandle,
) -> Result<DaemonRunRecord, String> {
    let state = app.state::<AppState>();
    if state.shutdown_started.load(Ordering::Acquire) {
        return Err("desktop shutdown has started".into());
    }
    let run_id = Uuid::parse_str(request.run_id.trim()).map_err(|_| "invalid run UUID")?;
    let cancellation = CancellationToken::new();
    {
        let mut active = state
            .daemon_run_cancellations
            .lock()
            .map_err(|error| error.to_string())?;
        if active.contains_key(&run_id) {
            return Err("run UUID is already active".into());
        }
        active.insert(run_id, cancellation.clone());
    }
    let result = run_managed_daemon_inner(request, run_id, app.clone(), cancellation).await;
    state
        .daemon_run_cancellations
        .lock()
        .map_err(|error| error.to_string())?
        .remove(&run_id);
    result
}

async fn run_managed_daemon_inner(
    request: RunManagedDaemonRequest,
    run_id: Uuid,
    app: AppHandle,
    cancellation: CancellationToken,
) -> Result<DaemonRunRecord, String> {
    let state = app.state::<AppState>();
    if request.wake_instruction.trim().is_empty() || request.wake_instruction.len() > MAX_WAKE_BYTES {
        return Err("wake instruction is empty or too large".into());
    }
    if let Some(context) = &request.context_snapshot {
        if serde_json::to_vec(context).map_err(|error| error.to_string())?.len() > MAX_CONTEXT_BYTES {
            return Err("context snapshot exceeds 64 KiB".into());
        }
    }
    let rendered_wake = render_wake_instruction(
        request.wake_instruction.trim(),
        request.context_snapshot.as_ref(),
    )?;
    let policy = load_daemon_policy(Path::new(&request.package_root), &request.daemon_id)?;
    let cwd = canonical_cwd(request.cwd.as_deref().unwrap_or(&request.package_root))?;
    let idle = request.idle_timeout_seconds.unwrap_or(300);
    let hard = request.hard_timeout_seconds.unwrap_or(1800);
    if idle == 0 || idle >= hard || hard > 86_400 {
        return Err("timeouts must be positive, idle < hard, and hard <= 24 hours".into());
    }

    resolve_ready_agent(&app, &request.agent_pubkey)?;
    let key = ManagedAgentRuntimeKey::new(request.agent_pubkey.clone(), &request.relay_url)?;
    let _ = start_managed_agent_runtime_pair_lazy(
        key.pubkey.clone(),
        key.relay_url.clone(),
        app.clone(),
    )?;
    let (base_url, bearer) = match wait_for_control(&app, &key, &cancellation).await {
        Ok(control) => control,
        Err(error) if cancellation.is_cancelled() => {
            return terminal_startup_cancellation(&app, request, run_id, cwd, key, error);
        }
        Err(error) => return Err(error),
    };

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(hard.saturating_add(15)))
        .build()
        .map_err(|error| error.to_string())?;
    let started_at = crate::util::now_iso();
    let body = ControlRunRequest {
        run_id,
        policy_markdown: &policy,
        wake_instruction: &rendered_wake,
        cwd: cwd.to_str().ok_or("cwd is not valid UTF-8")?,
        idle_timeout_seconds: idle,
        hard_timeout_seconds: hard,
    };
    let run_future = client
        .post(format!("{base_url}/v1/runs"))
        .bearer_auth(&bearer)
        .json(&body)
        .send();
    tokio::pin!(run_future);
    let control_result = tokio::select! {
        response = &mut run_future => parse_control_response(response).await,
        _ = cancellation.cancelled() => {
            let _ = client
                .post(format!("{base_url}/v1/runs/{run_id}/cancel"))
                .bearer_auth(&bearer)
                .timeout(Duration::from_secs(2))
                .send()
                .await;
            match tokio::time::timeout(Duration::from_secs(10), &mut run_future).await {
                Ok(response) => parse_control_response(response).await,
                Err(_) => Ok(ControlRunResponse {
                    run_id,
                    session_id: None,
                    status: "cancelled".into(),
                    output_markdown: None,
                    diagnostic: Some("cancellation acknowledgement timed out".into()),
                }),
            }
        }
    };
    let (status, session_id, diagnostic, output) = match control_result {
        Ok(response) if response.run_id == run_id && response.status == "succeeded" => (
            DaemonRunStatus::Succeeded,
            response.session_id,
            response.diagnostic,
            response.output_markdown,
        ),
        Ok(response) if response.status == "cancelled" => (
            DaemonRunStatus::Cancelled,
            response.session_id,
            response.diagnostic,
            None,
        ),
        Ok(response) => (
            DaemonRunStatus::Failed,
            response.session_id,
            response.diagnostic.or_else(|| Some("daemon execution failed".into())),
            None,
        ),
        Err(error) => (DaemonRunStatus::Failed, None, Some(error), None),
    };

    let mut record = DaemonRunRecord {
        run_id: run_id.to_string(),
        daemon_id: request.daemon_id,
        acp_session_id: session_id,
        status,
        started_at,
        completed_at: crate::util::now_iso(),
        agent_pubkey: key.pubkey,
        channel_id: request.channel_id,
        relay_url: key.relay_url,
        cwd: cwd.display().to_string(),
        context_snapshot: request.context_snapshot,
        diagnostic: diagnostic.map(|value| safe_diagnostic(&value)),
        output_event_id: None,
    };
    if record.status == DaemonRunStatus::Succeeded {
        match output {
            Some(output) => {
                match send_managed_agent_channel_message_inner(
                    record.agent_pubkey.clone(),
                    record.channel_id.clone(),
                    output,
                    Some(daemon_run_marker(run_id)),
                    Some("agent".into()),
                    None,
                    None,
                    None,
                    &app,
                    &state,
                )
                .await
                {
                    Ok(response) => record.output_event_id = Some(response.event_id),
                    Err(error) => {
                        record.status = DaemonRunStatus::Failed;
                        record.diagnostic = Some(safe_diagnostic(&format!("publication failed: {error}")));
                    }
                }
            }
            None => {
                record.status = DaemonRunStatus::Failed;
                record.diagnostic = Some("successful daemon response omitted output".into());
            }
        }
    }
    persist_run_record(&app, &record)?;
    Ok(record)
}

fn terminal_startup_cancellation(
    app: &AppHandle,
    request: RunManagedDaemonRequest,
    run_id: Uuid,
    cwd: PathBuf,
    key: ManagedAgentRuntimeKey,
    diagnostic: String,
) -> Result<DaemonRunRecord, String> {
    let now = crate::util::now_iso();
    let record = DaemonRunRecord {
        run_id: run_id.to_string(),
        daemon_id: request.daemon_id,
        acp_session_id: None,
        status: DaemonRunStatus::Cancelled,
        started_at: now.clone(),
        completed_at: now,
        agent_pubkey: key.pubkey,
        channel_id: request.channel_id,
        relay_url: key.relay_url,
        cwd: cwd.display().to_string(),
        context_snapshot: request.context_snapshot,
        diagnostic: Some(safe_diagnostic(&diagnostic)),
        output_event_id: None,
    };
    persist_run_record(app, &record)?;
    Ok(record)
}

#[tauri::command]
pub fn cancel_managed_daemon(run_id: String, app: AppHandle) -> Result<(), String> {
    let run_id = Uuid::parse_str(run_id.trim()).map_err(|_| "invalid run UUID")?;
    let state = app.state::<AppState>();
    let active = state.daemon_run_cancellations.lock().map_err(|error| error.to_string())?;
    let cancellation = active.get(&run_id).ok_or("daemon run is not active")?;
    cancellation.cancel();
    Ok(())
}

pub(crate) fn cancel_all_daemon_runs(app: &AppHandle) {
    if let Ok(active) = app.state::<AppState>().daemon_run_cancellations.lock() {
        for cancellation in active.values() {
            cancellation.cancel();
        }
    }
}

fn load_daemon_policy(root: &Path, daemon_id: &str) -> Result<String, String> {
    if daemon_id.is_empty()
        || !daemon_id.chars().all(|character| character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-')
    {
        return Err("daemon ID must contain only lowercase letters, digits, and hyphens".into());
    }
    let root = std::fs::canonicalize(root).map_err(|error| format!("invalid package root: {error}"))?;
    let path = root.join(".agents").join("daemons").join(daemon_id).join("DAEMON.md");
    let canonical = std::fs::canonicalize(&path).map_err(|error| format!("failed to load {}: {error}", path.display()))?;
    if !canonical.starts_with(&root) {
        return Err("daemon package escapes package root".into());
    }
    let metadata = std::fs::metadata(&canonical).map_err(|error| error.to_string())?;
    if metadata.len() > MAX_DAEMON_FILE_BYTES {
        return Err("DAEMON.md exceeds 256 KiB".into());
    }
    let contents = std::fs::read_to_string(canonical).map_err(|error| error.to_string())?;
    validate_daemon_policy(&contents, daemon_id)?;
    Ok(contents)
}

fn validate_daemon_policy(contents: &str, daemon_id: &str) -> Result<(), String> {
    let normalized = contents.replace("\r\n", "\n");
    let rest = normalized
        .strip_prefix("---\n")
        .ok_or("DAEMON.md must start with YAML frontmatter")?;
    let (frontmatter, body) = rest
        .split_once("\n---\n")
        .ok_or("DAEMON.md frontmatter is missing its closing delimiter")?;
    let metadata: DaemonFrontmatter = serde_yaml::from_str(frontmatter)
        .map_err(|error| format!("invalid DAEMON.md frontmatter: {error}"))?;
    if metadata.id != daemon_id {
        return Err("DAEMON.md ID must match its directory".into());
    }
    if metadata.purpose.trim().is_empty()
        || metadata.routines.is_empty()
        || metadata.routines.iter().any(|value| value.trim().is_empty())
        || metadata.watch.iter().any(|value| value.trim().is_empty())
        || metadata.deny.iter().any(|value| value.trim().is_empty())
        || metadata.schedule.is_some_and(|value| value.trim().is_empty())
        || body.trim().is_empty()
    {
        return Err("DAEMON.md has empty or missing required policy fields".into());
    }
    Ok(())
}

fn canonical_cwd(value: &str) -> Result<PathBuf, String> {
    let path = std::fs::canonicalize(value).map_err(|error| format!("invalid cwd: {error}"))?;
    if !path.is_dir() { return Err("cwd must be a directory".into()); }
    Ok(path)
}

fn resolve_ready_agent(app: &AppHandle, pubkey: &str) -> Result<(), String> {
    let records = load_managed_agents(app)?;
    let record = records.iter().find(|record| record.pubkey.eq_ignore_ascii_case(pubkey)).ok_or("managed agent not found")?;
    if record.backend != BackendKind::Local { return Err("daemon runs require a local managed agent".into()); }
    let personas = load_personas(app).unwrap_or_default();
    let global = load_global_agent_config(app).unwrap_or_default();
    let command = record_agent_command(record, &personas);
    if known_acp_runtime(&command).and_then(|runtime| runtime.mcp_command).is_none() {
        return Err("selected managed agent runtime does not support daemon completion".into());
    }
    let effective = resolve_effective_agent_env(record, &personas, known_acp_runtime(&command), &global);
    if !matches!(agent_readiness(&effective), AgentReadiness::Ready) {
        return Err("managed agent is not ready".into());
    }
    Ok(())
}

async fn wait_for_control(
    app: &AppHandle,
    key: &ManagedAgentRuntimeKey,
    cancellation: &CancellationToken,
) -> Result<(String, String), String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let control = {
            let state = app.state::<AppState>();
            let runtimes = state.managed_agent_processes.lock().map_err(|error| error.to_string())?;
            runtimes.get(key).map(|runtime| (
                runtime.child.id(),
                runtime.daemon_control_token.clone(),
                runtime.daemon_control_ready_file.clone(),
            ))
        };
        if let Some((pid, token, path)) = control {
            let safe_ready_file = std::fs::symlink_metadata(&path)
                .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink());
            if safe_ready_file {
                let bytes = std::fs::read(path).unwrap_or_default();
                if let Ok(ready) = serde_json::from_slice::<ControlReady>(&bytes) {
                    if ready.pid == pid {
                        let base_url = validate_loopback_control_url(&ready.base_url)?;
                        let response = reqwest::Client::new()
                            .get(format!("{base_url}/v1/ping"))
                            .bearer_auth(&token)
                            .timeout(Duration::from_secs(1))
                            .send()
                            .await;
                        if response.is_ok_and(|response| response.status().is_success()) {
                            return Ok((base_url, token));
                        }
                    }
                }
            }
        }
        if cancellation.is_cancelled() {
            return Err("daemon run cancelled before control endpoint became ready".into());
        }
        if tokio::time::Instant::now() >= deadline { return Err("managed agent daemon control endpoint did not become ready".into()); }
        tokio::select! {
            _ = cancellation.cancelled() => {
                return Err("daemon run cancelled before control endpoint became ready".into());
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
}

async fn parse_control_response(
    response: Result<reqwest::Response, reqwest::Error>,
) -> Result<ControlRunResponse, String> {
    match response {
        Ok(response) if response.status().is_success() => response
            .json::<ControlRunResponse>()
            .await
            .map_err(|error| safe_diagnostic(&error.to_string())),
        Ok(response) => Err(format!(
            "daemon control rejected run with status {}",
            response.status()
        )),
        Err(error) => Err(safe_diagnostic(&error.to_string())),
    }
}

fn render_wake_instruction(
    wake_instruction: &str,
    context_snapshot: Option<&serde_json::Value>,
) -> Result<String, String> {
    let Some(context) = context_snapshot else {
        return Ok(wake_instruction.to_string());
    };
    let context = serde_json::to_string_pretty(context).map_err(|error| error.to_string())?;
    let rendered = format!(
        "{wake_instruction}\n\nCanonical wake context snapshot:\n```json\n{context}\n```"
    );
    if rendered.len() > MAX_RENDERED_WAKE_BYTES {
        return Err("rendered wake instruction exceeds 96 KiB".into());
    }
    Ok(rendered)
}

fn validate_loopback_control_url(value: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(value).map_err(|_| "invalid daemon control URL")?;
    if url.scheme() != "http"
        || url.username() != ""
        || url.password().is_some()
        || (!url.path().is_empty() && url.path() != "/")
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.host().is_some_and(|host| match host {
            url::Host::Ipv4(address) => address.is_loopback(),
            url::Host::Ipv6(address) => address.is_loopback(),
            url::Host::Domain(_) => false,
        })
    {
        return Err("daemon control URL must be a bare HTTP loopback origin".into());
    }
    Ok(value.trim_end_matches('/').to_string())
}

fn daemon_run_marker(run_id: Uuid) -> String {
    format!("daemon-run:{run_id}")
}

fn persist_run_record(app: &AppHandle, record: &DaemonRunRecord) -> Result<(), String> {
    let dir = managed_agents_base_dir(app)?.join("daemon-runs");
    std::fs::create_dir_all(&dir).map_err(|error| format!("failed to create daemon run dir: {error}"))?;
    let payload = serde_json::to_vec_pretty(record).map_err(|error| error.to_string())?;
    atomic_write_json_restricted(&dir.join(format!("{}.json", record.run_id)), &payload)
}

fn safe_diagnostic(value: &str) -> String { value.chars().take(MAX_DIAGNOSTIC_CHARS).collect() }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn loader_accepts_portable_package_and_rejects_mismatch() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(".agents/daemons/cleanup");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("DAEMON.md"), "---\nid: cleanup\npurpose: test\nroutines:\n  - test\nwatch:\n  - test\n---\n# Policy\n").unwrap();
        assert!(load_daemon_policy(root.path(), "cleanup").is_ok());
        assert!(load_daemon_policy(root.path(), "other").is_err());

        std::fs::write(
            dir.join("DAEMON.md"),
            "---\nid: cleanup\npurpose: test\nroutines: []\n---\n# Policy\n",
        )
        .unwrap();
        assert!(load_daemon_policy(root.path(), "cleanup").is_err());
    }

    #[test]
    fn terminal_record_contains_required_correlation_fields() {
        let record = DaemonRunRecord {
            run_id: Uuid::new_v4().to_string(), daemon_id: "cleanup".into(), acp_session_id: Some("session-1".into()),
            status: DaemonRunStatus::Succeeded, started_at: "start".into(), completed_at: "end".into(),
            agent_pubkey: "a".repeat(64), channel_id: Uuid::new_v4().to_string(), relay_url: "ws://localhost".into(),
            cwd: "/tmp".into(), context_snapshot: Some(serde_json::json!({"source":"test"})),
            diagnostic: None, output_event_id: Some("event".into()),
        };
        let value = serde_json::to_value(record).unwrap();
        assert_eq!(value["acpSessionId"], "session-1");
        assert_eq!(value["outputEventId"], "event");
        assert_eq!(value["contextSnapshot"]["source"], "test");
    }

    #[test]
    fn wake_context_is_bounded_and_forwarded() {
        let context = serde_json::json!({"channel":"engineering", "eventId":"abc"});
        let rendered = render_wake_instruction("summarize", Some(&context)).unwrap();
        assert!(rendered.starts_with("summarize"));
        assert!(rendered.contains("\"eventId\": \"abc\""));
    }

    #[test]
    fn control_url_must_be_loopback_before_bearer_is_sent() {
        assert_eq!(
            validate_loopback_control_url("http://127.0.0.1:4321/").unwrap(),
            "http://127.0.0.1:4321"
        );
        assert!(validate_loopback_control_url("https://127.0.0.1:4321").is_err());
        assert!(validate_loopback_control_url("http://example.com:4321").is_err());
        assert!(validate_loopback_control_url("http://127.0.0.1:4321/path").is_err());
    }

    #[test]
    fn publication_marker_is_stable_for_idempotent_retries() {
        let run_id = Uuid::new_v4();
        assert_eq!(daemon_run_marker(run_id), format!("daemon-run:{run_id}"));
    }
}
