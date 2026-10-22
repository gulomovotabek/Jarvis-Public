use crate::mcp::{McpServer, UserError};
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct SheetsClearTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SheetsClearParams {
    /// Google spreadsheet id.
    pub spreadsheet_id: String,
    /// A1-notation range to clear.
    pub range: String,
}

impl Tool for SheetsClearTool {
    type Params = SheetsClearParams;

    fn name(&self) -> &'static str {
        "sheets_clear"
    }

    fn description(&self) -> &'static str {
        "Clear all values from a Sheets range (preserves formatting)."
    }

    fn run<'a>(
        &'a self,
        p: SheetsClearParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let sheets = server.google_sheets.as_ref().ok_or_else(|| {
                anyhow::Error::new(UserError(
                    "Google Sheets not configured (set GOOGLE_SERVICE_ACCOUNT_KEY env var)".into(),
                ))
            })?;
            sheets.clear_range(&p.spreadsheet_id, &p.range).await
        })
    }
}
