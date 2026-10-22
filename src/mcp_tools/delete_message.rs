use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct DeleteMessageTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteMessageParams {
    /// Chat id containing the message.
    pub chat_id: i64,
    /// Target message id.
    pub message_id: i64,
}

impl Tool for DeleteMessageTool {
    type Params = DeleteMessageParams;

    fn name(&self) -> &'static str {
        "delete_message"
    }

    fn description(&self) -> &'static str {
        "Delete a message by id."
    }

    fn run<'a>(
        &'a self,
        p: DeleteMessageParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            use teloxide::prelude::*;
            use teloxide::types::{ChatId, MessageId};
            let bot = server.bot()?;
            bot.delete_message(ChatId(p.chat_id), MessageId(p.message_id as i32))
                .await
                .context("failed to delete message")?;
            Ok(format!(
                "Message {} deleted from chat {}",
                p.message_id, p.chat_id
            ))
        })
    }
}
