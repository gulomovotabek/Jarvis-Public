use crate::mcp::{McpServer, UserError};
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct SheetsBatchReadTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SheetsBatchReadParams {
    /// Spreadsheet id.
    pub spreadsheet_id: String,
    /// List of A1-notation ranges to read in one call.
    pub ranges: Vec<String>,
    /// Optional value render option.
    #[serde(default)]
    pub value_render_option: Option<String>,
}

impl Tool for SheetsBatchReadTool {
    type Params = SheetsBatchReadParams;

    fn name(&self) -> &'static str {
        "sheets_batch_read"
    }

    fn description(&self) -> &'static str {
        "Read multiple ranges from a spreadsheet in a single API call."
    }

    fn run<'a>(
        &'a self,
        p: SheetsBatchReadParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let sheets = server.google_sheets.as_ref().ok_or_else(|| {
                anyhow::Error::new(UserError(
                    "Google Sheets not configured (set GOOGLE_SERVICE_ACCOUNT_KEY env var)".into(),
                ))
            })?;
            sheets
                .batch_read(&p.spreadsheet_id, &p.ranges, p.value_render_option.as_deref())
                .await
        })
    }
}
