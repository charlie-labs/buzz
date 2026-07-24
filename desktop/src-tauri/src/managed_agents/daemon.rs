//! Managed daemon execution using the safe app-data library, bindings, and durable history.

use std::{sync::atomic::Ordering, time::Duration};

use chrono::{DateTime, Timelike as _, Utc};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    agent_readiness, finalize_run, get_binding_internal, known_acp_runtime,
    load_global_agent_config, load_managed_agents, load_managed_package, load_personas,
    mark_run_publishing, mark_run_running, record_agent_command, reserve_run,
    resolve_effective_agent_env, start_managed_agent_runtime_pair_lazy, terminalize_run,
    AgentReadiness, BackendKind, DaemonRunRecord, DaemonRunStatus, ManagedAgentRuntimeKey,
    RunManagedDaemonRequest,
};
use crate::{
    app_state::AppState,
    commands::{
        managed_agent_channel_message_event_id_by_marker, managed_agent_submission_auth_tag,
        send_managed_agent_channel_message_inner,
    },
};

const MAX_WAKE_BYTES: usize = 32 * 1024;
const IDLE_TIMEOUT_SECONDS: u64 = 300;
const HARD_TIMEOUT_SECONDS: u64 = 1800;
const MAX_DIAGNOSTIC_CHARS: usize = 512;

pub(crate) struct DaemonControlConfig {
    pub token: String,
    pub ready_file: std::path::PathBuf,
}

pub(crate) fn daemon_control(
    app: &AppHandle,
    runtime_key: &ManagedAgentRuntimeKey,
    command: &mut std::process::Command,
    start_nonce: &str,
) -> Result<DaemonControlConfig, String> {
    let token = Uuid::new_v4().simple().to_string();
    let ready_file = super::managed_agents_base_dir(app)?
        .join("control")
        .join(format!("{}.json", runtime_key.runtime_id()));
    if let Some(parent) = ready_file.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create daemon control dir: {error}"))?;
    }
    let _ = std::fs::remove_file(&ready_file);
    command
        .env("BUZZ_MANAGED_AGENT", super::current_instance_id(app))
        .env("BUZZ_MANAGED_AGENT_START_NONCE", start_nonce)
        .env("BUZZ_ACP_CONTROL_TOKEN", &token)
        .env("BUZZ_ACP_CONTROL_READY_FILE", &ready_file);
    Ok(DaemonControlConfig { token, ready_file })
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
    let wake = request.wake_instruction.trim();
    if wake.is_empty() || wake.len() > MAX_WAKE_BYTES {
        return Err("wake instruction is empty or exceeds 32 KiB".into());
    }
    if matches!(request.trigger, super::DaemonRunTrigger::Schedule)
        != request.scheduled_for_utc.is_some()
    {
        return Err("scheduled daemon runs require an explicit UTC occurrence".into());
    }
    if let Some(scheduled_for) = request.scheduled_for_utc.as_deref() {
        validate_scheduled_occurrence(scheduled_for, Utc::now())?;
    }
    let binding = get_binding_internal(&app, request.binding_id.trim())?;
    let package = load_managed_package(&app, &binding.daemon_id)?;
    let cwd =
        super::revalidate_binding_context(&binding)?.unwrap_or_else(|| package.directory.clone());
    let reservation = {
        let _guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|error| error.to_string())?;
        reserve_run(
            &app,
            &run_id.to_string(),
            &binding,
            &package.package_hash,
            wake,
            request.trigger,
            request.scheduled_for_utc.as_deref(),
        )?
    };
    let mut record = match reservation {
        super::Reservation::ExistingTerminal(record) => return Ok(record),
        super::Reservation::ExistingPublishing(record) => {
            if let Some(event_id) = managed_agent_channel_message_event_id_by_marker(
                &state,
                &record.binding.agent_pubkey,
                &record.binding.channel_id,
                &daemon_run_marker(run_id),
            )
            .await?
            {
                let _guard = state
                    .managed_agents_store_lock
                    .lock()
                    .map_err(|error| error.to_string())?;
                return super::finish_publishing_recovery(&app, &record.run_id, event_id);
            }
            return Err(
                "daemon run is in recoverable publishing state; marker was not found and execution will not be relaunched"
                    .into(),
            );
        }
        super::Reservation::New(record) => record,
    };

    let cancellation = CancellationToken::new();
    {
        let mut active = state
            .daemon_runtime
            .cancellations
            .lock()
            .map_err(|error| error.to_string())?;
        if active.contains_key(&run_id) {
            terminalize_safely(
                &app,
                &state,
                &mut record,
                DaemonRunStatus::Interrupted,
                None,
                Some("duplicate in-memory run registration"),
            )?;
            return Err("run UUID is already active".into());
        }
        active.insert(run_id, cancellation.clone());
    }

    let result = run_managed_daemon_inner(
        &app,
        &state,
        run_id,
        wake,
        &binding,
        &package.daemon_md,
        cwd,
        &mut record,
        cancellation,
    )
    .await;
    if let Ok(mut active) = state.daemon_runtime.cancellations.lock() {
        active.remove(&run_id);
    }
    result
}

