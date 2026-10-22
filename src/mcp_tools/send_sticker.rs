use crate::mcp::{McpServer, make_input_file};
use crate::mcp_tools::Tool;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::future::Future;
use std::pin::Pin;

pub struct SendStickerTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendStickerParams {
    pub chat_id: i64,
    /// Sticker file_id, URL, or local path.
    pub sticker: String,
    #[serde(default)]
    pub reply_to_message_id: Option<i64>,
    #[serde(default)]
    pub thread_id: Option<i64>,
}

impl Tool for SendStickerTool {
    type Params = SendStickerParams;

    fn name(&self) -> &'static str {
        "send_sticker"
    }

    fn description(&self) -> &'static str {
        "Send a sticker to a chat."
    }

    fn run<'a>(
        &'a self,
        p: SendStickerParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            use teloxide::prelude::*;
            use teloxide::types::{ChatId, MessageId, ReplyParameters, ThreadId};
            let bot = server.bot()?;
            let input_file = make_input_file(&p.sticker)?;
            let mut req = bot.send_sticker(ChatId(p.chat_id), input_file);
            if let Some(tid) = p.thread_id {
                req = req.message_thread_id(ThreadId(MessageId(tid as i32)));
            }
            if let Some(rid) = p.reply_to_message_id {
                req = req.reply_parameters(ReplyParameters::new(MessageId(rid as i32)));
            }
            let sent = req.await.context("failed to send sticker")?;
            Ok(serde_json::to_string(&json!({
                "message_id": i64::from(sent.id.0),
                "chat_id": p.chat_id,
            }))?)
        })
    }
}
