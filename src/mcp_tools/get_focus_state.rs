use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct GetFocusStateTool;

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct GetFocusStateParams {}

impl Tool for GetFocusStateTool {
    type Params = GetFocusStateParams;

    fn name(&self) -> &'static str {
        "get_focus_state"
    }

    fn description(&self) -> &'static str {
        "Get the current focus state — which chat the bot is currently focused on, if any."
    }

    fn run<'a>(
        &'a self,
        _p: GetFocusStateParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            Ok(match server.focus.get_focus_state().await? {
                Some((chat_id, at)) => format!("Focused on chat {chat_id} since {at}"),
                None => "No focus set".to_string(),
            })
        })
    }
}
