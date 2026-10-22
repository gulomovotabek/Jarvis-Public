use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct BotEditMessageTool;

fn default_parse_mode() -> String {
    "markdown".to_string()
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BotEditMessageParams {
    /// Chat id containing the message.
    pub chat_id: i64,
    /// Target message id.
    pub message_id: i64,
    /// New text body.
    pub text: String,
    /// `markdown` (default), `html`, or `plain`.
    #[serde(default = "default_parse_mode")]
    pub parse_mode: String,
}

impl Tool for BotEditMessageTool {
    type Params = BotEditMessageParams;

    fn name(&self) -> &'static str {
        "bot_edit_message"
    }

    fn description(&self) -> &'static str {
        "Edit the text of a previously sent bot message."
    }

    fn run<'a>(
        &'a self,
        p: BotEditMessageParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            use crate::formatting::markdown_like_to_telegram_html;
            use teloxide::prelude::*;
            use teloxide::types::{ChatId, MessageId, ParseMode};

            let mode = p.parse_mode.to_lowercase();
            let rendered = match mode.as_str() {
                "html" | "plain" => p.text.clone(),
                _ => markdown_like_to_telegram_html(&p.text),
            };

            let bot = server.bot()?;
            let mut req = bot.edit_message_text(
                ChatId(p.chat_id),
                MessageId(p.message_id as i32),
                &rendered,
            );
            if mode != "plain" {
                req = req.parse_mode(ParseMode::Html);
            }
            req.await.context("failed to edit message via Bot API")?;
            Ok(format!(
                "Message {} edited in chat {}",
                p.message_id, p.chat_id
            ))
        })
    }
}
