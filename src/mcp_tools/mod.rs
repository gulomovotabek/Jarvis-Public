//! Typed MCP tool registry.
//!
//! Each tool implements [`Tool`] with a parameter struct that derives both
//! `serde::Deserialize` and `schemars::JsonSchema`, so the `inputSchema` sent
//! to the MCP client and the runtime parameter extraction come from the same
//! place. No more drift between a hand-rolled JSON schema and a handler that
//! reaches into `serde_json::Value` with copy-pasted `.get().and_then().context()`.
//!
//! Migration strategy: tools are migrated one-by-one into this module. The
//! legacy `call_tool` match in `mcp.rs` stays as a fallback for unmigrated
//! tools, and a drift test asserts no tool is defined in both places.

use crate::mcp::{McpServer, UserError};
use anyhow::Result;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;

pub mod add_monitored_group;
pub mod approve_chat_join_request;
pub mod ban_chat_member;
pub mod bot_edit_message;
pub mod bot_send_message;
#[cfg(feature = "calendar")]
pub mod calendar_create_event;
#[cfg(feature = "calendar")]
pub mod calendar_delete_event;
#[cfg(feature = "calendar")]
pub mod calendar_list_events;
pub mod cancel_reminder;
pub mod copy_message;
pub mod currency_rates;
pub mod create_chat_invite_link;
pub mod create_memory;
pub mod create_scheduled_task;
pub mod decline_chat_join_request;
pub mod delete_chat_photo;
pub mod delete_memory;
pub mod delete_message;
pub mod delete_scheduled_task;
pub mod download_file;
pub mod edit_memory;
#[cfg(feature = "pdf")]
pub mod extract_pdf_text;
#[cfg(feature = "faceit")]
pub mod faceit_match;
#[cfg(feature = "faceit")]
pub mod faceit_matches;
#[cfg(feature = "faceit")]
pub mod faceit_profile;
pub mod forward_message;
pub mod get_chat_member_count;
pub mod get_focus_state;
pub mod get_heartbeats;
pub mod get_tool_stats;
#[cfg(feature = "gmail")]
pub mod gmail_list_recent;
#[cfg(feature = "gmail")]
pub mod gmail_read_message;
#[cfg(feature = "gmail")]
pub mod gmail_search;
pub mod leave_chat;
pub mod list_memories;
pub mod list_monitored_groups;
pub mod list_reminders;
pub mod list_scheduled_tasks;
pub mod mute_chat;
pub mod now;
pub mod pin_message;
pub mod publish_blog;
pub mod query;
pub mod read_memory;
pub mod remove_monitored_group;
pub mod restart_self;
#[cfg(feature = "rss")]
pub mod rss_read;
pub mod search_memories;
pub mod search_messages;
pub mod self_deploy;
pub mod self_improve;
pub mod self_improve_status;
pub mod send_audio;
pub mod send_chat_action;
pub mod send_contact;
pub mod send_document;
#[cfg(feature = "smtp")]
pub mod send_email_application;
pub mod send_location;
pub mod send_photo;
pub mod send_poll;
pub mod send_reaction;
pub mod send_sticker;
pub mod send_video;
pub mod set_chat_description;
pub mod set_chat_photo;
pub mod set_chat_title;
pub mod set_reminder;
#[cfg(feature = "sheets")]
pub mod sheets_append;
#[cfg(feature = "sheets")]
pub mod sheets_batch_read;
#[cfg(feature = "sheets")]
pub mod sheets_batch_write;
#[cfg(feature = "sheets")]
pub mod sheets_clear;
#[cfg(feature = "sheets")]
pub mod sheets_create_sheet;
#[cfg(feature = "sheets")]
pub mod sheets_create_spreadsheet;
#[cfg(feature = "sheets")]
pub mod sheets_delete_sheet;
#[cfg(feature = "sheets")]
pub mod sheets_info;
#[cfg(feature = "sheets")]
pub mod sheets_read;
#[cfg(feature = "sheets")]
pub mod sheets_write;
pub mod switch_focus;
pub mod transcribe_voice;
pub mod unban_chat_member;
pub mod unmute_chat;
pub mod unpin_message;
pub mod update_scheduled_task;
pub mod weather;
pub mod web_fetch;
pub mod web_search;
pub mod youtube_transcript;

