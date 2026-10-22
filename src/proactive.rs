use crate::claude_code::ClaudeCode;
use crate::db::Database;
use anyhow::Result;
use tracing::{debug, warn};

#[derive(Debug, Clone)]
pub enum ProactiveDecision {
    Respond(String),
    React(String),
    Silent,
}

/// Check cooldown: returns true if the bot should skip (too recent).
pub async fn check_cooldown(
    db: &Database,
    chat_id: i64,
    is_bot_message: bool,
    cooldown_secs: u64,
    bot_cooldown_secs: u64,
) -> Result<bool> {
    let now = epoch_now();
    let (last_response, last_reaction) = db.get_proactive_cooldown(chat_id).await?;
    let last_action = last_response.max(last_reaction);

    let threshold = if is_bot_message {
        bot_cooldown_secs as i64
    } else {
        cooldown_secs as i64
    };

    Ok(now - last_action < threshold)
}

/// Build the decision prompt and call the subprocess, then parse the result.
pub async fn evaluate_proactive(
    claude: &ClaudeCode,
    recent_messages: &[(Option<i64>, Option<String>, String, i64)],
    bot_user_id: i64,
    is_bot_message: bool,
    last_response_secs_ago: Option<i64>,
) -> Result<ProactiveDecision> {
    let mut chat_lines = String::new();
    for (sender_id, sender_name, content, _ts) in recent_messages {
        let name = if *sender_id == Some(bot_user_id) {
            "Jarvis".to_string()
        } else {
            sender_name.clone().unwrap_or_else(|| "Noma'lum".to_string())
        };
        chat_lines.push_str(&format!("{}: {}\n", name, content));
    }

    let bot_hint = if is_bot_message {
        "Oxirgi xabar BOT tomonidan yuborilgan. Agar CS2, FaZe, Spirit, turnir haqida bo'lsa — hazil bilan javob ber.".to_string()
    } else {
        String::new()
    };

    let recency_hint = match last_response_secs_ago {
        Some(secs) if secs < 300 => format!(
            "\nMUHIM: Jarvis {} soniya oldin shu chatda javob bergan. Agar xabar Jarvisga yo'naltirilgan bo'lishi mumkin bo'lsa, RESPOND qil.",
            secs
        ),
        _ => String::new(),
    };

    let system_prompt = "Sen Jarvis — guruh ishtirokchisi. Sening vazifang: guruhda qiziqarli, hazilkash va foydali bo'lish. HECH QACHON monitoring, trigger, avtomatik yoki texnik tafsilotlar haqida gapirma.";

    let user_prompt = format!(
        r#"Quyida guruh chatidagi oxirgi xabarlar:

{chat_lines}
{bot_hint}{recency_hint}

Sening vazifang: bu xabarlarni tahlil qilib, nima qilish kerakligini hal qil.

Qoidalar (ustuvorlik tartibida):
- Agar OXIRGI xabar to'g'ridan-to'g'ri Jarvisga qaratilgan direktiv bo'lsa ("javob qaytar", "javob ber", "jim bo'lma", "gapir", "nima deysan", yoki savol aniq Jarvisga mo'ljallangan bo'lsa) — MAJBURAN RESPOND qil
- Agar xabar juda qiziqarli, kulgili yoki muhim bo'lsa — RESPOND
- Agar bot CS2, FaZe, Spirit, turnir haqida yozgan bo'lsa — hazil bilan RESPOND
- Agar emoji reaction mos bo'lsa — REACT
- Oddiy suhbat va Jarvisga aloqasi yo'q bo'lsa — SILENT

Faqat JSON formatida javob ber, boshqa hech narsa yozma:
{{"decision": "SILENT"}}
yoki
{{"decision": "RESPOND", "reason": "qisqa sabab"}}
yoki
{{"decision": "REACT", "emoji": "bitta emoji"}}"#
    );

    let combined_prompt = format!("[System: {}]\n\n{}", system_prompt, user_prompt);
    let raw = claude.send_message("", &combined_prompt, "Evaluating group chat").await;

    match raw {
        Ok(ref text) => Ok(parse_decision(text)),
        Err(ref e) => {
            warn!(?e, "proactive decision call failed, defaulting to Silent");
            Ok(ProactiveDecision::Silent)
        }
    }
}

fn parse_decision(raw: &str) -> ProactiveDecision {
    // Try to extract JSON from the response (might have extra text around it)
    let json_str = extract_json(raw);

    let parsed: serde_json::Value = match serde_json::from_str(&json_str) {
        Ok(v) => v,
        Err(e) => {
            debug!(?e, raw, "failed to parse proactive decision JSON");
            return ProactiveDecision::Silent;
        }
    };

    let decision = parsed
        .get("decision")
        .and_then(|v| v.as_str())
        .unwrap_or("SILENT")
        .to_ascii_uppercase();

    match decision.as_str() {
        "RESPOND" => {
            let reason = parsed
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            ProactiveDecision::Respond(reason)
        }
        "REACT" => {
            let emoji = parsed
                .get("emoji")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if emoji.is_empty() {
                ProactiveDecision::Silent
            } else {
                ProactiveDecision::React(emoji)
            }
        }
        _ => ProactiveDecision::Silent,
    }
}

/// Extract the first JSON object from a string that may contain surrounding text.
fn extract_json(input: &str) -> String {
    let trimmed = input.trim();
    if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            if end >= start {
                return trimmed[start..=end].to_string();
            }
        }
    }
    trimmed.to_string()
}

fn epoch_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}
