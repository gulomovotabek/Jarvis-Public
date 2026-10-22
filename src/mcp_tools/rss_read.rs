use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct RssReadTool;

fn default_max_items() -> u32 {
    10
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RssReadParams {
    /// RSS or Atom feed URL, e.g. "https://hnrss.org/frontpage".
    pub url: String,
    /// Maximum items to return (default 10, capped at 30).
    #[serde(default = "default_max_items")]
    pub max_items: u32,
}

fn text_of(t: &Option<feed_rs::model::Text>) -> String {
    t.as_ref().map(|v| v.content.trim().to_string()).unwrap_or_default()
}

impl Tool for RssReadTool {
    type Params = RssReadParams;

    fn name(&self) -> &'static str {
        "rss_read"
    }

    fn description(&self) -> &'static str {
        "Fetch and parse an RSS/Atom feed, return latest items (title, link, date, summary). Reliable news source — no bot-detection issues."
    }

    fn run<'a>(
        &'a self,
        p: RssReadParams,
        _server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let max = (p.max_items as usize).clamp(1, 30);
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .user_agent("jarvis-rss/1.0")
                .build()?;
            let bytes = client
                .get(&p.url)
                .send()
                .await
                .context("feed request failed")?
                .error_for_status()
                .context("feed returned error status")?
                .bytes()
                .await
                .context("failed to read feed body")?;
            let feed = feed_rs::parser::parse(&bytes[..]).context("failed to parse feed")?;

            let feed_title = text_of(&feed.title);
            let mut out = if feed_title.is_empty() {
                String::from("Feed")
            } else {
                feed_title
            };
            out.push_str(&format!(" — {} items:\n", feed.entries.len().min(max)));

            for entry in feed.entries.iter().take(max) {
                let title = text_of(&entry.title);
                let link = entry.links.first().map(|l| l.href.as_str()).unwrap_or("");
                let date = entry
                    .published
                    .or(entry.updated)
                    .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_default();
                let mut summary = text_of(&entry.summary);
                summary = crate::mcp::strip_html_tags(&summary);
                if summary.chars().count() > 300 {
                    summary = summary.chars().take(300).collect::<String>() + "…";
                }
                out.push_str(&format!("\n• {title}\n  {link}"));
                if !date.is_empty() {
                    out.push_str(&format!("\n  {date}"));
                }
                if !summary.is_empty() {
                    out.push_str(&format!("\n  {summary}"));
                }
                out.push('\n');
            }
            Ok(out)
        })
    }
}