/// A typed MCP tool. Implementations define a parameter struct, and both the
/// input schema and parameter parsing are derived from it automatically.
pub trait Tool: Send + Sync + 'static {
    type Params: JsonSchema + DeserializeOwned + Send;

    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;

    fn run<'a>(
        &'a self,
        params: Self::Params,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>>;
}

/// Object-safe erased view of [`Tool`] used for dynamic dispatch.
pub trait ErasedTool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn schema(&self) -> Value;
    fn call<'a>(
        &'a self,
        args: &'a Value,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>>;
}

impl<T: Tool> ErasedTool for T {
    fn name(&self) -> &'static str {
        Tool::name(self)
    }

    fn description(&self) -> &'static str {
        Tool::description(self)
    }

    fn schema(&self) -> Value {
        use schemars::gen::SchemaGenerator;
        let root = SchemaGenerator::default().into_root_schema_for::<T::Params>();
        serde_json::to_value(root.schema).unwrap_or_else(|_| json!({ "type": "object" }))
    }

    fn call<'a>(
        &'a self,
        args: &'a Value,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let params: T::Params = serde_json::from_value(args.clone()).map_err(|e| {
                // UserError → message passes through the MCP error filter unchanged,
                // so Claude sees "invalid params: ..." and can self-correct.
                anyhow::Error::new(UserError(format!("invalid parameters: {e}")))
            })?;
            Tool::run(self, params, server).await
        })
    }
}

pub struct ToolRegistry {
    tools: HashMap<&'static str, Box<dyn ErasedTool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: HashMap::new() }
    }

    pub fn register<T: Tool>(&mut self, tool: T) {
        let name = Tool::name(&tool);
        if self.tools.insert(name, Box::new(tool)).is_some() {
            panic!("duplicate tool registration: {name}");
        }
    }

    pub fn get(&self, name: &str) -> Option<&dyn ErasedTool> {
        self.tools.get(name).map(|b| b.as_ref())
    }

    pub fn iter(&self) -> impl Iterator<Item = &dyn ErasedTool> {
        self.tools.values().map(|b| b.as_ref())
    }

    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.tools.keys().copied()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Serialize all registered tools as MCP tool definitions, sorted by name
/// (stable ordering makes diffs in tests and logs readable).
pub fn list_definitions(registry: &ToolRegistry) -> Vec<Value> {
    let mut out: Vec<Value> = registry
        .iter()
        .map(|t| {
            json!({
                "name": t.name(),
                "description": t.description(),
                "inputSchema": t.schema(),
            })
        })
        .collect();
    out.sort_by(|a, b| {
        a.get("name").and_then(|v| v.as_str()).cmp(&b.get("name").and_then(|v| v.as_str()))
    });
    out
}

