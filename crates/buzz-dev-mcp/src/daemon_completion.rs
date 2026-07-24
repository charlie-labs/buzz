use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router, ErrorData, ServerHandler,
};
use serde::Deserialize;

const MAX_OUTPUT_BYTES: usize = 128 * 1024;

#[derive(Clone)]
pub(crate) struct DaemonCompletionMcp {
    callback_url: String,
    callback_token: String,
    run_id: String,
    client: reqwest::Client,
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct CompleteParams {
    /// Final markdown to publish as the managed agent's daemon result.
    markdown: String,
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
        description = "Complete this daemon activation exactly once. Supply the final markdown payload that Buzz should publish."
    )]
    async fn complete(
        &self,
        Parameters(params): Parameters<CompleteParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if params.markdown.trim().is_empty() || params.markdown.len() > MAX_OUTPUT_BYTES {
            return Ok(CallToolResult::error(vec![Content::text(
                "markdown must be non-empty and at most 128 KiB",
            )]));
        }
        let response = self
            .client
            .post(&self.callback_url)
            .json(&serde_json::json!({
                "runId": self.run_id,
                "callbackToken": self.callback_token,
                "markdown": params.markdown,
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
            .with_instructions("Call daemon_complete exactly once before ending the turn.")
    }
}