#[allow(clippy::too_many_arguments)]
async fn run_managed_daemon_inner(
    app: &AppHandle,
    state: &AppState,
    run_id: Uuid,
    wake: &str,
    binding: &super::DaemonBinding,
    policy: &str,
    cwd: std::path::PathBuf,
    record: &mut DaemonRunRecord,
    cancellation: CancellationToken,
) -> Result<DaemonRunRecord, String> {
    if let Err((_, reason)) = daemon_binding_readiness(app, binding) {
        let error = reason.unwrap_or_else(|| "daemon binding is not ready".into());
        terminalize_safely(
            app,
            state,
            record,
            DaemonRunStatus::Failed,
            None,
            Some(&error),
        )?;
        return Ok(record.clone());
    }
    if let Err(error) = validate_daemon_output_channel(app, binding).await {
        let status = if record.trigger == super::DaemonRunTrigger::Schedule {
            DaemonRunStatus::SkippedUnready
        } else {
            DaemonRunStatus::Failed
        };
        terminalize_safely(
            app,
            state,
            record,
            status,
            None,
            Some(&error),
        )?;
        return Ok(record.clone());
    }
    let key = ManagedAgentRuntimeKey::new(binding.agent_pubkey.clone(), &binding.relay_url)?;
    {
        let _guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|error| error.to_string())?;
        mark_run_running(app, record)?;
    }
    if let Err(error) = start_managed_agent_runtime_pair_lazy(
        key.pubkey.clone(),
        key.relay_url.clone(),
        app.clone(),
    ) {
        terminalize_safely(
            app,
            state,
            record,
            DaemonRunStatus::Failed,
            None,
            Some(&error),
        )?;
        return Ok(record.clone());
    }
    let (base_url, bearer) = match wait_for_control(app, &key, &cancellation).await {
        Ok(control) => control,
        Err(error) => {
            let status = if cancellation.is_cancelled() {
                DaemonRunStatus::Cancelled
            } else {
                DaemonRunStatus::Failed
            };
            terminalize_safely(app, state, record, status, None, Some(&error))?;
            return Ok(record.clone());
        }
    };

    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(HARD_TIMEOUT_SECONDS + 15))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            terminalize_safely(
                app,
                state,
                record,
                DaemonRunStatus::Failed,
                None,
                Some(&error.to_string()),
            )?;
            return Ok(record.clone());
        }
    };
    let Some(cwd) = cwd.to_str() else {
        terminalize_safely(
            app,
            state,
            record,
            DaemonRunStatus::Failed,
            None,
            Some("daemon working directory is not valid UTF-8"),
        )?;
        return Ok(record.clone());
    };
    let body = ControlRunRequest {
        run_id,
        policy_markdown: policy,
        wake_instruction: wake,
        cwd,
        idle_timeout_seconds: IDLE_TIMEOUT_SECONDS,
        hard_timeout_seconds: HARD_TIMEOUT_SECONDS,
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
    let response = match control_result {
        Ok(response) if response.run_id == run_id => response,
        Ok(_) => {
            terminalize_safely(
                app,
                state,
                record,
                DaemonRunStatus::Failed,
                None,
                Some("daemon control returned a mismatched run UUID"),
            )?;
            return Ok(record.clone());
        }
        Err(error) => {
            terminalize_safely(
                app,
                state,
                record,
                DaemonRunStatus::Failed,
                None,
                Some(&error),
            )?;
            return Ok(record.clone());
        }
    };
    if response.status == "cancelled" {
        terminalize_safely(
            app,
            state,
            record,
            DaemonRunStatus::Cancelled,
            response.session_id,
            response.diagnostic.as_deref(),
        )?;
        return Ok(record.clone());
    }
    let output = match classify_control_completion(&response.status, response.output_markdown) {
        Ok(None) => {
            terminalize_safely(
                app,
                state,
                record,
                DaemonRunStatus::NoOp,
                response.session_id,
                None,
            )?;
            return Ok(record.clone());
        }
        Ok(Some(output)) => output,
        Err(error) => {
            terminalize_safely(
                app,
                state,
                record,
                DaemonRunStatus::Failed,
                response.session_id,
                response.diagnostic.as_deref().or(Some(error)),
            )?;
            return Ok(record.clone());
        }
    };

    {
        let _guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|error| error.to_string())?;
        mark_run_publishing(app, record, response.session_id)?;
    }
    let event_id = send_managed_agent_channel_message_inner(
        binding.agent_pubkey.clone(),
        binding.channel_id.clone(),
        output,
        Some(daemon_run_marker(run_id)),
        Some("agent".into()),
        None,
        None,
        None,
        app,
        state,
    )
    .await
    .map_err(|error| {
        format!("daemon output publication failed; publishing state retained for recovery: {error}")
    })?
    .event_id;
    let final_result = {
        let _guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|error| error.to_string())?;
        finalize_run(app, record, event_id)
    };
    final_result.map_err(|error| format!("daemon output was published but final receipt persistence failed; recover by run marker: {error}"))?;
    Ok(record.clone())
}