/// Process-global registry. Initialized on first access and immutable thereafter.
pub fn global() -> &'static ToolRegistry {
    static REGISTRY: OnceLock<ToolRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut r = ToolRegistry::new();
        // Admin
        r.register(now::NowTool);
        // Messaging
        r.register(search_messages::SearchMessagesTool);
        // Memory
        r.register(create_memory::CreateMemoryTool);
        r.register(read_memory::ReadMemoryTool);
        r.register(edit_memory::EditMemoryTool);
        r.register(list_memories::ListMemoriesTool);
        r.register(search_memories::SearchMemoriesTool);
        r.register(delete_memory::DeleteMemoryTool);
        // Reminders
        r.register(set_reminder::SetReminderTool);
        r.register(list_reminders::ListRemindersTool);
        r.register(cancel_reminder::CancelReminderTool);
        // Database
        r.register(query::QueryTool);
        // Admin
        r.register(get_heartbeats::GetHeartbeatsTool);
        r.register(get_tool_stats::GetToolStatsTool);
        r.register(restart_self::RestartSelfTool);
        r.register(self_improve::SelfImproveTool);
        r.register(self_improve_status::SelfImproveStatusTool);
        r.register(self_deploy::SelfDeployTool);
        #[cfg(feature = "pdf")]
        r.register(extract_pdf_text::ExtractPdfTextTool);
        #[cfg(feature = "smtp")]
        r.register(send_email_application::SendEmailApplicationTool);
        // Gmail — gated behind `gmail` feature (default on). Requires
        // GMAIL_IMAP_USER + GMAIL_APP_PASSWORD in .env to actually function;
        // tools are registered regardless so Claude learns they exist.
        #[cfg(feature = "gmail")]
        {
            r.register(gmail_list_recent::GmailListRecentTool);
            r.register(gmail_search::GmailSearchTool);
            r.register(gmail_read_message::GmailReadMessageTool);
        }
        // Focus
        r.register(switch_focus::SwitchFocusTool);
        r.register(get_focus_state::GetFocusStateTool);
        r.register(mute_chat::MuteChatTool);
        r.register(unmute_chat::UnmuteChatTool);
        // Scheduled tasks
        r.register(create_scheduled_task::CreateScheduledTaskTool);
        r.register(list_scheduled_tasks::ListScheduledTasksTool);
        r.register(update_scheduled_task::UpdateScheduledTaskTool);
        r.register(delete_scheduled_task::DeleteScheduledTaskTool);
        // Proactive monitoring
        r.register(add_monitored_group::AddMonitoredGroupTool);
        r.register(remove_monitored_group::RemoveMonitoredGroupTool);
        r.register(list_monitored_groups::ListMonitoredGroupsTool);
        // Web
        r.register(web_fetch::WebFetchTool);
        r.register(web_search::WebSearchTool);
        r.register(download_file::DownloadFileTool);
        r.register(publish_blog::PublishBlogTool);
        r.register(youtube_transcript::YoutubeTranscriptTool);
        #[cfg(feature = "rss")]
        r.register(rss_read::RssReadTool);
        // Info utilities
        r.register(weather::WeatherTool);
        r.register(currency_rates::CurrencyRatesTool);
        // Audio
        r.register(transcribe_voice::TranscribeVoiceTool);
        // Google Calendar — gated behind `calendar` feature (default on).
        #[cfg(feature = "calendar")]
        {
            r.register(calendar_list_events::CalendarListEventsTool);
            r.register(calendar_create_event::CalendarCreateEventTool);
            r.register(calendar_delete_event::CalendarDeleteEventTool);
        }
        // FACEIT — gated behind `faceit` feature (default on).
        #[cfg(feature = "faceit")]
        {
            r.register(faceit_profile::FaceitProfileTool);
            r.register(faceit_matches::FaceitMatchesTool);
            r.register(faceit_match::FaceitMatchTool);
        }
        // Google Sheets — gated behind `sheets` feature (default on).
        #[cfg(feature = "sheets")]
        {
            r.register(sheets_read::SheetsReadTool);
            r.register(sheets_write::SheetsWriteTool);
            r.register(sheets_append::SheetsAppendTool);
            r.register(sheets_clear::SheetsClearTool);
            r.register(sheets_info::SheetsInfoTool);
            r.register(sheets_create_sheet::SheetsCreateSheetTool);
            r.register(sheets_delete_sheet::SheetsDeleteSheetTool);
            r.register(sheets_create_spreadsheet::SheetsCreateSpreadsheetTool);
            r.register(sheets_batch_read::SheetsBatchReadTool);
            r.register(sheets_batch_write::SheetsBatchWriteTool);
        }
        // Bot API — messaging
        r.register(bot_send_message::BotSendMessageTool);
        r.register(bot_edit_message::BotEditMessageTool);
        r.register(send_reaction::SendReactionTool);
        r.register(delete_message::DeleteMessageTool);
        r.register(forward_message::ForwardMessageTool);
        r.register(copy_message::CopyMessageTool);
        r.register(pin_message::PinMessageTool);
        r.register(unpin_message::UnpinMessageTool);
        // Bot API — media
        r.register(send_photo::SendPhotoTool);
        r.register(send_video::SendVideoTool);
        r.register(send_audio::SendAudioTool);
        r.register(send_document::SendDocumentTool);
        r.register(send_sticker::SendStickerTool);
        r.register(send_location::SendLocationTool);
        r.register(send_contact::SendContactTool);
        // Bot API — chat admin
        r.register(set_chat_title::SetChatTitleTool);
        r.register(set_chat_description::SetChatDescriptionTool);
        r.register(set_chat_photo::SetChatPhotoTool);
        r.register(delete_chat_photo::DeleteChatPhotoTool);
        r.register(get_chat_member_count::GetChatMemberCountTool);
        r.register(create_chat_invite_link::CreateChatInviteLinkTool);
        r.register(send_chat_action::SendChatActionTool);
        r.register(send_poll::SendPollTool);
        // Bot API — membership
        r.register(ban_chat_member::BanChatMemberTool);
        r.register(unban_chat_member::UnbanChatMemberTool);
        r.register(approve_chat_join_request::ApproveChatJoinRequestTool);
        r.register(decline_chat_join_request::DeclineChatJoinRequestTool);
        r.register(leave_chat::LeaveChatTool);
        r
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_contains_migrated_tools() {
        let r = global();
        assert!(r.get("now").is_some(), "now tool missing from registry");
        assert!(
            r.get("search_messages").is_some(),
            "search_messages tool missing from registry"
        );
    }

    #[test]
    #[ignore = "debug only — run with --ignored to print generated schemas"]
    fn dump_schemas() {
        let defs = list_definitions(global());
        println!("{}", serde_json::to_string_pretty(&defs).unwrap());
    }

    #[test]
    fn every_registered_tool_has_object_schema() {
        for tool in global().iter() {
            let schema = tool.schema();
            assert!(
                schema.is_object(),
                "tool `{}` schema is not a JSON object",
                tool.name()
            );
            assert_eq!(
                schema.get("type").and_then(|v| v.as_str()),
                Some("object"),
                "tool `{}` schema root type must be \"object\"",
                tool.name()
            );
        }
    }

    #[test]
    fn list_definitions_is_sorted() {
        let defs = list_definitions(global());
        let names: Vec<_> = defs.iter().filter_map(|d| d["name"].as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "list_definitions output must be sorted by name");
    }

    /// Ensure the registry has the expected minimum tool count (accounting
    /// for feature-gated tools) and that every tool name is unique.
    /// `ToolRegistry::register` panics on duplicate names, so this test also
    /// doubles as smoke for construction.
    #[test]
    fn registry_has_all_expected_tools() {
        let names: Vec<&'static str> = global().names().collect();

        // Core (always-compiled) tools, after the job-application subsystem
        // (ATS apply, LinkedIn, resumes, answer bank, job profile, application
        // tracking, dispatch_campaign) was removed. Includes weather,
        // currency_rates, youtube_transcript, transcribe_voice,
        // send_location, send_contact.
        let mut expected_minimum = 60;
        #[cfg(feature = "faceit")]
        {
            // faceit_profile, faceit_matches, faceit_match (demo tools removed).
            expected_minimum += 3;
        }
        #[cfg(feature = "sheets")]
        {
            expected_minimum += 10;
        }
        #[cfg(feature = "gmail")]
        {
            expected_minimum += 3;
        }
        #[cfg(feature = "pdf")]
        {
            expected_minimum += 1;
        }
        #[cfg(feature = "smtp")]
        {
            expected_minimum += 1;
        }
        #[cfg(feature = "rss")]
        {
            expected_minimum += 1;
        }
        #[cfg(feature = "calendar")]
        {
            expected_minimum += 3;
        }
        assert!(
            names.len() >= expected_minimum,
            "registry only has {} tools, expected at least {expected_minimum}",
            names.len()
        );

        let unique: std::collections::HashSet<&&'static str> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "duplicate tool names in registry");

        // Always-present sanity set — every tool listed here must exist
        // regardless of feature selection.
        for expected in [
            "now",
            "search_messages",
            "create_memory",
            "set_reminder",
            "query",
            "bot_send_message",
            "send_photo",
            "web_fetch",
            "get_tool_stats",
        ] {
            assert!(
                global().get(expected).is_some(),
                "registry missing expected tool `{expected}`"
            );
        }

        #[cfg(feature = "sheets")]
        assert!(global().get("sheets_read").is_some());
        #[cfg(feature = "faceit")]
        assert!(global().get("faceit_profile").is_some());
    }
}
