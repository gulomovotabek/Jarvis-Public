use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct UnbanChatMemberTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UnbanChatMemberParams {
    pub chat_id: i64,
    pub user_id: u64,
}

impl Tool for UnbanChatMemberTool {
    type Params = UnbanChatMemberParams;

    fn name(&self) -> &'static str {
        "unban_chat_member"
    }

    fn description(&self) -> &'static str {
        "Unban a previously banned user. Only affects currently banned users."
    }

    fn run<'a>(
        &'a self,
        p: UnbanChatMemberParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            use teloxide::prelude::*;
            use teloxide::types::{ChatId, UserId};
            let bot = server.bot()?;
            bot.unban_chat_member(ChatId(p.chat_id), UserId(p.user_id))
                .only_if_banned(true)
                .await
                .context("failed to unban user")?;
            Ok(format!("User {} unbanned from chat {}", p.user_id, p.chat_id))
        })
    }
}
