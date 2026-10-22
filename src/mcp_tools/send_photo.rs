use crate::mcp::{McpServer, make_input_file};
use crate::mcp_tools::Tool;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::future::Future;
use std::pin::Pin;

pub struct SendPhotoTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendPhotoParams {
    /// Target chat id.
    pub chat_id: i64,
    /// URL, file_id, or local absolute path.
    pub photo: String,
    /// Optional caption (rendered as HTML).
    #[serde(default)]
    pub caption: Option<String>,
    /// Reply target message id.
    #[serde(default)]
    pub reply_to_message_id: Option<i64>,
    /// Forum topic thread id.
    #[serde(default)]
    pub thread_id: Option<i64>,
}

impl Tool for SendPhotoTool {
    type Params = SendPhotoParams;

    fn name(&self) -> &'static str {
        "send_photo"
    }

    fn description(&self) -> &'static str {
        "Send a photo to a chat. Accepts URL, Telegram file_id, or local file path."
    }

    fn run<'a>(
        &'a self,
        p: SendPhotoParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            use teloxide::prelude::*;
            use teloxide::types::{ChatId, MessageId, ParseMode, ReplyParameters, ThreadId};
            let bot = server.bot()?;
            let input_file = make_input_file(&p.photo)?;
            let mut req = bot.send_photo(ChatId(p.chat_id), input_file);
            if let Some(cap) = &p.caption {
                req = req.caption(cap).parse_mode(ParseMode::Html);
            }
            if let Some(tid) = p.thread_id {
                req = req.message_thread_id(ThreadId(MessageId(tid as i32)));
            }
            if let Some(rid) = p.reply_to_message_id {
                req = req.reply_parameters(ReplyParameters::new(MessageId(rid as i32)));
            }
            let sent = req.await.context("failed to send photo")?;
            Ok(serde_json::to_string(&json!({
                "message_id": i64::from(sent.id.0),
                "chat_id": p.chat_id,
            }))?)
        })
    }
}
