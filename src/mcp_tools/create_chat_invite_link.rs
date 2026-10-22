use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::future::Future;
use std::pin::Pin;

pub struct CreateChatInviteLinkTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateChatInviteLinkParams {
    pub chat_id: i64,
    /// Optional friendly name for the invite link.
    #[serde(default)]
    pub name: Option<String>,
    /// Optional Unix timestamp after which the link expires.
    #[serde(default)]
    pub expire_date: Option<i64>,
    /// Optional maximum number of users who can join via this link.
    #[serde(default)]
    pub member_limit: Option<u32>,
}

impl Tool for CreateChatInviteLinkTool {
    type Params = CreateChatInviteLinkParams;

    fn name(&self) -> &'static str {
        "create_chat_invite_link"
    }

    fn description(&self) -> &'static str {
        "Create an invite link for a chat, optionally with expiry and member limit."
    }

    fn run<'a>(
        &'a self,
        p: CreateChatInviteLinkParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            use teloxide::prelude::*;
            use teloxide::types::ChatId;
            let bot = server.bot()?;
            let mut req = bot.create_chat_invite_link(ChatId(p.chat_id));
            if let Some(n) = &p.name {
                req = req.name(n);
            }
            if let Some(exp) = p.expire_date {
                req = req.expire_date(
                    chrono::DateTime::from_timestamp(exp, 0).unwrap_or_default(),
                );
            }
            if let Some(lim) = p.member_limit {
                req = req.member_limit(lim);
            }
            let link = req.await.context("failed to create invite link")?;
            Ok(serde_json::to_string(&json!({
                "invite_link": link.invite_link,
                "chat_id": p.chat_id,
            }))?)
        })
    }
}
