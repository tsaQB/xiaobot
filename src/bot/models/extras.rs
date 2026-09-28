//! Bot API objects for replies, checklists and inline mode. Only the fields
//! Xiao reads are modelled; everything is optional where Telegram may omit it.

use serde::{Deserialize, Serialize};

use super::base::{deserialize_flexible_i64, deserialize_flexible_opt_i64, Location, User};

/// The part of a replied-to message the user quoted (`TextQuote`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextQuote {
    pub text: String,
    #[serde(default, deserialize_with = "deserialize_flexible_opt_i64")]
    pub position: Option<i64>,
    /// `True` when the user picked the quote by hand; otherwise the server
    /// added it automatically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_manual: Option<bool>,
}

/// One task of a checklist (`ChecklistTask`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChecklistTask {
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub id: i64,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_by_user: Option<User>,
    /// Unix time of completion; `0` or absent while the task is open.
    #[serde(default, deserialize_with = "deserialize_flexible_opt_i64")]
    pub completion_date: Option<i64>,
}

impl ChecklistTask {
    pub fn is_done(&self) -> bool {
        self.completed_by_user.is_some() || self.completion_date.is_some_and(|date| date > 0)
    }
}

/// A checklist shared in a chat (`Checklist`). Bots can only send checklists
/// on behalf of a business account, but they can read the ones users share.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checklist {
    pub title: String,
    #[serde(default)]
    pub tasks: Vec<ChecklistTask>,
}

/// An incoming inline query (`InlineQuery`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineQuery {
    pub id: String,
    pub from: User,
    #[serde(default)]
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_type: Option<String>,
}

/// An inline result the user picked and sent (`ChosenInlineResult`).
/// Delivered only when inline feedback is enabled in @BotFather.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChosenInlineResult {
    pub result_id: String,
    pub from: User,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
    /// Present only when the sent message carries an inline keyboard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_message_id: Option<String>,
    #[serde(default)]
    pub query: String,
}