fn terminalize_safely(
    app: &AppHandle,
    state: &AppState,
    record: &mut DaemonRunRecord,
    status: DaemonRunStatus,
    session_id: Option<String>,
    diagnostic: Option<&str>,
) -> Result<(), String> {
    let _guard = state
        .managed_agents_store_lock
        .lock()
        .map_err(|error| error.to_string())?;
    terminalize_run(app, record, status, session_id, diagnostic)
}

#[tauri::command]
pub fn cancel_managed_daemon(run_id: String, app: AppHandle) -> Result<(), String> {
    let run_id = Uuid::parse_str(run_id.trim()).map_err(|_| "invalid run UUID")?;
    let state = app.state::<AppState>();
    let active = state
        .daemon_runtime
        .cancellations
        .lock()
        .map_err(|error| error.to_string())?;
    let cancellation = active.get(&run_id).ok_or("daemon run is not active")?;
    cancellation.cancel();
    Ok(())
}

pub(crate) fn cancel_all_daemon_runs(app: &AppHandle) {
    if let Ok(active) = app
        .state::<AppState>()
        .daemon_runtime
        .cancellations
        .lock()
    {
        for cancellation in active.values() {
            cancellation.cancel();
        }
    }
}

pub(crate) fn daemon_binding_readiness(
    app: &AppHandle,
    binding: &super::DaemonBinding,
) -> Result<(), (super::DaemonScheduleReadiness, Option<String>)> {
    let result = (|| {
        validate_daemon_output_channel_scope(app.state::<AppState>().inner(), binding)?;
        let records = load_managed_agents(app)?;
        let record = records
            .iter()
            .find(|record| record.pubkey.eq_ignore_ascii_case(&binding.agent_pubkey))
            .ok_or("managed agent not found")?;
        if record.relay_url.trim_end_matches('/') != binding.relay_url.trim_end_matches('/') {
            return Err("binding relay does not match the managed agent relay".into());
        }
        resolve_ready_agent_record(app, record)
    })();
    result.map_err(|error| {
        let readiness = if error.contains("output channel") || error.contains("channel UUID") {
            super::DaemonScheduleReadiness::ChannelUnavailable
        } else if error.contains("not found") {
            super::DaemonScheduleReadiness::MissingAgent
        } else if error.contains("relay") {
            super::DaemonScheduleReadiness::RelayMismatch
        } else if error.contains("does not support") || error.contains("local managed agent") {
            super::DaemonScheduleReadiness::UnsupportedRuntime
        } else {
            super::DaemonScheduleReadiness::AgentNotReady
        };
        (readiness, Some(error))
    })
}

fn validate_daemon_output_channel_scope(
    state: &AppState,
    binding: &super::DaemonBinding,
) -> Result<(), String> {
    Uuid::parse_str(&binding.channel_id).map_err(|_| "invalid output channel UUID".to_string())?;
    let active = ManagedAgentRuntimeKey::new(
        binding.agent_pubkey.clone(),
        &crate::relay::relay_ws_url_with_override(state),
    )?;
    let configured = ManagedAgentRuntimeKey::new(
        binding.agent_pubkey.clone(),
        &binding.relay_url,
    )?;
    if active.relay_url != configured.relay_url {
        return Err(
            "output channel belongs to a different relay; choose a channel in the active community"
                .into(),
        );
    }
    Ok(())
}

