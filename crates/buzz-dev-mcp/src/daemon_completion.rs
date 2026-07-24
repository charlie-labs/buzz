use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router, ErrorData, ServerHandler,
};
use serde::{Deserialize, Serialize};

const MAX_OUTPUT_BYTES: usize = 128 * 1024;

#[derive(Clone)]
pub(crate) struct DaemonCompletionMcp {
    callback_url: String,
    callback_token: String,
    run_id: String,
    client: reqwest::Client,
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CompletionOutcome {
    #[default]
    Succeeded,
    NoOp,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct CompleteParams {
    /// Terminal outcome. Omit for succeeded; use no_op only when there is no user-visible work.
    #[serde(default)]
    outcome: CompletionOutcome,
    /// Final markdown to publish. Required for succeeded; optional for no_op.
    markdown: Option<String>,
}

#[tool_router]
impl DaemonCompletionMcp {
    pub(crate) fn from_env() -> Option<Self> {
        Some(Self {
            callback_url: std::env::var("BUZZ_DAEMON_CALLBACK_URL").ok()?,
            callback_token: std::env::var("BUZZ_DAEMON_CALLBACK_TOKEN").ok()?,
            run_id: std::env::var("BUZZ_DAEMON_RUN_ID").ok()?,
            client: reqwest::Client::new(),
            tool_router: Self::tool_router(),
        })
    }

    #[tool(
        name = "daemon_complete",
        description = "Complete this daemon activation exactly once. Omit outcome (or use succeeded) with non-empty markdown to publish. Use no_op without markdown only when there is no user-visible result."
    )]
    async fn complete(
        &self,
        Parameters(params): Parameters<CompleteParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let markdown = params.markdown.unwrap_or_default();
        if markdown.len() > MAX_OUTPUT_BYTES
            || (matches!(params.outcome, CompletionOutcome::Succeeded)
                && markdown.trim().is_empty())
        {
            return Ok(CallToolResult::error(vec![Content::text(
                "markdown must be non-empty for succeeded and at most 128 KiB",
            )]));
        }
        let response = self
            .client
            .post(&self.callback_url)
            .json(&serde_json::json!({
                "runId": self.run_id,
                "callbackToken": self.callback_token,
                "outcome": params.outcome,
                "markdown": markdown,
            }))
            .send()
            .await
            .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
        if !response.status().is_success() {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "completion callback rejected with status {}",
                response.status()
            ))]));
        }
        Ok(CallToolResult::success(vec![Content::text(
            "daemon completion accepted",
        )]))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for DaemonCompletionMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(rmcp::model::Implementation::new(
                "buzz-daemon-completion",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions("Call daemon_complete exactly once before ending the turn. Use no_op only after confirming there is no user-visible result to publish.")
    }
}

#[cfg(test)]
mod tests {
    use super::{CompleteParams, CompletionOutcome};

    #[test]
    fn completion_params_preserve_legacy_success_and_accept_no_op() {
        let legacy: CompleteParams = serde_json::from_value(serde_json::json!({
            "markdown": "# Done"
        }))
        .unwrap();
        assert!(matches!(legacy.outcome, CompletionOutcome::Succeeded));

        let explicit: CompleteParams = serde_json::from_value(serde_json::json!({
            "outcome": "succeeded",
            "markdown": "# Done"
        }))
        .unwrap();
        assert!(matches!(explicit.outcome, CompletionOutcome::Succeeded));

        let no_op: CompleteParams = serde_json::from_value(serde_json::json!({
            "outcome": "no_op"
        }))
        .unwrap();
        assert!(matches!(no_op.outcome, CompletionOutcome::NoOp));
        assert!(no_op.markdown.is_none());

        assert!(serde_json::from_value::<CompleteParams>(serde_json::json!({
            "outcome": "unknown"
        }))
        .is_err());
    }
}
