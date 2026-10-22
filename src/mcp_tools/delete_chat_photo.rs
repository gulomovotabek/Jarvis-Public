use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct DeleteChatPhotoTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteChatPhotoParams {
    pub chat_id: i64,
}

impl Tool for DeleteChatPhotoTool {
    type Params = DeleteChatPhotoParams;

    fn name(&self) -> &'static str {
        "delete_chat_photo"
    }

    fn description(&self) -> &'static str {
        "Delete a chat profile photo. Requires bot admin rights."
    }

    fn run<'a>(
        &'a self,
        p: DeleteChatPhotoParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            use teloxide::prelude::*;
            use teloxide::types::ChatId;
            let bot = server.bot()?;
            bot.delete_chat_photo(ChatId(p.chat_id))
                .await
                .context("failed to delete chat photo")?;
            Ok(format!("Chat photo deleted for chat {}", p.chat_id))
        })
    }
}