pub(crate) async fn validate_daemon_output_channel(
    app: &AppHandle,
    binding: &super::DaemonBinding,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    validate_daemon_output_channel_scope(&state, binding)?;
    let record = load_managed_agents(app)?
        .into_iter()
        .find(|record| record.pubkey.eq_ignore_ascii_case(&binding.agent_pubkey))
        .ok_or_else(|| "managed agent not found".to_string())?;
    let keys = nostr::Keys::parse(record.private_key_nsec.trim())
        .map_err(|error| format!("failed to parse managed agent key: {error}"))?;
    if !keys
        .public_key()
        .to_hex()
        .eq_ignore_ascii_case(&binding.agent_pubkey)
    {
        return Err("managed agent key does not match the binding identity".into());
    }
    let auth_tag = managed_agent_submission_auth_tag(&record, &state, &keys.public_key())?;
    let events = crate::relay::query_relay_at_with_keys(
        &state,
        &crate::relay::relay_api_base_url_with_override(&state),
        &[
            serde_json::json!({"kinds": [39000], "#d": [&binding.channel_id], "limit": 1}),
            serde_json::json!({"kinds": [39002], "#d": [&binding.channel_id], "limit": 1}),
        ],
        &keys,
        auth_tag.as_deref(),
    )
    .await
    .map_err(|error| format!("could not validate output channel readiness: {error}"))?;
    validate_daemon_output_channel_events(&events, &binding.agent_pubkey)
}

fn validate_daemon_output_channel_events(
    events: &[nostr::Event],
    agent_pubkey: &str,
) -> Result<(), String> {
    let metadata = events
        .iter()
        .find(|event| event.kind.as_u16() == 39000)
        .ok_or_else(|| {
            "output channel is unavailable on the active relay; choose an existing channel"
                .to_string()
        })?;
    let channel = crate::nostr_convert::channel_info_from_event(metadata, None, None)
        .map_err(|error| format!("output channel metadata is invalid: {error}"))?;
    if channel.visibility == "open" {
        return Ok(());
    }
    let members = events
        .iter()
        .find(|event| event.kind.as_u16() == 39002)
        .ok_or_else(|| "output channel membership is unavailable for the managed agent".to_string())?;
    let membership = crate::nostr_convert::channel_members_from_event(members)
        .map_err(|error| format!("output channel membership is invalid: {error}"))?;
    if !membership
        .members
        .iter()
        .any(|member| member.pubkey.eq_ignore_ascii_case(agent_pubkey))
    {
        return Err(
            "managed agent is not a member of the private output channel; add it before running the daemon"
                .into(),
        );
    }
    Ok(())
}

