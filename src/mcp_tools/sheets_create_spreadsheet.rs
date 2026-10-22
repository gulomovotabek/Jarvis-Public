use crate::mcp::{McpServer, UserError};
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct SheetsCreateSpreadsheetTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SheetsCreateSpreadsheetParams {
    /// Title of the new spreadsheet.
    pub title: String,
}

impl Tool for SheetsCreateSpreadsheetTool {
    type Params = SheetsCreateSpreadsheetParams;

    fn name(&self) -> &'static str {
        "sheets_create_spreadsheet"
    }

    fn description(&self) -> &'static str {
        "Create a brand-new Google spreadsheet owned by the service account."
    }

    fn run<'a>(
        &'a self,
        p: SheetsCreateSpreadsheetParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let sheets = server.google_sheets.as_ref().ok_or_else(|| {
                anyhow::Error::new(UserError(
                    "Google Sheets not configured (set GOOGLE_SERVICE_ACCOUNT_KEY env var)".into(),
                ))
            })?;
            sheets.create_spreadsheet(&p.title).await
        })
    }
}
