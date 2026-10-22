//! Minimal Gmail IMAP client for reading the inbox.
//!
//! Uses the sync `imap` crate under `spawn_blocking` — IMAP reads are not
//! latency-sensitive (poll cadence is minutes), so avoiding an async TLS
//! dep is cheaper than hand-rolling native-tls integration.
//!
//! Auth is app-password only:
//! 1. Enable 2FA on the Google account
//! 2. Create an app password at https://myaccount.google.com/apppasswords
//! 3. Set `GMAIL_IMAP_USER` + `GMAIL_APP_PASSWORD` in `.env`
//!
//! The client is read-only by design: it opens the mailbox with `examine()`
//! so there's no risk of accidentally marking messages read or deleting them.

use anyhow::{Context, Result};
use serde::Serialize;

const DEFAULT_HOST: &str = "imap.gmail.com";
const DEFAULT_PORT: u16 = 993;
const DEFAULT_MAILBOX: &str = "INBOX";

#[derive(Clone)]
pub struct GmailClient {
    user: String,
    password: String,
    host: String,
    port: u16,
}

/// One message summary or full body from Gmail.
#[derive(Debug, Clone, Serialize)]
pub struct GmailMessage {
    /// IMAP UID — stable across sessions, use this to read/reference later.
    pub uid: u32,
    pub subject: Option<String>,
    pub from: Option<String>,
    pub date: Option<String>,
    /// Plain-text body if present in the message. HTML bodies are stripped
    /// of tags and collapsed to text by `read_message`.
    pub body: Option<String>,
}

impl GmailClient {
    pub fn new(user: String, password: String) -> Self {
        Self {
            user,
            password,
            host: DEFAULT_HOST.to_string(),
            port: DEFAULT_PORT,
        }
    }

    pub fn with_host(mut self, host: String, port: u16) -> Self {
        self.host = host;
        self.port = port;
        self
    }

