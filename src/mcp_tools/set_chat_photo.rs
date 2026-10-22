use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct SetChatPhotoTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetChatPhotoParams {
    pub chat_id: i64,
    /// URL of the image to use as the chat photo.
    pub photo: String,
}

impl Tool for SetChatPhotoTool {
    type Params = SetChatPhotoParams;

    fn name(&self) -> &'static str {
        "set_chat_photo"
    }

    fn description(&self) -> &'static str {
        "Set a chat profile photo from an image URL. Requires bot admin rights."
    }

    fn run<'a>(
        &'a self,
        p: SetChatPhotoParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            use teloxide::prelude::*;
            use teloxide::types::{ChatId, InputFile};

            // Telegram's setChatPhoto only accepts multipart file upload.
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()?;
            let resp = client
                .get(&p.photo)
                .send()
                .await
                .context("failed to download photo")?;
            let bytes = resp.bytes().await.context("failed to read photo bytes")?;

            let bot = server.bot()?;
            bot.set_chat_photo(
                ChatId(p.chat_id),
                InputFile::memory(bytes).file_name("photo.jpg"),
            )
            .await
            .context("failed to set chat photo")?;
            Ok(format!("Chat photo updated for chat {}", p.chat_id))
        })
    }
}
