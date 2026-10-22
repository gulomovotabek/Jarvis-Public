use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::{Context, Result, bail};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

pub struct YoutubeTranscriptTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct YoutubeTranscriptParams {
    /// YouTube video URL or bare 11-char video id.
    pub video: String,
    /// Preferred caption language code, e.g. "en", "ru", "uz". Falls back to English, then first available.
    #[serde(default)]
    pub language: Option<String>,
}

fn extract_video_id(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.len() == 11 && trimmed.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Some(trimmed.to_string());
    }
    let url = url::Url::parse(trimmed).ok()?;
    if url.host_str()? .contains("youtu.be") {
        return url.path_segments()?.next().map(|s| s.to_string());
    }
    if let Some(v) = url.query_pairs().find(|(k, _)| k == "v") {
        return Some(v.1.to_string());
    }
    // /shorts/ID, /embed/ID
    let segments: Vec<_> = url.path_segments()?.collect();
    if segments.len() >= 2 && matches!(segments[0], "shorts" | "embed" | "live") {
        return Some(segments[1].to_string());
    }
    None
}

/// Extract the JSON array following `"captionTracks":` with a string-aware
/// bracket-depth scan (snippet contains nested objects and escaped strings).
fn extract_caption_tracks(html: &str) -> Option<Value> {
    let start = html.find("\"captionTracks\":")? + "\"captionTracks\":".len();
    let rest = &html[start..];
    let bytes = rest.as_bytes();
    if bytes.first() != Some(&b'[') {
        return None;
    }
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'[' | b'{' => depth += 1,
            b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return serde_json::from_str(&rest[..=i]).ok();
                }
            }
            _ => {}
        }
    }
    None
}

fn decode_xml_entities(s: &str) -> String {
    s.replace("&amp;#39;", "'")
        .replace("&#39;", "'")
        .replace("&amp;quot;", "\"")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
}

impl Tool for YoutubeTranscriptTool {
    type Params = YoutubeTranscriptParams;

    fn name(&self) -> &'static str {
        "youtube_transcript"
    }

    fn description(&self) -> &'static str {
        "Fetch the caption transcript of a YouTube video as plain text (no API key; only works when the video has captions)."
    }

    fn run<'a>(
        &'a self,
        p: YoutubeTranscriptParams,
        _server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let video_id = extract_video_id(&p.video)
                .with_context(|| format!("could not extract video id from: {}", p.video))?;
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
                .build()?;
            let html = client
                .get(format!("https://www.youtube.com/watch?v={video_id}"))
                .send()
                .await
                .context("watch page request failed")?
                .text()
                .await?;

            let Some(tracks) = extract_caption_tracks(&html) else {
                bail!("no captions found for video {video_id} (video may have captions disabled, or YouTube blocked the request)");
            };
            let tracks = tracks.as_array().cloned().unwrap_or_default();
            if tracks.is_empty() {
                bail!("caption track list is empty for video {video_id}");
            }

            let lang_of = |t: &Value| t["languageCode"].as_str().unwrap_or("").to_string();
            let track = p
                .language
                .as_deref()
                .and_then(|want| tracks.iter().find(|t| lang_of(t).starts_with(want)))
                .or_else(|| tracks.iter().find(|t| lang_of(t).starts_with("en")))
                .or_else(|| tracks.first())
                .cloned()
                .context("no usable caption track")?;
            let base_url = track["baseUrl"].as_str().context("caption track has no baseUrl")?;

            let xml = client
                .get(base_url)
                .send()
                .await
                .context("caption fetch failed")?
                .text()
                .await?;

            let mut text = String::new();
            for chunk in xml.split("<text") {
                let Some(gt) = chunk.find('>') else { continue };
                let Some(end) = chunk[gt..].find("</text>") else { continue };
                let piece = decode_xml_entities(chunk[gt + 1..gt + end].trim());
                if !piece.is_empty() {
                    if !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(&piece);
                }
            }
            if text.is_empty() {
                bail!("caption track fetched but contained no text");
            }
            const MAX: usize = 20_000;
            if text.chars().count() > MAX {
                text = text.chars().take(MAX).collect::<String>() + "\n\n[...truncated]";
            }
            Ok(format!("Transcript ({}) for {video_id}:\n{text}", lang_of(&track)))
        })
    }
}
