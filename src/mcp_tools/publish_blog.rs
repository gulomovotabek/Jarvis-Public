use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::{Context, Result, anyhow};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::future::Future;
use std::pin::Pin;

pub struct PublishBlogTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PublishBlogParams {
    /// Post title, plain text, 100 characters max.
    pub title: String,
    /// Post body as CKEditor-compatible HTML (<h2>, <p>, <ul>/<li>, <strong>,
    /// <em>, <blockquote>, <a>). Do NOT include an <h1> — the title is separate.
    pub content: String,
}

impl Tool for PublishBlogTool {
    type Params = PublishBlogParams;

    fn name(&self) -> &'static str {
        "publish_blog"
    }

    fn description(&self) -> &'static str {
        "Publish a post to the configured blog (BLOG_INTERNAL_URL). Provide title (<=100 chars) and \
         content (HTML body, no <h1>). Returns the new post id and public URL. \
         Only call after the user has approved the draft."
    }

    fn run<'a>(
        &'a self,
        p: PublishBlogParams,
        _server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            if p.title.chars().count() > 100 {
                return Err(anyhow!(
                    "title too long ({} chars, max 100)",
                    p.title.chars().count()
                ));
            }
            let url = std::env::var("BLOG_INTERNAL_URL").unwrap_or_else(|_| {
                "http://127.0.0.1:8001/api/posts/internal/create/".to_string()
            });
            let token = std::env::var("BLOG_POST_TOKEN")
                .map_err(|_| anyhow!("BLOG_POST_TOKEN is not set in the environment"))?;

            let resp = reqwest::Client::new()
                .post(&url)
                .header("X-Post-Token", token)
                .json(&json!({ "title": p.title, "content": p.content }))
                .send()
                .await
                .context("request to blog failed")?;

            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(anyhow!("blog create failed: HTTP {status} {body}"));
            }

            let id = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v.get("id").and_then(|x| x.as_i64()))
                .ok_or_else(|| anyhow!("unexpected blog response: {body}"))?;

            let public_url = std::env::var("BLOG_PUBLIC_URL")
                .map(|base| format!("{}/{id}", base.trim_end_matches('/')))
                .unwrap_or_else(|_| "(BLOG_PUBLIC_URL not set)".to_string());
            Ok(format!("Published: {public_url} (post id {id})"))
        })
    }
}
