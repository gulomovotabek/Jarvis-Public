use crate::mcp::{McpServer, UserError};
use crate::mcp_tools::Tool;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::future::Future;
use std::pin::Pin;

pub struct SendPollTool;

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendPollParams {
    pub chat_id: i64,
    pub question: String,
    /// At least 2 answer options.
    pub options: Vec<String>,
    /// Hide voter identities (default: true).
    #[serde(default = "default_true")]
    pub is_anonymous: bool,
    /// Allow multi-select.
    #[serde(default)]
    pub allows_multiple_answers: bool,
    #[serde(default)]
    pub thread_id: Option<i64>,
}

impl Tool for SendPollTool {
    type Params = SendPollParams;

    fn name(&self) -> &'static str {
        "send_poll"
    }

    fn description(&self) -> &'static str {
        "Create a poll in a chat (anonymous/named, single/multi answer)."
    }

    fn run<'a>(
        &'a self,
        p: SendPollParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            use teloxide::prelude::*;
            use teloxide::types::{ChatId, MessageId, ThreadId};
            if p.options.len() < 2 {
                return Err(anyhow::Error::new(UserError(
                    "poll needs at least 2 options".into(),
                )));
            }
            let bot = server.bot()?;
            let mut req = bot
                .send_poll(ChatId(p.chat_id), &p.question, p.options.clone())
                .is_anonymous(p.is_anonymous)
                .allows_multiple_answers(p.allows_multiple_answers);
            if let Some(tid) = p.thread_id {
                req = req.message_thread_id(ThreadId(MessageId(tid as i32)));
            }
            let sent = req.await.context("failed to send poll")?;
            Ok(serde_json::to_string(&json!({
                "message_id": i64::from(sent.id.0),
                "chat_id": p.chat_id,
            }))?)
        })
    }
}
