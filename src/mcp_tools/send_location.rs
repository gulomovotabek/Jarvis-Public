use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::future::Future;
use std::pin::Pin;

pub struct SendLocationTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendLocationParams {
    /// Target chat id.
    pub chat_id: i64,
    /// Latitude in degrees.
    pub latitude: f64,
    /// Longitude in degrees.
    pub longitude: f64,
    /// Reply target message id.
    #[serde(default)]
    pub reply_to_message_id: Option<i64>,
    /// Forum topic thread id.
    #[serde(default)]
    pub thread_id: Option<i64>,
}

impl Tool for SendLocationTool {
    type Params = SendLocationParams;

    fn name(&self) -> &'static str {
        "send_location"
    }

    fn description(&self) -> &'static str {
        "Send a map location (latitude/longitude point) to a chat."
    }

    fn run<'a>(
        &'a self,
        p: SendLocationParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            use teloxide::prelude::*;
            use teloxide::types::{ChatId, MessageId, ReplyParameters, ThreadId};
            let bot = server.bot()?;
            let mut req = bot.send_location(ChatId(p.chat_id), p.latitude, p.longitude);
            if let Some(tid) = p.thread_id {
                req = req.message_thread_id(ThreadId(MessageId(tid as i32)));
            }
            if let Some(rid) = p.reply_to_message_id {
                req = req.reply_parameters(ReplyParameters::new(MessageId(rid as i32)));
            }
            let sent = req.await.context("failed to send location")?;
            Ok(serde_json::to_string(&json!({
                "message_id": i64::from(sent.id.0),
                "chat_id": p.chat_id,
            }))?)
        })
    }
}
