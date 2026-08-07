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
    if resp.status().as_u16() == 401 {
        return Err("unauthorized".into());
    }
    if !resp.status().is_success() {
        return Err(format!("http {}", resp.status()));
    }
    resp.json::<UsageResponse>().await.map_err(|e| e.to_string())
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

fn to_window(resp: &WindowSnapshot) -> Option<UsageWindow> {
    let used_pct = resp.used_percent?;
    let resets_at = resp.reset_at.and_then(|secs| Utc.timestamp_opt(secs, 0).single());
    Some(UsageWindow {
        used_pct: used_pct as f32,
        resets_label: resets_at.map(reset_label).unwrap_or_default(),
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
        return ToolUsage::not_logged_in("codex", "Not logged in — run `codex login`");
    };

    match fetch_live(&cred).await {
        Ok(resp) => {
            let rate_limit = resp.rate_limit.unwrap_or(RateLimit {
                primary_window: None,
                secondary_window: None,
            });
            ToolUsage {
                tool: "codex".into(),
                status: ToolStatus::Ok,
                five_hour: rate_limit.primary_window.as_ref().and_then(to_window),
                weekly: rate_limit.secondary_window.as_ref().and_then(to_window),
                monthly: None,
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
