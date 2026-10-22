use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::{Context, Result, bail};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

pub struct TranscribeVoiceTool;

const MAX_AUDIO_BYTES: u64 = 25 * 1024 * 1024;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TranscribeVoiceParams {
    /// Telegram file_id of a voice/audio/video-note message (from raw_json in the messages table).
    #[serde(default)]
    pub file_id: Option<String>,
    /// Local audio file path (alternative to file_id).
    #[serde(default)]
    pub path: Option<String>,
    /// Optional ISO language hint, e.g. "uz", "ru", "en" — improves accuracy.
    #[serde(default)]
    pub language: Option<String>,
}

impl Tool for TranscribeVoiceTool {
    type Params = TranscribeVoiceParams;

    fn name(&self) -> &'static str {
        "transcribe_voice"
    }

    fn description(&self) -> &'static str {
        "Transcribe a Telegram voice message (by file_id) or local audio file to text via Whisper (Groq or OpenAI — needs GROQ_API_KEY or OPENAI_API_KEY)."
    }

    fn run<'a>(
        &'a self,
        p: TranscribeVoiceParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let groq_key = std::env::var("GROQ_API_KEY").ok().filter(|v| !v.trim().is_empty());
            let openai_key = std::env::var("OPENAI_API_KEY").ok().filter(|v| !v.trim().is_empty());
            let (endpoint, model, key) = match (&groq_key, &openai_key) {
                (Some(k), _) => (
                    "https://api.groq.com/openai/v1/audio/transcriptions",
                    "whisper-large-v3",
                    k.clone(),
                ),
                (None, Some(k)) => (
                    "https://api.openai.com/v1/audio/transcriptions",
                    "whisper-1",
                    k.clone(),
                ),
                (None, None) => bail!(
                    "no transcription backend configured — set GROQ_API_KEY (free tier at console.groq.com) or OPENAI_API_KEY"
                ),
            };

            let (bytes, filename) = match (&p.file_id, &p.path) {
                (Some(file_id), _) => {
                    use teloxide::net::Download;
                    use teloxide::prelude::Requester;
                    let bot = server.bot()?;
                    let file = bot.get_file(file_id).await.context("getFile failed")?;
                    if file.size as u64 > MAX_AUDIO_BYTES {
                        bail!("audio too large: {} bytes (max {MAX_AUDIO_BYTES})", file.size);
                    }
                    let mut data = Vec::with_capacity(file.size as usize);
                    bot.download_file(&file.path, &mut data)
                        .await
                        .context("telegram file download failed")?;
                    let name = file
                        .path
                        .rsplit('/')
                        .next()
                        .filter(|n| n.contains('.'))
                        .unwrap_or("voice.oga")
                        .to_string();
                    (data, name)
                }
                (None, Some(path)) => {
                    let meta = tokio::fs::metadata(path)
                        .await
                        .with_context(|| format!("file not found: {path}"))?;
                    if meta.len() > MAX_AUDIO_BYTES {
                        bail!("audio too large: {} bytes (max {MAX_AUDIO_BYTES})", meta.len());
                    }
                    let data = tokio::fs::read(path).await?;
                    let name = std::path::Path::new(path)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("audio.ogg")
                        .to_string();
                    (data, name)
                }
                (None, None) => bail!("pass file_id or path"),
            };

            let mut form = reqwest::multipart::Form::new()
                .part(
                    "file",
                    reqwest::multipart::Part::bytes(bytes).file_name(filename),
                )
                .text("model", model);
            if let Some(lang) = &p.language {
                form = form.text("language", lang.clone());
            }

            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()?;
            let resp = client
                .post(endpoint)
                .bearer_auth(&key)
                .multipart(form)
                .send()
                .await
                .context("transcription request failed")?;
            let status = resp.status();
            let body: Value = resp.json().await.context("failed to parse transcription response")?;
            if !status.is_success() {
                let msg = body["error"]["message"].as_str().unwrap_or("unknown error");
                bail!("transcription failed ({status}): {msg}");
            }
            let text = body["text"].as_str().unwrap_or("").trim().to_string();
            if text.is_empty() {
                bail!("transcription returned empty text");
            }
            Ok(text)
        })
    }
}
