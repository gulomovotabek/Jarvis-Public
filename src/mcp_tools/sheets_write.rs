use crate::mcp::{McpServer, UserError};
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

pub struct SheetsWriteTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SheetsWriteParams {
    /// Google spreadsheet id.
    pub spreadsheet_id: String,
    /// A1-notation range to overwrite.
    pub range: String,
    /// 2D array of values (rows of cells).
    pub values: Value,
    /// Optional `valueInputOption`: `USER_ENTERED` (default) or `RAW`.
    #[serde(default)]
    pub value_input_option: Option<String>,
}

impl Tool for SheetsWriteTool {
    type Params = SheetsWriteParams;

    fn name(&self) -> &'static str {
        "sheets_write"
    }

    fn description(&self) -> &'static str {
        "Write values to a Sheets range, overwriting existing data."
    }

    fn run<'a>(
        &'a self,
        p: SheetsWriteParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let sheets = server.google_sheets.as_ref().ok_or_else(|| {
                anyhow::Error::new(UserError(
                    "Google Sheets not configured (set GOOGLE_SERVICE_ACCOUNT_KEY env var)".into(),
                ))
            })?;
            sheets
                .write_range(&p.spreadsheet_id, &p.range, &p.values, p.value_input_option.as_deref())
                .await
        })
    }
}
