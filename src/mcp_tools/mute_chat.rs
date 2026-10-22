use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct MuteChatTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MuteChatParams {
    /// Chat id to mute.
    pub chat_id: i64,
    /// Optional Unix timestamp after which the mute auto-expires.
    #[serde(default)]
    pub until: Option<i64>,
}

impl Tool for MuteChatTool {
    type Params = MuteChatParams;

    fn name(&self) -> &'static str {
        "mute_chat"
    }

    fn description(&self) -> &'static str {
        "Mute a chat so the bot no longer responds. Optional auto-unmute via `until` timestamp."
    }

    fn run<'a>(
        &'a self,
        p: MuteChatParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            server.db.mute_chat(p.chat_id, p.until).await?;
            Ok(format!("Chat {} muted", p.chat_id))
        })
    }
}
