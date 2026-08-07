use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Ok,
    NotLoggedIn,
    AuthExpired,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DataSource {
    Live,
    Cached,
    LocalEstimate,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageWindow {
    /// 0-100
    pub used_pct: f32,
    /// Short human label, e.g. "4:12pm" or "Mon" or "Aug 18"
    pub resets_label: String,
    pub resets_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUsage {
    pub tool: String,
    pub status: ToolStatus,
    pub five_hour: Option<UsageWindow>,
    pub weekly: Option<UsageWindow>,
    pub monthly: Option<UsageWindow>,
    /// Small secondary note, e.g. Cursor on-demand spend
    pub note: Option<String>,
    pub source: DataSource,
    pub fetched_at: DateTime<Utc>,
    pub message: Option<String>,
}

impl ToolUsage {
    pub fn not_logged_in(tool: &str, message: impl Into<String>) -> Self {
        Self {
            tool: tool.to_string(),
            status: ToolStatus::NotLoggedIn,
            five_hour: None,
            weekly: None,
            monthly: None,
            note: None,
            source: DataSource::None,
            fetched_at: Utc::now(),
            message: Some(message.into()),
        }
    }

    pub fn auth_expired(tool: &str, message: impl Into<String>) -> Self {
        Self {
            tool: tool.to_string(),
            status: ToolStatus::AuthExpired,
            five_hour: None,
            weekly: None,
            monthly: None,
            note: None,
            source: DataSource::None,
            fetched_at: Utc::now(),
            message: Some(message.into()),
        }
    }

    pub fn error(tool: &str, message: impl Into<String>) -> Self {
        Self {
            tool: tool.to_string(),
            status: ToolStatus::Error,
            five_hour: None,
            weekly: None,
            monthly: None,
            note: None,
            source: DataSource::None,
            fetched_at: Utc::now(),
            message: Some(message.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AllUsage {
    pub claude: Option<ToolUsage>,
    pub codex: Option<ToolUsage>,
    pub cursor: Option<ToolUsage>,
}
