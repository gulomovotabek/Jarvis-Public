use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct UnpinMessageTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UnpinMessageParams {
    pub chat_id: i64,
    /// If omitted, all pinned messages in the chat are unpinned.
    #[serde(default)]
    pub message_id: Option<i64>,
}

impl Tool for UnpinMessageTool {
    type Params = UnpinMessageParams;

    fn name(&self) -> &'static str {
        "unpin_message"
    }

    fn description(&self) -> &'static str {
        "Unpin a specific message or all messages in a chat."
    }

    fn run<'a>(
        &'a self,
        p: UnpinMessageParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            use teloxide::prelude::*;
            use teloxide::types::{ChatId, MessageId};
            let bot = server.bot()?;
            if let Some(mid) = p.message_id {
                bot.unpin_chat_message(ChatId(p.chat_id))
                    .message_id(MessageId(mid as i32))
                    .await
                    .context("failed to unpin message")?;
                Ok(format!("Message {mid} unpinned in chat {}", p.chat_id))
            } else {
                bot.unpin_all_chat_messages(ChatId(p.chat_id))
                    .await
                    .context("failed to unpin all messages")?;
                Ok(format!("All messages unpinned in chat {}", p.chat_id))
            }
        })
    }
}
