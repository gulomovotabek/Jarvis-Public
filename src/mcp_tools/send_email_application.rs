use crate::mcp::{McpServer, UserError};
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct SendEmailApplicationTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendEmailApplicationParams {
    /// Recipient email address (the company's jobs alias, recruiter, etc.).
    pub to: String,
    /// Email subject line — e.g. "Application: Senior Rust Engineer".
    pub subject: String,
    /// Plain-text email body (the cover letter). Keep it concise; most
    /// recruiters don't read past 300 words.
    pub body: String,
    /// Optional attachment path (e.g. a PDF). Any file under
    /// `/tmp/jarvis_downloads/` works.
    #[serde(default)]
    pub attachment_path: Option<String>,
    /// Display name in the From header (e.g. "Jane Doe"). Falls back
    /// to the raw SMTP user if omitted.
    #[serde(default)]
    pub from_name: Option<String>,
}

impl Tool for SendEmailApplicationTool {
    type Params = SendEmailApplicationParams;

    fn name(&self) -> &'static str {
        "send_email_application"
    }

    fn description(&self) -> &'static str {
        "Send an email (plain-text body + optional file attachment) via the configured \
         SMTP account. Defaults to the same Gmail app-password used by IMAP. To attach \
         a file, download it first (e.g. via download_file) and pass the local path as \
         attachment_path."
    }

    fn run<'a>(
        &'a self,
        p: SendEmailApplicationParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let smtp = server.smtp.as_ref().ok_or_else(|| {
                anyhow::Error::new(UserError(
                    "SMTP not configured — set SMTP_USER/SMTP_PASSWORD or reuse \
                     GMAIL_IMAP_USER/GMAIL_APP_PASSWORD in .env"
                        .into(),
                ))
            })?;

            let result = smtp
                .send(
                    p.from_name.as_deref(),
                    &p.to,
                    &p.subject,
                    &p.body,
                    p.attachment_path.as_deref(),
                )
                .await?;

            Ok(format!("Email sent to {}: {result}", p.to))
        })
    }
}
