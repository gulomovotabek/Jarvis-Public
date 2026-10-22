use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct UnmuteChatTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UnmuteChatParams {
    /// Chat id to unmute.
    pub chat_id: i64,
}

impl Tool for UnmuteChatTool {
    type Params = UnmuteChatParams;

    fn name(&self) -> &'static str {
        "unmute_chat"
    }

    fn description(&self) -> &'static str {
        "Unmute a previously muted chat."
    }

    fn run<'a>(
        &'a self,
        p: UnmuteChatParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            Ok(if server.db.unmute_chat(p.chat_id).await? {
                format!("Chat {} unmuted", p.chat_id)
            } else {
                format!("Chat {} was not muted", p.chat_id)
            })
        })
    }
}
