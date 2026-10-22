use crate::db::Database;
use anyhow::Result;
use std::collections::HashSet;
use tracing::info;

/// Outcome of focus gating.
#[derive(Debug, Clone, PartialEq)]
pub enum FocusDecision {
    /// Process this message normally.
    Process,
    /// Message is queued because another chat is focused.
    Queue,
    /// Bypass focus (owner/allowed user).
    Bypass,
}

/// Manages single-chat focus mode.
pub struct FocusManager {
    db: Database,
    owner_user_id: Option<i64>,
    allowed_users: HashSet<i64>,
}

impl FocusManager {
    pub fn new(db: Database, owner_user_id: Option<i64>, allowed_users: HashSet<i64>) -> Self {
        Self {
            db,
            owner_user_id,
            allowed_users,
        }
    }

    /// Decide whether a message from `chat_id` by `user_id` should be processed.
    pub async fn should_process(&self, chat_id: i64, user_id: i64) -> Result<FocusDecision> {
        // Owner and allowed users always bypass
        if self.owner_user_id == Some(user_id) || self.allowed_users.contains(&user_id) {
            return Ok(FocusDecision::Bypass);
        }

        let focus_state = self.db.get_focus_state().await?;

        match focus_state {
            None => Ok(FocusDecision::Process),
            Some((focused_chat_id, _)) => {
                if chat_id == focused_chat_id {
                    Ok(FocusDecision::Process)
                } else {
                    Ok(FocusDecision::Queue)
                }
            }
        }
    }

    /// Switch focus to a new chat. Returns the previously focused chat ID if any.
    pub async fn switch_focus(&self, chat_id: Option<i64>) -> Result<Option<i64>> {
        let prev = self.db.get_focus_state().await?;
        let prev_chat = prev.map(|(c, _)| c);

        self.db.set_focus_state(chat_id).await?;

        if let Some(cid) = chat_id {
            info!(chat_id = cid, "focus switched");
        } else {
            info!("focus cleared");
        }

        Ok(prev_chat)
    }

    /// Get current focus state.
    pub async fn get_focus_state(&self) -> Result<Option<(i64, i64)>> {
        self.db.get_focus_state().await
    }
}
