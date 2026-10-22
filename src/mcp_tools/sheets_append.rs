use crate::mcp::{McpServer, UserError};
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

pub struct SheetsAppendTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SheetsAppendParams {
    /// Google spreadsheet id.
    pub spreadsheet_id: String,
    /// Range whose last row determines the insertion point.
    pub range: String,
    /// 2D array of rows to append.
    pub values: Value,
    /// Optional `valueInputOption`: `USER_ENTERED` (default) or `RAW`.
    #[serde(default)]
    pub value_input_option: Option<String>,
}

impl Tool for SheetsAppendTool {
    type Params = SheetsAppendParams;

    fn name(&self) -> &'static str {
        "sheets_append"
    }

    fn description(&self) -> &'static str {
        "Append rows after the last populated row in the given range."
    }

    fn run<'a>(
        &'a self,
        p: SheetsAppendParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let sheets = server.google_sheets.as_ref().ok_or_else(|| {
                anyhow::Error::new(UserError(
                    "Google Sheets not configured (set GOOGLE_SERVICE_ACCOUNT_KEY env var)".into(),
                ))
            })?;
            sheets
                .append_rows(&p.spreadsheet_id, &p.range, &p.values, p.value_input_option.as_deref())
                .await
        })
    }
}
