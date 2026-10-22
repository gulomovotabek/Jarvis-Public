use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::future::Future;
use std::pin::Pin;

pub struct BotSendMessageTool;

fn default_parse_mode() -> String {
    "markdown".to_string()
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ChatTarget {
    /// Numeric chat id.
    Id(i64),
    /// Public `@username` for a channel or group.
    Username(String),
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BotSendMessageParams {
    /// Target chat: numeric id, or public `@username` for a channel.
    pub chat_id: ChatTarget,
    /// Message body.
    pub text: String,
    /// Forum topic thread id.
    #[serde(default)]
    pub thread_id: Option<i64>,
    /// Telegram message id to reply to.
    #[serde(default)]
    pub reply_to_message_id: Option<i64>,
    /// `markdown` (default — converted to HTML), `html`, or `plain`.
    #[serde(default = "default_parse_mode")]
    pub parse_mode: String,
}

impl Tool for BotSendMessageTool {
    type Params = BotSendMessageParams;

    fn name(&self) -> &'static str {
        "bot_send_message"
    }

    fn description(&self) -> &'static str {
        "Send a reply via the Bot API. Primary way to deliver responses. For long text split into multiple calls (~4096 char limit). After sending, return [BOT_SENT] as your final text."
    }

    fn run<'a>(
        &'a self,
        p: BotSendMessageParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            use crate::formatting::markdown_like_to_telegram_html;
            use teloxide::prelude::*;
            use teloxide::types::{ChatId, MessageId, ParseMode, Recipient, ReplyParameters, ThreadId};

            let mode = p.parse_mode.to_lowercase();
            let rendered = match mode.as_str() {
                "html" | "plain" => p.text.clone(),
                _ => markdown_like_to_telegram_html(&p.text),
            };

            let recipient = match &p.chat_id {
                ChatTarget::Id(id) => Recipient::Id(ChatId(*id)),
                ChatTarget::Username(u) => Recipient::ChannelUsername(u.clone()),
            };
            let bot = server.bot()?;
            let mut req = bot.send_message(recipient, &rendered);
            if mode != "plain" {
                req = req.parse_mode(ParseMode::Html);
            }
            if let Some(tid) = p.thread_id {
                req = req.message_thread_id(ThreadId(MessageId(tid as i32)));
            }
            if let Some(rid) = p.reply_to_message_id {
                req = req.reply_parameters(ReplyParameters::new(MessageId(rid as i32)));
            }

            let sent = req.await.context("failed to send message via Bot API")?;
            let sent_msg_id = i64::from(sent.id.0);
            let resolved_chat_id = sent.chat.id.0;

            // Store in DB for RAG.
            if let Some(bot_uid) = server.bot_user_id {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or_default();
                let _ = server
                    .db
                    .insert_or_update_message(&crate::db::StoredMessageInput {
                        chat_id: resolved_chat_id,
                        telegram_message_id: sent_msg_id,
                        sender_id: Some(bot_uid),
                        content: Some(p.text.clone()),
                        raw_json: "{}".to_string(),
                        created_at: now,
                        ingested_at: now,
                        reply_to_message_id: p.reply_to_message_id,
                        message_link: None,
                    })
                    .await;
            }

            Ok(serde_json::to_string(
                &json!({ "message_id": sent_msg_id, "chat_id": resolved_chat_id }),
            )?)
        })
    }
}
