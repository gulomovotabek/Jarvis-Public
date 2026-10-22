use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct PinMessageTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PinMessageParams {
    pub chat_id: i64,
    pub message_id: i64,
    /// If true, pin silently without notifying chat members.
    #[serde(default)]
    pub disable_notification: bool,
}

impl Tool for PinMessageTool {
    type Params = PinMessageParams;

    fn name(&self) -> &'static str {
        "pin_message"
    }

    fn description(&self) -> &'static str {
        "Pin a message in a chat."
    }

    fn run<'a>(
        &'a self,
        p: PinMessageParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            use teloxide::prelude::*;
            use teloxide::types::{ChatId, MessageId};
            let bot = server.bot()?;
            let mut req = bot.pin_chat_message(ChatId(p.chat_id), MessageId(p.message_id as i32));
            if p.disable_notification {
                req = req.disable_notification(true);
            }
            req.await.context("failed to pin message")?;
            Ok(format!(
                "Message {} pinned in chat {}",
                p.message_id, p.chat_id
            ))
        })
    }
}
