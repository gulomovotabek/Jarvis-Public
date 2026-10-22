use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::future::Future;
use std::pin::Pin;

pub struct CopyMessageTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CopyMessageParams {
    /// Destination chat id.
    pub chat_id: i64,
    /// Source chat id.
    pub from_chat_id: i64,
    /// Source message id.
    pub message_id: i64,
}

impl Tool for CopyMessageTool {
    type Params = CopyMessageParams;

    fn name(&self) -> &'static str {
        "copy_message"
    }

    fn description(&self) -> &'static str {
        "Copy a message to another chat without the \"Forwarded from\" header."
    }

    fn run<'a>(
        &'a self,
        p: CopyMessageParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            use teloxide::prelude::*;
            use teloxide::types::{ChatId, MessageId};
            let bot = server.bot()?;
            let result = bot
                .copy_message(ChatId(p.chat_id), ChatId(p.from_chat_id), MessageId(p.message_id as i32))
                .await
                .context("failed to copy message")?;
            Ok(serde_json::to_string(&json!({
                "message_id": result.0,
                "chat_id": p.chat_id,
            }))?)
        })
    }
}