fn resolve_ready_agent_record(
    app: &AppHandle,
    record: &super::ManagedAgentRecord,
) -> Result<(), String> {
    if record.backend != BackendKind::Local {
        return Err("daemon runs require a local managed agent".into());
    }
    let personas = load_personas(app).unwrap_or_default();
    let global = load_global_agent_config(app).unwrap_or_default();
    let command = record_agent_command(record, &personas);
    if known_acp_runtime(&command)
        .and_then(|runtime| runtime.mcp_command)
        .is_none()
    {
        return Err("selected managed agent runtime does not support daemon completion".into());
    }
    let effective =
        resolve_effective_agent_env(record, &personas, known_acp_runtime(&command), &global);
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
            let runtimes = state
                .managed_agent_processes
                .lock()
                .map_err(|error| error.to_string())?;
            runtimes.get(key).map(|runtime| {
                (
                    runtime.child.id(),
                    runtime.daemon_control_token.clone(),
                    runtime.daemon_control_ready_file.clone(),
                )
            })
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
        if tokio::time::Instant::now() >= deadline {
            return Err("managed agent daemon control endpoint did not become ready".into());
        }
        tokio::select! {
            _ = cancellation.cancelled() => return Err("daemon run cancelled before control endpoint became ready".into()),
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

fn classify_control_completion(
    status: &str,
    output_markdown: Option<String>,
) -> Result<Option<String>, &'static str> {
    match status {
        "no_op" => Ok(None),
        "succeeded" => output_markdown
            .filter(|output| !output.trim().is_empty())
            .map(Some)
            .ok_or("successful daemon response omitted output"),
        _ => Err("daemon execution failed"),
    }
}

fn validate_scheduled_occurrence(value: &str, now: DateTime<Utc>) -> Result<(), String> {
    let parsed = DateTime::parse_from_rfc3339(value)
        .map_err(|error| format!("invalid scheduled UTC occurrence: {error}"))?;
    if parsed.offset().local_minus_utc() != 0 {
        return Err("scheduled occurrence must use an explicit UTC offset".into());
    }
    let occurrence = parsed.with_timezone(&Utc);
    let minute = |value: DateTime<Utc>| {
        value
            .with_second(0)
            .and_then(|value| value.with_nanosecond(0))
    };
    if minute(occurrence) != minute(now) {
        return Err("scheduled occurrence is no longer in the current UTC minute".into());
    }
    Ok(())
}

fn safe_diagnostic(value: &str) -> String {
    value.chars().take(MAX_DIAGNOSTIC_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};

    fn channel_event(kind: u16, tags: Vec<Vec<String>>) -> nostr::Event {
        EventBuilder::new(Kind::Custom(kind), "")
            .tags(
                tags.into_iter()
                    .map(|tag| Tag::parse(tag).expect("valid channel tag")),
            )
            .sign_with_keys(&Keys::generate())
            .expect("sign channel event")
    }

    #[test]
    fn request_rejects_removed_renderer_paths_and_camel_case_is_stable() {
        let value = serde_json::json!({"runId": Uuid::new_v4().to_string(), "bindingId": Uuid::new_v4().to_string(), "wakeInstruction": "run now", "packageRoot": "/tmp"});
        assert!(serde_json::from_value::<RunManagedDaemonRequest>(value).is_err());
    }

    #[test]
    fn control_url_must_be_loopback_before_bearer_is_sent() {
        assert_eq!(
            validate_loopback_control_url("http://127.0.0.1:4321/").unwrap(),
            "http://127.0.0.1:4321"
        );
        assert!(validate_loopback_control_url("https://127.0.0.1:4321").is_err());
        assert!(validate_loopback_control_url("http://example.com:4321").is_err());
    }

    #[test]
    fn publication_marker_is_stable() {
        let run_id = Uuid::new_v4();
        assert_eq!(daemon_run_marker(run_id), format!("daemon-run:{run_id}"));
    }

    #[test]
    fn control_completion_no_op_has_no_publication_payload() {
        assert_eq!(classify_control_completion("no_op", None).unwrap(), None);
        assert_eq!(
            classify_control_completion("succeeded", Some("# Done".into())).unwrap(),
            Some("# Done".into())
        );
        assert!(classify_control_completion("succeeded", None).is_err());
        assert!(classify_control_completion("unknown", None).is_err());
    }

    #[test]
    fn output_channel_events_reject_unknown_and_accept_open_channel() {
        let channel_id = Uuid::new_v4().to_string();
        let agent_pubkey = "aa".repeat(32);
        let error = validate_daemon_output_channel_events(&[], &agent_pubkey).unwrap_err();
        assert!(error.contains("unavailable"));

        let metadata = channel_event(
            39000,
            vec![
                vec!["d".into(), channel_id],
                vec!["visibility".into(), "open".into()],
            ],
        );
        assert!(validate_daemon_output_channel_events(&[metadata], &agent_pubkey).is_ok());
    }

    #[test]
    fn output_channel_scope_rejects_stale_relay_and_accepts_active_relay() {
        let state = crate::app_state::build_app_state();
        *state.relay_url_override.lock().unwrap() = Some("wss://active.example".into());
        let mut binding = super::super::DaemonBinding {
            id: Uuid::new_v4().to_string(),
            daemon_id: "safe".into(),
            agent_pubkey: "aa".repeat(32),
            relay_url: "wss://stale.example".into(),
            channel_id: Uuid::new_v4().to_string(),
            context_directory: None,
            context_configured: false,
            schedule_enabled: false,
            created_at: "2026-07-24T00:00:00Z".into(),
            updated_at: "2026-07-24T00:00:00Z".into(),
        };
        assert!(validate_daemon_output_channel_scope(&state, &binding)
            .unwrap_err()
            .contains("different relay"));
        binding.relay_url = "wss://active.example".into();
        assert!(validate_daemon_output_channel_scope(&state, &binding).is_ok());
    }

    #[test]
    fn scheduled_occurrence_must_be_current_utc_minute() {
        let now = DateTime::parse_from_rfc3339("2026-07-24T21:00:59Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(validate_scheduled_occurrence("2026-07-24T21:00:00Z", now).is_ok());
        assert!(validate_scheduled_occurrence("2026-07-24T20:59:00Z", now).is_err());
        assert!(validate_scheduled_occurrence("2026-07-24T23:00:00+02:00", now).is_err());
    }
}
