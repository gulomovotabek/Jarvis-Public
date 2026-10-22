use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct SetChatTitleTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetChatTitleParams {
    pub chat_id: i64,
    pub title: String,
}

impl Tool for SetChatTitleTool {
    type Params = SetChatTitleParams;

    fn name(&self) -> &'static str {
        "set_chat_title"
    }

    fn description(&self) -> &'static str {
        "Change a chat title. Requires bot to have admin rights."
    }

    fn run<'a>(
        &'a self,
        p: SetChatTitleParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            use teloxide::prelude::*;
            use teloxide::types::ChatId;
            let bot = server.bot()?;
            bot.set_chat_title(ChatId(p.chat_id), &p.title)
                .await
                .context("failed to set chat title")?;
            Ok(format!("Chat title set to '{}' for chat {}", p.title, p.chat_id))
        })
    }
}
