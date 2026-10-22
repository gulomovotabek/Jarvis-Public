use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::future::Future;
use std::pin::Pin;

pub struct SearchMessagesTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchMessagesParams {
    /// Search query (keywords).
    pub query: String,
    /// Maximum number of results to return. Default 10, capped at 50.
    #[serde(default = "default_limit")]
    pub limit: u32,
}

fn default_limit() -> u32 {
    10
}

impl Tool for SearchMessagesTool {
    type Params = SearchMessagesParams;

    fn name(&self) -> &'static str {
        "search_messages"
    }

    fn description(&self) -> &'static str {
        "Search through stored messages in the database by keyword (lexical RAG)."
    }

    fn run<'a>(
        &'a self,
        params: SearchMessagesParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let limit = (params.limit as usize).clamp(1, 50);
            let rag = crate::rag::RagService::new(server.db.clone(), limit, 2000);
            let results = rag.search(&params.query).await?;
            if results.is_empty() {
                return Ok(format!("No messages found matching '{}'", params.query));
            }
            let out: Vec<Value> = results
                .iter()
                .map(|r| {
                    json!({
                        "chat": r.chat_title,
                        "content": r.content,
                        "created_at": r.created_at,
                        "message_link": r.message_link,
                        "score": r.score,
                    })
                })
                .collect();
            Ok(serde_json::to_string_pretty(&out)?)
        })
    }
}
