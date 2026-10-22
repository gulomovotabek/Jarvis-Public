use crate::mcp::{McpServer, UserError};
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct GmailListRecentTool;

fn default_limit() -> u32 {
    20
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GmailListRecentParams {
    /// Maximum messages to return (newest first).
    #[serde(default = "default_limit")]
    pub limit: u32,
}

impl Tool for GmailListRecentTool {
    type Params = GmailListRecentParams;

    fn name(&self) -> &'static str {
        "gmail_list_recent"
    }

    fn description(&self) -> &'static str {
        "List the most recent Gmail inbox messages as summaries (uid, subject, from, \
         date). Read-only — does not mark anything read. Use for a quick daily inbox \
         triage."
    }

    fn run<'a>(
        &'a self,
        p: GmailListRecentParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let gmail = server.gmail.as_ref().ok_or_else(|| {
                anyhow::Error::new(UserError(
                    "Gmail not configured — set GMAIL_IMAP_USER + GMAIL_APP_PASSWORD in .env"
                        .into(),
                ))
            })?;
            let msgs = gmail.list_recent(p.limit as usize).await?;
            if msgs.is_empty() {
                return Ok("No messages in inbox.".to_string());
            }
            Ok(serde_json::to_string_pretty(&msgs)?)
        })
    }
}
