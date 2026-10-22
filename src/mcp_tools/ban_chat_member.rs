use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct BanChatMemberTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BanChatMemberParams {
    pub chat_id: i64,
    pub user_id: u64,
}

impl Tool for BanChatMemberTool {
    type Params = BanChatMemberParams;

    fn name(&self) -> &'static str {
        "ban_chat_member"
    }

    fn description(&self) -> &'static str {
        "Ban a user from a chat. Requires bot admin rights."
    }

    fn run<'a>(
        &'a self,
        p: BanChatMemberParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            use teloxide::prelude::*;
            use teloxide::types::{ChatId, UserId};
            let bot = server.bot()?;
            bot.ban_chat_member(ChatId(p.chat_id), UserId(p.user_id))
                .await
                .context("failed to ban user")?;
            Ok(format!("User {} banned from chat {}", p.user_id, p.chat_id))
        })
    }
}
