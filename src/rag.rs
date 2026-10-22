use crate::db::Database;
use anyhow::Result;

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub score: f32,
    pub telegram_message_id: i64,
    pub chat_title: Option<String>,
    pub content: String,
    pub created_at: i64,
    pub message_link: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RagService {
    db: Database,
    top_k: usize,
    candidate_limit: usize,
}

impl RagService {
    pub fn new(db: Database, top_k: usize, candidate_limit: usize) -> Self {
        Self {
            db,
            top_k,
            candidate_limit,
        }
    }

    pub async fn index_message(&self, _row_id: i64, _text: &str) -> Result<()> {
        Ok(())
    }

    pub async fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }

        let candidates = self
            .db
            .fetch_recent_candidates(self.candidate_limit)
            .await?;
        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        let query_lc = query.to_ascii_lowercase();
        let tokens = tokenize(query);

        let mut scored = Vec::new();
        for c in candidates {
            let content_lc = c.content.to_ascii_lowercase();
            let mut score: f32 = if content_lc.contains(&query_lc) {
                4.0_f32
            } else {
                0.0_f32
            };

            for token in &tokens {
                if content_lc.contains(token) {
                    score += 1.0_f32;
                }
            }

            if score > 0.0_f32 {
                scored.push((score, c));
            }
        }

        scored.sort_by(|a, b| {
            b.0.total_cmp(&a.0)
                .then_with(|| b.1.created_at.cmp(&a.1.created_at))
        });
        scored.truncate(self.top_k);

        Ok(scored
            .into_iter()
            .map(|(score, item)| SearchResult {
                score,
                telegram_message_id: item.telegram_message_id,
                chat_title: item.chat_title,
                content: item.content,
                created_at: item.created_at,
                message_link: item.message_link,
            })
            .collect())
    }
}

pub fn format_context(results: &[SearchResult], max_chars: usize) -> String {
    if results.is_empty() {
        return "(no relevant history found)".to_string();
    }

    let mut out = String::new();
    let mut used = 0usize;

    for (idx, result) in results.iter().enumerate() {
        let mut header = format!(
            "[{}] chat={} message={} score={:.3} ts={}\n",
            idx + 1,
            result.chat_title.as_deref().unwrap_or("unknown"),
            result.telegram_message_id,
            result.score,
            result.created_at
        );
        if let Some(link) = &result.message_link {
            header.push_str(&format!("link={}\n", link));
        }

        let content = result.content.trim();
        let snippet = truncate(content, 380);
        let block = format!("{}{snippet}\n\n", header);

        if used + block.len() > max_chars {
            break;
        }

        used += block.len();
        out.push_str(&block);
    }

    if out.is_empty() {
        "(context exists but exceeded max length)".to_string()
    } else {
        out
    }
}

pub fn truncate(input: &str, max: usize) -> String {
    if input.chars().count() <= max {
        return input.to_string();
    }

    let mut out = String::with_capacity(max + 3);
    for ch in input.chars().take(max) {
        out.push(ch);
    }
    out.push_str("...");
    out
}

fn tokenize(input: &str) -> Vec<String> {
    input
        .split(|c: char| !c.is_alphanumeric())
        .filter_map(|piece| {
            let s = piece.trim().to_ascii_lowercase();
            if s.len() < 3 {
                None
            } else {
                Some(s)
            }
        })
        .collect()
}
