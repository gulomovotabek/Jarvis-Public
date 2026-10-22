use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::future::Future;
use std::pin::Pin;

pub struct GetChatMemberCountTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetChatMemberCountParams {
    pub chat_id: i64,
}

impl Tool for GetChatMemberCountTool {
    type Params = GetChatMemberCountParams;

    fn name(&self) -> &'static str {
        "get_chat_member_count"
    }

    fn description(&self) -> &'static str {
        "Get the number of members in a chat."
    }

    fn run<'a>(
        &'a self,
        p: GetChatMemberCountParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            use teloxide::prelude::*;
            use teloxide::types::ChatId;
            let bot = server.bot()?;
            let count = bot
                .get_chat_member_count(ChatId(p.chat_id))
                .await
                .context("failed to get member count")?;
            Ok(serde_json::to_string(&json!({
                "chat_id": p.chat_id,
                "member_count": count,
            }))?)
        })
    }
}
