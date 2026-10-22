//! Outbound SMTP client for sending job application emails.
//!
//! Reuses the same `GMAIL_IMAP_USER` + `GMAIL_APP_PASSWORD` that the `gmail`
//! feature uses for IMAP reads — Gmail app passwords work for both IMAP and
//! SMTP. Override via `SMTP_USER`, `SMTP_PASSWORD`, `SMTP_HOST`, `SMTP_PORT`
//! if you want to send from a different account.
//!
//! Attachments are opened lazily from disk at send time (so e.g. the PDF
//! materialized by `get_resume_pdf` can be attached without round-tripping
//! through base64 in the tool layer).

use anyhow::{Context, Result};
use lettre::message::header::ContentType;
use lettre::message::{Attachment, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

const DEFAULT_HOST: &str = "smtp.gmail.com";
const DEFAULT_PORT: u16 = 587;

#[derive(Clone)]
pub struct SmtpClient {
    from: String,
    transport: AsyncSmtpTransport<Tokio1Executor>,
}

impl SmtpClient {
    /// Build an SMTP client configured for STARTTLS on `host:port`. The
    /// `from` address is the MAIL FROM header and must match the
    /// authenticated user for Gmail.
    pub fn new(user: String, password: String, host: String, port: u16) -> Result<Self> {
        let creds = Credentials::new(user.clone(), password);
        let transport = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&host)
            .context("smtp starttls_relay")?
            .port(port)
            .credentials(creds)
            .build();
        Ok(Self {
            from: user,
            transport,
        })
    }

    /// Convenience constructor for Gmail using the same app password the
    /// IMAP reader already uses.
    pub fn from_gmail(user: String, app_password: String) -> Result<Self> {
        Self::new(user, app_password, DEFAULT_HOST.to_string(), DEFAULT_PORT)
    }

    /// Send a plain-text email, optionally with a single file attachment.
    /// `from_name` is used for the display name in the From header.
    pub async fn send(
        &self,
        from_name: Option<&str>,
        to: &str,
        subject: &str,
        body: &str,
        attachment_path: Option<&str>,
    ) -> Result<String> {
        let from_header = match from_name {
            Some(name) => format!("{name} <{}>", self.from),
            None => self.from.clone(),
        };

        // Build body. If there's an attachment, wrap body + attachment in
        // a mixed multipart envelope.
        let text_part = SinglePart::plain(body.to_string());

        let email = if let Some(path) = attachment_path {
            let data = tokio::fs::read(path)
                .await
                .with_context(|| format!("failed to read attachment {path}"))?;
            let filename = std::path::Path::new(path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("attachment.bin")
                .to_string();
            let content_type = guess_content_type(&filename);
            let attachment_part = Attachment::new(filename)
                .body(data, content_type);

            let multipart = MultiPart::mixed()
                .singlepart(text_part)
                .singlepart(attachment_part);

            Message::builder()
                .from(from_header.parse().context("invalid from address")?)
                .to(to.parse().context("invalid to address")?)
                .subject(subject)
                .multipart(multipart)
                .context("message build error")?
        } else {
            Message::builder()
                .from(from_header.parse().context("invalid from address")?)
                .to(to.parse().context("invalid to address")?)
                .subject(subject)
                .singlepart(text_part)
                .context("message build error")?
        };

        let response = self
            .transport
            .send(email)
            .await
            .context("smtp send failed")?;
        Ok(format!(
            "sent (code={}, message=\"{}\")",
            response.code(),
            response.first_line().unwrap_or("ok")
        ))
    }
}

fn guess_content_type(filename: &str) -> ContentType {
    let lower = filename.to_lowercase();
    let mime_str = if lower.ends_with(".pdf") {
        "application/pdf"
    } else if lower.ends_with(".txt") || lower.ends_with(".md") {
        "text/plain"
    } else if lower.ends_with(".docx") {
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
    } else if lower.ends_with(".doc") {
        "application/msword"
    } else if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else {
        "application/octet-stream"
    };
    ContentType::parse(mime_str).unwrap_or(ContentType::parse("application/octet-stream").unwrap())
}
