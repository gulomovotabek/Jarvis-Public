use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct SwitchFocusTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SwitchFocusParams {
    /// Target chat id. Omit or pass null to clear focus.
    #[serde(default)]
    pub chat_id: Option<i64>,
}

impl Tool for SwitchFocusTool {
    type Params = SwitchFocusParams;

    fn name(&self) -> &'static str {
        "switch_focus"
    }

    fn description(&self) -> &'static str {
        "Switch the bot's attention to a specific chat (or pass nothing to clear focus)."
    }

    fn run<'a>(
        &'a self,
        p: SwitchFocusParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let prev = server.focus.switch_focus(p.chat_id).await?;
            Ok(match (p.chat_id, prev) {
                (Some(cid), Some(prev)) => format!("Focus switched from chat {prev} to {cid}"),
                (Some(cid), None) => format!("Focus set to chat {cid}"),
                (None, Some(prev)) => format!("Focus cleared (was on chat {prev})"),
                (None, None) => "Focus was already clear".to_string(),
            })
        })
    }
}
