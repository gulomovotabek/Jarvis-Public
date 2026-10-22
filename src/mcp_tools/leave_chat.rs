use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct LeaveChatTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LeaveChatParams {
    pub chat_id: i64,
}

impl Tool for LeaveChatTool {
    type Params = LeaveChatParams;

    fn name(&self) -> &'static str {
        "leave_chat"
    }

    fn description(&self) -> &'static str {
        "Make the bot leave a group or channel."
    }

    fn run<'a>(
        &'a self,
        p: LeaveChatParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            use teloxide::prelude::*;
            use teloxide::types::ChatId;
            let bot = server.bot()?;
            bot.leave_chat(ChatId(p.chat_id))
                .await
                .context("failed to leave chat")?;
            Ok(format!("Left chat {}", p.chat_id))
        })
    }
}