    /// List the N most-recent messages in INBOX as summaries (UID, subject,
    /// from, date). Body is not fetched — use [`read_message`](Self::read_message).
    pub async fn list_recent(&self, limit: usize) -> Result<Vec<GmailMessage>> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || this.list_recent_blocking(limit))
            .await
            .context("gmail list_recent task panicked")?
    }

    /// Search INBOX using Gmail's X-GM-RAW (Gmail search syntax) if the query
    /// starts with `gmail:`, otherwise a plain IMAP `SUBJECT` search.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<GmailMessage>> {
        let this = self.clone();
        let query = query.to_string();
        tokio::task::spawn_blocking(move || this.search_blocking(&query, limit))
            .await
            .context("gmail search task panicked")?
    }

    /// Fetch full RFC822 body for a specific UID. Returns the parsed message
    /// with body text (stripped of html tags).
    pub async fn read_message(&self, uid: u32) -> Result<GmailMessage> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || this.read_message_blocking(uid))
            .await
            .context("gmail read_message task panicked")?
    }

    // ── blocking implementations ──

    fn connect(&self) -> Result<imap::Session<native_tls::TlsStream<std::net::TcpStream>>> {
        let tls = native_tls::TlsConnector::builder()
            .build()
            .context("failed to build TLS connector")?;
        let client = imap::connect((self.host.as_str(), self.port), &self.host, &tls)
            .context("failed to connect to imap server")?;
        let session = client
            .login(&self.user, &self.password)
            .map_err(|(e, _)| anyhow::anyhow!("imap login failed: {e}"))?;
        Ok(session)
    }

    fn list_recent_blocking(&self, limit: usize) -> Result<Vec<GmailMessage>> {
        let mut session = self.connect()?;
        let mailbox = session
            .examine(DEFAULT_MAILBOX)
            .context("failed to select inbox")?;
        let total = mailbox.exists;
        if total == 0 {
            let _ = session.logout();
            return Ok(vec![]);
        }
        let lim = limit.max(1) as u32;
        let from = total.saturating_sub(lim - 1).max(1);
        let range = format!("{from}:{total}");
        let fetches = session
            .fetch(&range, "(UID ENVELOPE INTERNALDATE)")
            .context("imap fetch failed")?;

        let mut out = Vec::new();
        for f in fetches.iter() {
            let uid = match f.uid {
                Some(u) => u,
                None => continue,
            };
            let (subject, from_addr, date) = if let Some(env) = f.envelope() {
                let subject = env
                    .subject
                    .as_ref()
                    .map(|b| String::from_utf8_lossy(b).to_string());
                let from_addr = env.from.as_ref().and_then(|fs| {
                    fs.first().map(|a| {
                        let mailbox = a
                            .mailbox
                            .as_ref()
                            .map(|m| String::from_utf8_lossy(m).to_string())
                            .unwrap_or_default();
                        let host = a
                            .host
                            .as_ref()
                            .map(|h| String::from_utf8_lossy(h).to_string())
                            .unwrap_or_default();
                        let name = a
                            .name
                            .as_ref()
                            .map(|n| String::from_utf8_lossy(n).to_string());
                        match name {
                            Some(n) if !n.is_empty() => format!("{n} <{mailbox}@{host}>"),
                            _ => format!("{mailbox}@{host}"),
                        }
                    })
                });
                let date = env
                    .date
                    .as_ref()
                    .map(|b| String::from_utf8_lossy(b).to_string());
                (subject, from_addr, date)
            } else {
                (None, None, None)
            };
            out.push(GmailMessage {
                uid,
                subject,
                from: from_addr,
                date,
                body: None,
            });
        }
        // Sort newest first — IMAP returned oldest→newest in the range.
        out.sort_by(|a, b| b.uid.cmp(&a.uid));
        let _ = session.logout();
        Ok(out)
    }

    fn search_blocking(&self, query: &str, limit: usize) -> Result<Vec<GmailMessage>> {
        let mut session = self.connect()?;
        session.examine(DEFAULT_MAILBOX).context("select inbox")?;

        // Build search criteria. `gmail:` prefix → Gmail-native X-GM-RAW.
        let criteria = if let Some(stripped) = query.strip_prefix("gmail:") {
            format!("X-GM-RAW \"{}\"", stripped.replace('"', "\\\""))
        } else {
            // Default to substring search on SUBJECT + BODY. IMAP `OR` takes
            // two keys at a time.
            format!(
                "OR SUBJECT \"{}\" BODY \"{}\"",
                query.replace('"', "\\\""),
                query.replace('"', "\\\"")
            )
        };

        let uids = session
            .uid_search(&criteria)
            .context("imap search failed")?;
        if uids.is_empty() {
            let _ = session.logout();
            return Ok(vec![]);
        }

        // Take the newest `limit` uids.
        let mut sorted: Vec<u32> = uids.into_iter().collect();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        sorted.truncate(limit.max(1));

        let range = sorted
            .iter()
            .map(|u| u.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let fetches = session
            .uid_fetch(&range, "(UID ENVELOPE INTERNALDATE)")
            .context("uid_fetch failed")?;

        let mut out = Vec::new();
        for f in fetches.iter() {
            let uid = match f.uid {
                Some(u) => u,
                None => continue,
            };
            let mut msg = GmailMessage {
                uid,
                subject: None,
                from: None,
                date: None,
                body: None,
            };
            if let Some(env) = f.envelope() {
                msg.subject = env.subject.as_ref().map(|b| String::from_utf8_lossy(b).to_string());
                msg.from = env.from.as_ref().and_then(|fs| {
                    fs.first().map(|a| {
                        let mailbox = a
                            .mailbox
                            .as_ref()
                            .map(|m| String::from_utf8_lossy(m).to_string())
                            .unwrap_or_default();
                        let host = a
                            .host
                            .as_ref()
                            .map(|h| String::from_utf8_lossy(h).to_string())
                            .unwrap_or_default();
                        format!("{mailbox}@{host}")
                    })
                });
                msg.date = env
                    .date
                    .as_ref()
                    .map(|b| String::from_utf8_lossy(b).to_string());
            }
            out.push(msg);
        }
        out.sort_by(|a, b| b.uid.cmp(&a.uid));
        let _ = session.logout();
        Ok(out)
    }

    fn read_message_blocking(&self, uid: u32) -> Result<GmailMessage> {
        let mut session = self.connect()?;
        session.examine(DEFAULT_MAILBOX).context("select inbox")?;

        let fetches = session
            .uid_fetch(uid.to_string(), "(UID ENVELOPE BODY[TEXT])")
            .context("uid_fetch failed")?;

        let fetch = fetches
            .iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no message with uid {uid}"))?;

        let mut msg = GmailMessage {
            uid,
            subject: None,
            from: None,
            date: None,
            body: None,
        };

        if let Some(env) = fetch.envelope() {
            msg.subject = env
                .subject
                .as_ref()
                .map(|b| String::from_utf8_lossy(b).to_string());
            msg.from = env.from.as_ref().and_then(|fs| {
                fs.first().map(|a| {
                    let mailbox = a
                        .mailbox
                        .as_ref()
                        .map(|m| String::from_utf8_lossy(m).to_string())
                        .unwrap_or_default();
                    let host = a
                        .host
                        .as_ref()
                        .map(|h| String::from_utf8_lossy(h).to_string())
                        .unwrap_or_default();
                    format!("{mailbox}@{host}")
                })
            });
            msg.date = env
                .date
                .as_ref()
                .map(|b| String::from_utf8_lossy(b).to_string());
        }

        if let Some(text) = fetch.text() {
            let raw = String::from_utf8_lossy(text).to_string();
            msg.body = Some(clean_body(&raw, 8_000));
        }

        let _ = session.logout();
        Ok(msg)
    }
}

/// Decode quoted-printable-ish text heuristically, strip HTML tags, collapse
/// whitespace, truncate. Good enough for application emails.
fn clean_body(input: &str, max: usize) -> String {
    // Drop any HTML tags and collapse whitespace.
    let mut out = String::with_capacity(input.len().min(max));
    let mut in_tag = false;
    let mut last_was_space = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                if !last_was_space {
                    out.push(' ');
                    last_was_space = true;
                }
            }
            c if !in_tag => {
                if c.is_whitespace() {
                    if !last_was_space {
                        out.push(' ');
                        last_was_space = true;
                    }
                } else {
                    out.push(c);
                    last_was_space = false;
                }
                if out.chars().count() >= max {
                    out.push('…');
                    break;
                }
            }
            _ => {}
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_body_strips_html() {
        assert_eq!(
            clean_body("<p>hi <b>there</b></p>", 100),
            "hi there"
        );
    }

    #[test]
    fn clean_body_truncates() {
        let s = "a".repeat(10_000);
        let out = clean_body(&s, 500);
        assert!(out.chars().count() <= 501);
    }
}
