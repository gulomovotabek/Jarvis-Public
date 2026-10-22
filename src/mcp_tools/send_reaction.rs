use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct SendReactionTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendReactionParams {
    /// Chat id where the message lives.
    pub chat_id: i64,
    /// Target message id.
    pub message_id: i64,
    /// Emoji to react with (e.g. 👍 🔥 ❤️ 😂).
    pub emoji: String,
}

impl Tool for SendReactionTool {
    type Params = SendReactionParams;

    fn name(&self) -> &'static str {
        "send_reaction"
    }

    fn description(&self) -> &'static str {
        "Set an emoji reaction on a specific message via the Bot API (setMessageReaction)."
    }

    fn run<'a>(
        &'a self,
        p: SendReactionParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            use teloxide::prelude::*;
            use teloxide::types::{ChatId, MessageId, ReactionType};
            let bot = server.bot()?;
            bot.set_message_reaction(ChatId(p.chat_id), MessageId(p.message_id as i32))
                .reaction(vec![ReactionType::Emoji { emoji: p.emoji.clone() }])
                .await
                .context("failed to set reaction")?;
            Ok(format!(
                "Reaction {} set on message {} in chat {}",
                p.emoji, p.message_id, p.chat_id
            ))
        })
    }
}
