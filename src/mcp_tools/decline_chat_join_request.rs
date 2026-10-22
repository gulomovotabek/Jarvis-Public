use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct DeclineChatJoinRequestTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeclineChatJoinRequestParams {
    pub chat_id: i64,
    pub user_id: u64,
}

impl Tool for DeclineChatJoinRequestTool {
    type Params = DeclineChatJoinRequestParams;

    fn name(&self) -> &'static str {
        "decline_chat_join_request"
    }

    fn description(&self) -> &'static str {
        "Decline a pending chat join request."
    }

    fn run<'a>(
        &'a self,
        p: DeclineChatJoinRequestParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            use teloxide::prelude::*;
            use teloxide::types::{ChatId, UserId};
            let bot = server.bot()?;
            bot.decline_chat_join_request(ChatId(p.chat_id), UserId(p.user_id))
                .await
                .context("failed to decline join request")?;
            Ok(format!(
                "Join request declined for user {} in chat {}",
                p.user_id, p.chat_id
            ))
        })
    }
}
