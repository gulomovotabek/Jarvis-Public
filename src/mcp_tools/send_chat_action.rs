use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct SendChatActionTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendChatActionParams {
    pub chat_id: i64,
    /// One of: typing, upload_photo, record_video, upload_video, record_voice,
    /// upload_voice, upload_document, find_location, record_video_note, upload_video_note.
    pub action: String,
}

impl Tool for SendChatActionTool {
    type Params = SendChatActionParams;

    fn name(&self) -> &'static str {
        "send_chat_action"
    }

    fn description(&self) -> &'static str {
        "Send a typing/uploading indicator to a chat."
    }

    fn run<'a>(
        &'a self,
        p: SendChatActionParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            use teloxide::prelude::*;
            use teloxide::types::{ChatAction, ChatId};
            let chat_action = match p.action.as_str() {
                "typing" => ChatAction::Typing,
                "upload_photo" => ChatAction::UploadPhoto,
                "record_video" => ChatAction::RecordVideo,
                "upload_video" => ChatAction::UploadVideo,
                "record_voice" => ChatAction::RecordVoice,
                "upload_voice" => ChatAction::UploadVoice,
                "upload_document" => ChatAction::UploadDocument,
                "find_location" => ChatAction::FindLocation,
                "record_video_note" => ChatAction::RecordVideoNote,
                "upload_video_note" => ChatAction::UploadVideoNote,
                _ => ChatAction::Typing,
            };
            let bot = server.bot()?;
            bot.send_chat_action(ChatId(p.chat_id), chat_action)
                .await
                .context("failed to send chat action")?;
            Ok(format!("Chat action '{}' sent to chat {}", p.action, p.chat_id))
        })
    }
}
