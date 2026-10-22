use crate::mcp::{McpServer, UserError};
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct GmailReadMessageTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GmailReadMessageParams {
    /// IMAP UID of the message (from `gmail_list_recent` or `gmail_search`).
    pub uid: u32,
}

impl Tool for GmailReadMessageTool {
    type Params = GmailReadMessageParams;

    fn name(&self) -> &'static str {
        "gmail_read_message"
    }

    fn description(&self) -> &'static str {
        "Fetch the full body of a specific Gmail message by UID. Returns subject, from, \
         date, and the plain-text body (HTML is stripped). Read-only; does not mark \
         the message as read."
    }

    fn run<'a>(
        &'a self,
        p: GmailReadMessageParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let gmail = server.gmail.as_ref().ok_or_else(|| {
                anyhow::Error::new(UserError(
                    "Gmail not configured — set GMAIL_IMAP_USER + GMAIL_APP_PASSWORD in .env"
                        .into(),
                ))
            })?;
            let msg = gmail.read_message(p.uid).await?;
            Ok(serde_json::to_string_pretty(&msg)?)
        })
    }
}
