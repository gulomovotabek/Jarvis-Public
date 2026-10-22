use crate::mcp::{McpServer, make_input_file};
use crate::mcp_tools::Tool;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::future::Future;
use std::pin::Pin;

pub struct SendVideoTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendVideoParams {
    pub chat_id: i64,
    /// URL, file_id, or local absolute path.
    pub video: String,
    #[serde(default)]
    pub caption: Option<String>,
    #[serde(default)]
    pub reply_to_message_id: Option<i64>,
    #[serde(default)]
    pub thread_id: Option<i64>,
}

impl Tool for SendVideoTool {
    type Params = SendVideoParams;

    fn name(&self) -> &'static str {
        "send_video"
    }

    fn description(&self) -> &'static str {
        "Send a video to a chat. Accepts URL, Telegram file_id, or local path."
    }

    fn run<'a>(
        &'a self,
        p: SendVideoParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            use teloxide::prelude::*;
            use teloxide::types::{ChatId, MessageId, ParseMode, ReplyParameters, ThreadId};
            let bot = server.bot()?;
            let input_file = make_input_file(&p.video)?;
            let mut req = bot.send_video(ChatId(p.chat_id), input_file);
            if let Some(cap) = &p.caption {
                req = req.caption(cap).parse_mode(ParseMode::Html);
            }
            if let Some(tid) = p.thread_id {
                req = req.message_thread_id(ThreadId(MessageId(tid as i32)));
            }
            if let Some(rid) = p.reply_to_message_id {
                req = req.reply_parameters(ReplyParameters::new(MessageId(rid as i32)));
            }
            let sent = req.await.context("failed to send video")?;
            Ok(serde_json::to_string(&json!({
                "message_id": i64::from(sent.id.0),
                "chat_id": p.chat_id,
            }))?)
        })
    }
}
