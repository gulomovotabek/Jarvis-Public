use crate::mcp::{McpServer, UserError};
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

pub struct SheetsBatchWriteTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SheetsBatchWriteParams {
    /// Spreadsheet id.
    pub spreadsheet_id: String,
    /// Array of `ValueRange` objects: each `{ range, values }`.
    pub data: Value,
    /// Optional `valueInputOption`: `USER_ENTERED` (default) or `RAW`.
    #[serde(default)]
    pub value_input_option: Option<String>,
}

impl Tool for SheetsBatchWriteTool {
    type Params = SheetsBatchWriteParams;

    fn name(&self) -> &'static str {
        "sheets_batch_write"
    }

    fn description(&self) -> &'static str {
        "Write to multiple ranges of a spreadsheet in a single API call."
    }

    fn run<'a>(
        &'a self,
        p: SheetsBatchWriteParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let sheets = server.google_sheets.as_ref().ok_or_else(|| {
                anyhow::Error::new(UserError(
                    "Google Sheets not configured (set GOOGLE_SERVICE_ACCOUNT_KEY env var)".into(),
                ))
            })?;
            sheets
                .batch_write(&p.spreadsheet_id, &p.data, p.value_input_option.as_deref())
                .await
        })
    }
}
