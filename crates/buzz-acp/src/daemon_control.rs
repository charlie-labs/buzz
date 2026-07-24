//! Authenticated loopback control plane for bounded daemon activations.

use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq as _;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    acp::{AcpClient, EnvVar, McpServer, StopReason},
    config::Config,
};

const MAX_REQUEST_BYTES: usize = 384 * 1024;
const MAX_POLICY_BYTES: usize = 256 * 1024;
const MAX_WAKE_BYTES: usize = 96 * 1024;
const MAX_OUTPUT_BYTES: usize = 128 * 1024;
const MAX_TIMEOUT_SECS: u64 = 24 * 60 * 60;

#[derive(Clone)]
struct ExecutorConfig {
    command: String,
    args: Vec<String>,
    extra_env: Vec<(String, String)>,
    generated_codex_config: bool,
    mcp_command: String,
}

#[derive(Clone)]
struct ControlState {
    bearer: Arc<str>,
    executor: ExecutorConfig,
    base_url: Arc<str>,
    active: Arc<Mutex<HashMap<Uuid, ActiveRun>>>,
}

struct ActiveRun {
    cancel: CancellationToken,
    callback_token: String,
    completion: Arc<Mutex<Option<DaemonCompletion>>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DaemonRunRequest {
    run_id: Uuid,
    policy_markdown: String,
    wake_instruction: String,
    cwd: String,
    idle_timeout_seconds: u64,
    hard_timeout_seconds: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DaemonRunResponse {
    run_id: Uuid,
    session_id: Option<String>,
    status: &'static str,
    output_markdown: Option<String>,
    diagnostic: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompletionRequest {
    run_id: Uuid,
    callback_token: String,
    #[serde(default)]
    outcome: CompletionOutcome,
    #[serde(default)]
    markdown: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CompletionOutcome {
    #[default]
    Succeeded,
    NoOp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DaemonCompletion {
    outcome: CompletionOutcome,
    markdown: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadyFile {
    pid: u32,
    base_url: String,
}

pub(crate) async fn start(config: &Config) -> anyhow::Result<()> {
    let token = match std::env::var("BUZZ_ACP_CONTROL_TOKEN") {
        Ok(value) if !value.is_empty() => value,
        _ => return Ok(()),
    };
    let ready_path = PathBuf::from(std::env::var("BUZZ_ACP_CONTROL_READY_FILE")?);
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let address = listener.local_addr()?;
    let base_url = format!("http://127.0.0.1:{}", address.port());
    let state = ControlState {
        bearer: Arc::from(token),
        executor: ExecutorConfig {
            command: config.agent_command.clone(),
            args: config.agent_args.clone(),
            extra_env: config.persona_env_vars.clone(),
            generated_codex_config: config.has_generated_codex_config,
            mcp_command: config.mcp_command.clone(),
        },
        base_url: Arc::from(base_url.clone()),
        active: Arc::new(Mutex::new(HashMap::new())),
    };
    let router = Router::new()
        .route("/v1/ping", get(ping))
        .route("/v1/runs", post(run))
        .route("/v1/runs/{run_id}/cancel", post(cancel))
        .route("/v1/complete", post(complete))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .with_state(state);
    tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, router).await {
            tracing::warn!("daemon control server stopped: {error}");
        }
    });

    if let Some(parent) = ready_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let payload = serde_json::to_vec(&ReadyFile {
        pid: std::process::id(),
        base_url,
    })?;
    let temp = ready_path.with_extension(format!("{}.tmp", Uuid::new_v4().simple()));
    write_restricted(&temp, &payload)?;
    std::fs::rename(&temp, &ready_path)?;
    Ok(())
}

fn write_restricted(path: &std::path::Path, payload: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    std::io::Write::write_all(&mut file, payload)?;
    file.sync_all()
}

async fn ping(State(state): State<ControlState>, headers: HeaderMap) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(serde_json::json!({ "ready": true })).into_response()
}

async fn run(
    State(state): State<ControlState>,
    headers: HeaderMap,
    Json(request): Json<DaemonRunRequest>,
) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if state.executor.mcp_command.is_empty() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "managed runtime does not support daemon completion",
        )
            .into_response();
    }
    match validate_request(&request) {
        Ok(cwd) => Json(execute(state, request, cwd).await).into_response(),
        Err(error) => (StatusCode::BAD_REQUEST, error).into_response(),
    }
}

