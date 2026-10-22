use crate::mcp::{McpServer, UserError};
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct ExtractPdfTextTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExtractPdfTextParams {
    /// Absolute path to a local PDF file. Usually
    /// `/tmp/jarvis_downloads/<id>_<filename>.pdf` after a user upload, or
    /// any path returned by `download_file`.
    pub path: String,
}

impl Tool for ExtractPdfTextTool {
    type Params = ExtractPdfTextParams;

    fn name(&self) -> &'static str {
        "extract_pdf_text"
    }

    fn description(&self) -> &'static str {
        "Extract text from a local PDF file. Works on text-based PDFs but not \
         scanned image PDFs — there's no OCR. When a user uploads a PDF it is \
         stashed under /tmp/jarvis_downloads/; pass that path here to read it."
    }

    fn run<'a>(
        &'a self,
        p: ExtractPdfTextParams,
        _server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            // Validate the path exists before spawning. `pdf_extract::extract_text`
            // is a sync call that also parses the whole document, so we run it
            // on the blocking threadpool.
            let path = p.path.clone();
            let path_check = std::path::Path::new(&path);
            if !path_check.exists() {
                return Err(anyhow::Error::new(UserError(format!(
                    "no file at {path}"
                ))));
            }
            let text = tokio::task::spawn_blocking(move || {
                pdf_extract::extract_text(&path)
                    .map_err(|e| anyhow::anyhow!("pdf extraction failed: {e}"))
            })
            .await
            .map_err(|e| anyhow::anyhow!("pdf task join error: {e}"))??;

            if text.trim().is_empty() {
                return Err(anyhow::Error::new(UserError(
                    "pdf contained no extractable text (is this a scanned image PDF?)".into(),
                )));
            }
            Ok(text)
        })
    }
}
