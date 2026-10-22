use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct SetChatDescriptionTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetChatDescriptionParams {
    pub chat_id: i64,
    pub description: String,
}

impl Tool for SetChatDescriptionTool {
    type Params = SetChatDescriptionParams;

    fn name(&self) -> &'static str {
        "set_chat_description"
    }

    fn description(&self) -> &'static str {
        "Change a chat description. Requires bot to have admin rights."
    }

    fn run<'a>(
        &'a self,
        p: SetChatDescriptionParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            use teloxide::prelude::*;
            use teloxide::types::ChatId;
            let bot = server.bot()?;
            bot.set_chat_description(ChatId(p.chat_id))
                .description(&p.description)
                .await
                .context("failed to set chat description")?;
            Ok(format!("Chat description updated for chat {}", p.chat_id))
        })
    }
}
