use crate::mcp::{McpServer, UserError};
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct SheetsDeleteSheetTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SheetsDeleteSheetParams {
    /// Spreadsheet id containing the sheet.
    pub spreadsheet_id: String,
    /// Numeric sheet id (not the title) — use sheets_info to look it up.
    pub sheet_id: u64,
}

impl Tool for SheetsDeleteSheetTool {
    type Params = SheetsDeleteSheetParams;

    fn name(&self) -> &'static str {
        "sheets_delete_sheet"
    }

    fn description(&self) -> &'static str {
        "Delete a sheet/tab by its numeric id."
    }

    fn run<'a>(
        &'a self,
        p: SheetsDeleteSheetParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let sheets = server.google_sheets.as_ref().ok_or_else(|| {
                anyhow::Error::new(UserError(
                    "Google Sheets not configured (set GOOGLE_SERVICE_ACCOUNT_KEY env var)".into(),
                ))
            })?;
            sheets.delete_sheet(&p.spreadsheet_id, p.sheet_id).await
        })
    }
}