async fn cancel(
    State(state): State<ControlState>,
    headers: HeaderMap,
    Path(run_id): Path<Uuid>,
) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let active = state.active.lock().await;
    if let Some(run) = active.get(&run_id) {
        run.cancel.cancel();
        StatusCode::ACCEPTED.into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

async fn complete(
    State(state): State<ControlState>,
    Json(request): Json<CompletionRequest>,
) -> Response {
    let markdown = request.markdown.filter(|value| !value.trim().is_empty());
    if markdown
        .as_ref()
        .is_some_and(|value| value.len() > MAX_OUTPUT_BYTES)
        || (request.outcome == CompletionOutcome::Succeeded && markdown.is_none())
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let active = state.active.lock().await;
    let Some(run) = active.get(&request.run_id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !constant_time_eq(&run.callback_token, &request.callback_token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut completion = run.completion.lock().await;
    if completion.is_some() {
        return StatusCode::CONFLICT.into_response();
    }
    *completion = Some(DaemonCompletion {
        outcome: request.outcome,
        markdown,
    });
    StatusCode::NO_CONTENT.into_response()
}

async fn execute(
    state: ControlState,
    request: DaemonRunRequest,
    cwd: PathBuf,
) -> DaemonRunResponse {
    let cancel = CancellationToken::new();
    let callback_token = Uuid::new_v4().simple().to_string();
    let completion = Arc::new(Mutex::new(None));
    {
        let mut active = state.active.lock().await;
        if active.contains_key(&request.run_id) {
            return failed(request.run_id, None, "run ID is already active");
        }
        active.insert(
            request.run_id,
            ActiveRun {
                cancel: cancel.clone(),
                callback_token: callback_token.clone(),
                completion: completion.clone(),
            },
        );
    }

    let result = execute_inner(&state, &request, &cwd, &callback_token, cancel, &completion).await;
    state.active.lock().await.remove(&request.run_id);
    result
}

async fn execute_inner(
    state: &ControlState,
    request: &DaemonRunRequest,
    cwd: &std::path::Path,
    callback_token: &str,
    cancel: CancellationToken,
    completion: &Arc<Mutex<Option<DaemonCompletion>>>,
) -> DaemonRunResponse {
    let spawn = AcpClient::spawn(
        &state.executor.command,
        &state.executor.args,
        &state.executor.extra_env,
        state.executor.generated_codex_config,
    );
    let mut client = match tokio::select! {
        result = spawn => Some(result),
        _ = cancel.cancelled() => None,
    } {
        Some(Ok(client)) => client,
        Some(Err(error)) => {
            return failed(request.run_id, None, &safe_diagnostic(&error.to_string()));
        }
        None => return cancelled(request.run_id, None, "cancelled before agent startup"),
    };
    client.disable_wire_payload_logging();
    let initialization = tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(60), client.initialize()) => Some(result),
        _ = cancel.cancelled() => None,
    };
    match initialization {
        Some(Ok(Ok(_))) => {}
        Some(Ok(Err(error))) => {
            bounded_shutdown(&mut client).await;
            return failed(request.run_id, None, &safe_diagnostic(&error.to_string()));
        }
        Some(Err(_)) => {
            bounded_shutdown(&mut client).await;
            return failed(request.run_id, None, "agent initialization timed out");
        }
        None => {
            bounded_shutdown(&mut client).await;
            return cancelled(
                request.run_id,
                None,
                "cancelled during agent initialization",
            );
        }
    }

    let mcp_servers = daemon_mcp_server(state, request.run_id, callback_token);
    let cwd = cwd.to_string_lossy();
    let session_result = tokio::select! {
        result = tokio::time::timeout(
            Duration::from_secs(60),
            client.session_new_full(&cwd, mcp_servers, Some(&request.policy_markdown)),
        ) => Some(result),
        _ = cancel.cancelled() => None,
    };
    let session = match session_result {
        Some(Ok(Ok(session))) => session,
        Some(Ok(Err(error))) => {
            bounded_shutdown(&mut client).await;
            return failed(request.run_id, None, &safe_diagnostic(&error.to_string()));
        }
        Some(Err(_)) => {
            bounded_shutdown(&mut client).await;
            return failed(request.run_id, None, "agent session creation timed out");
        }
        None => {
            bounded_shutdown(&mut client).await;
            return cancelled(request.run_id, None, "cancelled during session creation");
        }
    };

    let prompt = format!(
        "{}\n\nBefore ending, call daemon_complete exactly once. Use succeeded with final markdown when there is user-visible work. Use no_op without markdown only when there is nothing to publish. Do not publish directly to Buzz.",
        request.wake_instruction.trim()
    );
    let result = tokio::select! {
        result = client.session_prompt_with_idle_timeout(
            &session.session_id,
            &prompt,
            Duration::from_secs(request.idle_timeout_seconds),
            Duration::from_secs(request.hard_timeout_seconds),
        ) => result.map_err(|error| safe_diagnostic(&error.to_string())),
        _ = cancel.cancelled() => {
            let _ = tokio::time::timeout(
                Duration::from_secs(6),
                client.cancel_with_cleanup_grace(&session.session_id, Duration::from_secs(5)),
            ).await;
            Ok(StopReason::Cancelled)
        }
    };
    bounded_shutdown(&mut client).await;

    match result {
        Ok(reason) => terminal_response(
            request.run_id,
            session.session_id,
            reason,
            completion.lock().await.take(),
        ),
        Err(error) => failed(request.run_id, Some(session.session_id), &error),
    }
}

async fn bounded_shutdown(client: &mut AcpClient) {
    let _ = tokio::time::timeout(Duration::from_secs(5), client.shutdown()).await;
}

fn terminal_response(
    run_id: Uuid,
    session_id: String,
    reason: StopReason,
    completion: Option<DaemonCompletion>,
) -> DaemonRunResponse {
    match (reason, completion) {
        (StopReason::Cancelled, _) => DaemonRunResponse {
            run_id,
            session_id: Some(session_id),
            status: "cancelled",
            output_markdown: None,
            diagnostic: None,
        },
        (
            StopReason::EndTurn,
            Some(DaemonCompletion {
                outcome: CompletionOutcome::Succeeded,
                markdown: Some(output),
            }),
        ) => DaemonRunResponse {
            run_id,
            session_id: Some(session_id),
            status: "succeeded",
            output_markdown: Some(output),
            diagnostic: None,
        },
        (
            StopReason::EndTurn,
            Some(DaemonCompletion {
                outcome: CompletionOutcome::NoOp,
                ..
            }),
        ) => DaemonRunResponse {
            run_id,
            session_id: Some(session_id),
            status: "no_op",
            output_markdown: None,
            diagnostic: None,
        },
        (StopReason::EndTurn, None) => failed(
            run_id,
            Some(session_id),
            "agent ended without daemon_complete callback",
        ),
        (reason, _) => failed(
            run_id,
            Some(session_id),
            &format!("agent stopped without completion: {reason:?}"),
        ),
    }
}

fn daemon_mcp_server(state: &ControlState, run_id: Uuid, callback_token: &str) -> Vec<McpServer> {
    if state.executor.mcp_command.is_empty() {
        return Vec::new();
    }
    vec![McpServer {
        name: "buzz-daemon-completion".into(),
        command: state.executor.mcp_command.clone(),
        args: Vec::new(),
        env: vec![
            EnvVar {
                name: "BUZZ_DAEMON_CALLBACK_URL".into(),
                value: format!("{}/v1/complete", state.base_url),
            },
            EnvVar {
                name: "BUZZ_DAEMON_CALLBACK_TOKEN".into(),
                value: callback_token.to_string(),
            },
            EnvVar {
                name: "BUZZ_DAEMON_RUN_ID".into(),
                value: run_id.to_string(),
            },
        ],
    }]
}

fn validate_request(request: &DaemonRunRequest) -> Result<PathBuf, String> {
    if request.policy_markdown.len() > MAX_POLICY_BYTES || request.policy_markdown.trim().is_empty()
    {
        return Err("daemon policy is empty or too large".into());
    }
    if request.wake_instruction.len() > MAX_WAKE_BYTES || request.wake_instruction.trim().is_empty()
    {
        return Err("wake instruction is empty or too large".into());
    }
    if request.idle_timeout_seconds == 0
        || request.hard_timeout_seconds == 0
        || request.idle_timeout_seconds >= request.hard_timeout_seconds
        || request.hard_timeout_seconds > MAX_TIMEOUT_SECS
    {
        return Err("timeouts must be positive, idle < hard, and hard <= 24 hours".into());
    }
    let cwd = std::fs::canonicalize(&request.cwd).map_err(|_| "cwd does not exist".to_string())?;
    if !cwd.is_absolute() || !cwd.is_dir() {
        return Err("cwd must resolve to an absolute directory".into());
    }
    Ok(cwd)
}

fn authorized(state: &ControlState, headers: &HeaderMap) -> bool {
    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|candidate| constant_time_eq(&state.bearer, candidate))
}

