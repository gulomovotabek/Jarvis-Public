use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct QueryTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct QueryParams {
    /// SELECT SQL query to execute. AST-validated: no INSERT/UPDATE/DELETE/DROP/ATTACH/PRAGMA.
    pub sql: String,
}

impl Tool for QueryTool {
    type Params = QueryParams;

    fn name(&self) -> &'static str {
        "query"
    }

    fn description(&self) -> &'static str {
        "Execute a read-only SELECT query against the bot's SQLite database. Max 100 rows."
    }

    fn run<'a>(
        &'a self,
        p: QueryParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move { server.db.execute_select_query(&p.sql, 100).await })
    }
}
