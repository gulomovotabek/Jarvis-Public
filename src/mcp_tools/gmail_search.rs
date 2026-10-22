use crate::mcp::{McpServer, UserError};
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct GmailSearchTool;

fn default_limit() -> u32 {
    20
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GmailSearchParams {
    /// Search query. Two forms supported:
    /// - Plain substring (e.g. `"interview"`) — searches subject + body
    /// - `gmail:` prefix (e.g. `"gmail:from:recruiter@ newer_than:7d"`) — Gmail's
    ///   native search syntax via `X-GM-RAW`
    pub query: String,
    /// Max messages to return.
    #[serde(default = "default_limit")]
    pub limit: u32,
}

impl Tool for GmailSearchTool {
    type Params = GmailSearchParams;

    fn name(&self) -> &'static str {
        "gmail_search"
    }

    fn description(&self) -> &'static str {
        "Search Gmail INBOX. Plain queries match subject/body; prefix with `gmail:` to \
         use Gmail's native search syntax (from:, subject:, newer_than:, has:attachment, \
         etc). Use this to find all messages related to a specific application or to \
         detect rejection/interview emails in the last N days."
    }

    fn run<'a>(
        &'a self,
        p: GmailSearchParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let gmail = server.gmail.as_ref().ok_or_else(|| {
                anyhow::Error::new(UserError(
                    "Gmail not configured — set GMAIL_IMAP_USER + GMAIL_APP_PASSWORD in .env"
                        .into(),
                ))
            })?;
            let msgs = gmail.search(&p.query, p.limit as usize).await?;
            if msgs.is_empty() {
                return Ok(format!("No messages matching: {}", p.query));
            }
            Ok(serde_json::to_string_pretty(&msgs)?)
        })
    }
}