fn constant_time_eq(expected: &str, candidate: &str) -> bool {
    expected.len() == candidate.len() && bool::from(expected.as_bytes().ct_eq(candidate.as_bytes()))
}

fn safe_diagnostic(value: &str) -> String {
    value.chars().take(512).collect()
}

fn failed(run_id: Uuid, session_id: Option<String>, diagnostic: &str) -> DaemonRunResponse {
    DaemonRunResponse {
        run_id,
        session_id,
        status: "failed",
        output_markdown: None,
        diagnostic: Some(safe_diagnostic(diagnostic)),
    }
}

fn cancelled(run_id: Uuid, session_id: Option<String>, diagnostic: &str) -> DaemonRunResponse {
    DaemonRunResponse {
        run_id,
        session_id,
        status: "cancelled",
        output_markdown: None,
        diagnostic: Some(safe_diagnostic(diagnostic)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state(run_id: Uuid, callback_token: &str) -> ControlState {
        let completion = Arc::new(Mutex::new(None));
        let mut active = HashMap::new();
        active.insert(
            run_id,
            ActiveRun {
                cancel: CancellationToken::new(),
                callback_token: callback_token.into(),
                completion,
            },
        );
        ControlState {
            bearer: Arc::from("bearer"),
            executor: ExecutorConfig {
                command: String::new(),
                args: Vec::new(),
                extra_env: Vec::new(),
                generated_codex_config: false,
                mcp_command: String::new(),
            },
            base_url: Arc::from("http://127.0.0.1:1"),
            active: Arc::new(Mutex::new(active)),
        }
    }

    #[test]
    fn constant_time_auth_rejects_wrong_or_partial_tokens() {
        assert!(constant_time_eq("secret", "secret"));
        assert!(!constant_time_eq("secret", "secrex"));
        assert!(!constant_time_eq("secret", "sec"));
    }

    #[test]
    fn validation_canonicalizes_cwd_and_bounds_timeouts() {
        let dir = tempfile::tempdir().unwrap();
        let mut request = DaemonRunRequest {
            run_id: Uuid::new_v4(),
            policy_markdown: "policy".into(),
            wake_instruction: "wake".into(),
            cwd: dir.path().display().to_string(),
            idle_timeout_seconds: 10,
            hard_timeout_seconds: 20,
        };
        assert_eq!(
            validate_request(&request).unwrap(),
            dir.path().canonicalize().unwrap()
        );
        request.idle_timeout_seconds = 20;
        assert!(validate_request(&request).is_err());
    }

    #[test]
    fn terminal_response_requires_callback_and_preserves_cancellation() {
        let run_id = Uuid::new_v4();
        let success = terminal_response(
            run_id,
            "session".into(),
            StopReason::EndTurn,
            Some(DaemonCompletion {
                outcome: CompletionOutcome::Succeeded,
                markdown: Some("# Done".into()),
            }),
        );
        assert_eq!(success.status, "succeeded");
        assert_eq!(success.output_markdown.as_deref(), Some("# Done"));

        let missing = terminal_response(run_id, "session".into(), StopReason::EndTurn, None);
        assert_eq!(missing.status, "failed");
        assert!(missing
            .diagnostic
            .unwrap()
            .contains("without daemon_complete"));

        let cancelled = terminal_response(
            run_id,
            "session".into(),
            StopReason::Cancelled,
            Some(DaemonCompletion {
                outcome: CompletionOutcome::Succeeded,
                markdown: Some("ignored".into()),
            }),
        );
        assert_eq!(cancelled.status, "cancelled");
        assert!(cancelled.output_markdown.is_none());

        let no_op = terminal_response(
            run_id,
            "session".into(),
            StopReason::EndTurn,
            Some(DaemonCompletion {
                outcome: CompletionOutcome::NoOp,
                markdown: None,
            }),
        );
        assert_eq!(no_op.status, "no_op");
        assert!(no_op.output_markdown.is_none());
    }

    #[test]
    fn completion_request_wire_contract_defaults_success_and_rejects_unknown_outcomes() {
        let run_id = Uuid::new_v4();
        let legacy: CompletionRequest = serde_json::from_value(serde_json::json!({
            "runId": run_id,
            "callbackToken": "token",
            "markdown": "# Done"
        }))
        .unwrap();
        assert_eq!(legacy.outcome, CompletionOutcome::Succeeded);

        let explicit: CompletionRequest = serde_json::from_value(serde_json::json!({
            "runId": run_id,
            "callbackToken": "token",
            "outcome": "succeeded",
            "markdown": "# Done"
        }))
        .unwrap();
        assert_eq!(explicit.outcome, CompletionOutcome::Succeeded);

        assert!(
            serde_json::from_value::<CompletionRequest>(serde_json::json!({
                "runId": run_id,
                "callbackToken": "token",
                "outcome": "unknown"
            }))
            .is_err()
        );
    }

    #[tokio::test]
    async fn completion_callback_authenticates_and_accepts_exactly_once() {
        let run_id = Uuid::new_v4();
        let state = test_state(run_id, "callback-secret");
        let wrong = complete(
            State(state.clone()),
            Json(CompletionRequest {
                run_id,
                callback_token: "wrong".into(),
                outcome: CompletionOutcome::Succeeded,
                markdown: Some("# Done".into()),
            }),
        )
        .await;
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

        let accepted = complete(
            State(state.clone()),
            Json(CompletionRequest {
                run_id,
                callback_token: "callback-secret".into(),
                outcome: CompletionOutcome::Succeeded,
                markdown: Some("# Done".into()),
            }),
        )
        .await;
        assert_eq!(accepted.status(), StatusCode::NO_CONTENT);

        let duplicate = complete(
            State(state),
            Json(CompletionRequest {
                run_id,
                callback_token: "callback-secret".into(),
                outcome: CompletionOutcome::Succeeded,
                markdown: Some("# Again".into()),
            }),
        )
        .await;
        assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn completion_callback_accepts_no_op_and_rejects_success_without_markdown() {
        let no_op_run = Uuid::new_v4();
        let no_op = complete(
            State(test_state(no_op_run, "callback-secret")),
            Json(CompletionRequest {
                run_id: no_op_run,
                callback_token: "callback-secret".into(),
                outcome: CompletionOutcome::NoOp,
                markdown: None,
            }),
        )
        .await;
        assert_eq!(no_op.status(), StatusCode::NO_CONTENT);

        let invalid_run = Uuid::new_v4();
        let invalid = complete(
            State(test_state(invalid_run, "callback-secret")),
            Json(CompletionRequest {
                run_id: invalid_run,
                callback_token: "callback-secret".into(),
                outcome: CompletionOutcome::Succeeded,
                markdown: None,
            }),
        )
        .await;
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    }
}
