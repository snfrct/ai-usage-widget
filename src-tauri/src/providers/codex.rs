use std::path::PathBuf;
use std::process::Command;

use chrono::{DateTime, Local, TimeZone, Utc};
use serde::Deserialize;

use crate::models::{DataSource, ToolStatus, ToolUsage, UsageWindow};

fn codex_home() -> PathBuf {
    std::env::var("CODEX_HOME")
        .ok()
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".codex"))
}

#[derive(Debug, Deserialize)]
struct AuthFile {
    tokens: Option<Tokens>,
}

#[derive(Debug, Deserialize)]
struct Tokens {
    access_token: Option<String>,
    account_id: Option<String>,
}

struct Credential {
    access_token: String,
    account_id: Option<String>,
}

fn read_credential() -> Option<Credential> {
    let path = codex_home().join("auth.json");
    let content = std::fs::read_to_string(path).ok()?;
    let parsed: AuthFile = serde_json::from_str(&content).ok()?;
    let tokens = parsed.tokens?;
    let access_token = tokens.access_token.filter(|t| !t.is_empty())?;
    Some(Credential {
        access_token,
        account_id: tokens.account_id,
    })
}

#[derive(Debug, Deserialize)]
struct WindowSnapshot {
    used_percent: Option<f64>,
    reset_at: Option<i64>,
    /// Free-tier Codex accounts get a single ~30-day (monthly) rate limit
    /// window instead of the 5h+weekly pair paid plans get — confirmed via
    /// a real `plan_type: "free"` response where `primary_window` had
    /// `limit_window_seconds: 2592000` (30 days) and `secondary_window` was
    /// null. Using this field to classify each window is what lets both
    /// shapes map correctly instead of assuming `primary` is always 5h.
    limit_window_seconds: Option<i64>,
}

#[derive(Debug, Clone, Copy)]
enum WindowKind {
    FiveHour,
    Weekly,
    Monthly,
}

fn classify_window(seconds: i64) -> WindowKind {
    const ONE_DAY: i64 = 24 * 3600;
    if seconds <= ONE_DAY {
        WindowKind::FiveHour
    } else if seconds <= 10 * ONE_DAY {
        WindowKind::Weekly
    } else {
        WindowKind::Monthly
    }
}

#[derive(Debug, Deserialize)]
struct RateLimit {
    primary_window: Option<WindowSnapshot>,
    secondary_window: Option<WindowSnapshot>,
}

#[derive(Debug, Deserialize)]
struct UsageResponse {
    rate_limit: Option<RateLimit>,
}

async fn fetch_live(cred: &Credential) -> Result<UsageResponse, String> {
    let client = reqwest::Client::new();
    let mut req = client
        .get("https://chatgpt.com/backend-api/wham/usage")
        .header("Authorization", format!("Bearer {}", cred.access_token))
        .header("Accept", "application/json")
        .header("User-Agent", "ai-usage-widget");
    if let Some(account_id) = &cred.account_id {
        req = req.header("ChatGPT-Account-Id", account_id.clone());
    }

    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    if status.as_u16() == 401 {
        return Err("unauthorized".into());
    }
    let body = resp.text().await.map_err(|e| e.to_string())?;
    crate::debug_log!("[codex] raw usage response: {body}");
    if !status.is_success() {
        return Err(format!("http {status}"));
    }
    serde_json::from_str::<UsageResponse>(&body).map_err(|e| e.to_string())
}

fn reset_label(dt: DateTime<Utc>) -> String {
    let local = dt.with_timezone(&Local);
    let now = Local::now();
    if local.date_naive() == now.date_naive() {
        local.format("%-I:%M%p").to_string().to_lowercase()
    } else {
        local.format("%a").to_string()
    }
}

fn monthly_reset_label(dt: DateTime<Utc>) -> String {
    dt.with_timezone(&Local).format("%b %-d").to_string()
}

fn to_window(resp: &WindowSnapshot, kind: WindowKind) -> Option<UsageWindow> {
    let used_pct = resp.used_percent?;
    let resets_at = resp.reset_at.and_then(|secs| Utc.timestamp_opt(secs, 0).single());
    let resets_label = resets_at
        .map(|dt| match kind {
            WindowKind::Monthly => monthly_reset_label(dt),
            WindowKind::FiveHour | WindowKind::Weekly => reset_label(dt),
        })
        .unwrap_or_default();
    Some(UsageWindow {
        used_pct: used_pct as f32,
        resets_label,
        resets_at,
    })
}

/// Shells out to `codex login`, mirroring the Claude Code integration: we
/// never implement our own OAuth flow, just reuse the CLI's own login and
/// then read the credential file it writes.
#[tauri::command]
pub fn codex_login() -> Result<(), String> {
    Command::new("codex")
        .arg("login")
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Couldn't launch `codex login`: {e}"))
}

pub async fn refresh() -> ToolUsage {
    let Some(cred) = read_credential() else {
        crate::debug_log!("[codex] no credential found (~/.codex/auth.json missing or unparsable)");
        return ToolUsage::not_logged_in("codex", "Not logged in — run `codex login`");
    };
    crate::debug_log!(
        "[codex] credential found: access_token_len={} has_account_id={}",
        cred.access_token.len(),
        cred.account_id.is_some()
    );

    match fetch_live(&cred).await {
        Ok(resp) => {
            let rate_limit = resp.rate_limit.unwrap_or(RateLimit {
                primary_window: None,
                secondary_window: None,
            });

            // `primary_window` isn't always the 5h window — free-tier
            // accounts get a single ~30-day window there instead, with
            // `secondary_window` absent entirely. Classify by
            // `limit_window_seconds` rather than assuming a fixed position;
            // when that field is missing, fall back to the old positional
            // assumption (primary=5h, secondary=weekly) for safety.
            let mut five_hour = None;
            let mut weekly = None;
            let mut monthly = None;
            for (window, default_kind) in [
                (rate_limit.primary_window.as_ref(), WindowKind::FiveHour),
                (rate_limit.secondary_window.as_ref(), WindowKind::Weekly),
            ] {
                let Some(window) = window else { continue };
                let kind = window.limit_window_seconds.map(classify_window).unwrap_or(default_kind);
                let mapped = to_window(window, kind);
                match kind {
                    WindowKind::FiveHour => five_hour = mapped,
                    WindowKind::Weekly => weekly = mapped,
                    WindowKind::Monthly => monthly = mapped,
                }
            }
            crate::debug_log!(
                "[codex] mapped: five_hour={:?} weekly={:?} monthly={:?}",
                five_hour,
                weekly,
                monthly
            );

            ToolUsage {
                tool: "codex".into(),
                status: ToolStatus::Ok,
                five_hour,
                weekly,
                monthly,
                note: None,
                source: DataSource::Live,
                fetched_at: Utc::now(),
                message: None,
            }
        }
        Err(e) if e == "unauthorized" => {
            ToolUsage::auth_expired("codex", "Codex session expired — run `codex login`")
        }
        Err(e) => ToolUsage::error("codex", format!("Couldn't reach OpenAI ({e})")),
    }
}
