use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::future::Future;
use std::pin::Pin;

pub struct SendContactTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendContactParams {
    /// Target chat id.
    pub chat_id: i64,
    /// Contact phone number, e.g. "+998901234567".
    pub phone_number: String,
    /// Contact first name.
    pub first_name: String,
    /// Contact last name.
    #[serde(default)]
    pub last_name: Option<String>,
    /// Reply target message id.
    #[serde(default)]
    pub reply_to_message_id: Option<i64>,
    /// Forum topic thread id.
    #[serde(default)]
    pub thread_id: Option<i64>,
}

impl Tool for SendContactTool {
    type Params = SendContactParams;

    fn name(&self) -> &'static str {
        "send_contact"
    }

    fn description(&self) -> &'static str {
        "Send a phone contact card to a chat."
    }

    fn run<'a>(
        &'a self,
        p: SendContactParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            use teloxide::prelude::*;
            use teloxide::types::{ChatId, MessageId, ReplyParameters, ThreadId};
            let bot = server.bot()?;
            let mut req = bot.send_contact(ChatId(p.chat_id), p.phone_number.clone(), p.first_name.clone());
            if let Some(last) = &p.last_name {
                req = req.last_name(last);
            }
            if let Some(tid) = p.thread_id {
                req = req.message_thread_id(ThreadId(MessageId(tid as i32)));
            }
            if let Some(rid) = p.reply_to_message_id {
                req = req.reply_parameters(ReplyParameters::new(MessageId(rid as i32)));
            }
            let sent = req.await.context("failed to send contact")?;
            Ok(serde_json::to_string(&json!({
                "message_id": i64::from(sent.id.0),
                "chat_id": p.chat_id,
            }))?)
        })
    }
}
