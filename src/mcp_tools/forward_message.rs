use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::future::Future;
use std::pin::Pin;

pub struct ForwardMessageTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ForwardMessageParams {
    /// Destination chat id.
    pub chat_id: i64,
    /// Source chat id the message is being forwarded from.
    pub from_chat_id: i64,
    /// Source message id.
    pub message_id: i64,
}

impl Tool for ForwardMessageTool {
    type Params = ForwardMessageParams;

    fn name(&self) -> &'static str {
        "forward_message"
    }

    fn description(&self) -> &'static str {
        "Forward a message to another chat, preserving the original sender header."
    }

    fn run<'a>(
        &'a self,
        p: ForwardMessageParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            use teloxide::prelude::*;
            use teloxide::types::{ChatId, MessageId};
            let bot = server.bot()?;
            let sent = bot
                .forward_message(ChatId(p.chat_id), ChatId(p.from_chat_id), MessageId(p.message_id as i32))
                .await
                .context("failed to forward message")?;
            Ok(serde_json::to_string(&json!({
                "message_id": i64::from(sent.id.0),
                "chat_id": p.chat_id,
            }))?)
        })
    }
}
