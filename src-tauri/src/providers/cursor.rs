use std::path::PathBuf;

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;

use crate::models::{DataSource, ToolStatus, ToolUsage, UsageWindow};

/// Cursor stores its own session in a VS Code-style SQLite key/value store.
/// This file is not locked while Cursor is running, so a read-only open
/// alongside a live Cursor instance is safe. See README for the full
/// disclosure — this is the most fragile integration in the app.
fn db_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    #[cfg(target_os = "macos")]
    {
        Some(home.join("Library/Application Support/Cursor/User/globalStorage/state.vscdb"))
    }
    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var("APPDATA").ok()?;
        Some(PathBuf::from(appdata).join("Cursor/User/globalStorage/state.vscdb"))
    }
    #[cfg(target_os = "linux")]
    {
        let config = std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".config"));
        Some(config.join("Cursor/User/globalStorage/state.vscdb"))
    }
}

/// The Cursor IDE's own session token, read from its local SQLite store.
fn read_ide_token() -> Option<String> {
    let path = db_path()?;
    if !path.exists() {
        return None;
    }
    let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    conn.query_row(
        "SELECT value FROM ItemTable WHERE key = ? LIMIT 1;",
        ["cursorAuth/accessToken"],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .filter(|v| !v.is_empty())
}

/// The `cursor-agent` CLI (`agent login` / `CURSOR_API_KEY`) keeps its own,
/// separate credential — an opaque `crsr_...` API key, not a JWT — at one of
/// these paths depending on version/platform. None of this is documented by
/// Cursor; the paths and field names below come from community
/// reverse-engineering (strace against the actual binary), not an official
/// source, so this is best-effort and may need updating on a CLI update.
fn cli_auth_paths() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    vec![
        home.join(".config/cursor/auth.json"),
        home.join(".config/cursor/credentials.json"),
        home.join(".cursor/credentials.json"),
    ]
}

#[derive(Debug, Deserialize)]
struct CliCredentialFile {
    #[serde(
        rename = "accessToken",
        alias = "access_token",
        alias = "token",
        alias = "apiKey",
        alias = "api_key"
    )]
    access_token: Option<String>,
}

fn read_cli_token() -> Option<String> {
    for path in cli_auth_paths() {
        if !path.exists() {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(parsed) = serde_json::from_str::<CliCredentialFile>(&content) else {
            continue;
        };
        if let Some(token) = parsed.access_token.filter(|t| !t.is_empty()) {
            return Some(token);
        }
    }
    None
}

/// Cursor's access token is a JWT. We don't verify its signature — we're not
/// trusting it as a security boundary, just reusing the same logged-in
/// session Cursor itself already established, to derive a web session cookie.
fn decode_jwt_payload(token: &str) -> Option<serde_json::Value> {
    let segment = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(segment)
        .or_else(|_| STANDARD.decode(pad_base64(segment)))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn pad_base64(s: &str) -> String {
    let mut s = s.replace('-', "+").replace('_', "/");
    while s.len() % 4 != 0 {
        s.push('=');
    }
    s
}

fn user_id_from_token(token: &str) -> Option<String> {
    let payload = decode_jwt_payload(token)?;
    let sub = payload.get("sub")?.as_str()?;
    Some(sub.rsplit('|').next().unwrap_or(sub).to_string())
}

/// `cursor.com`'s first-party web session cookie, derived from the local app
/// token rather than calling the private `api2.cursor.sh` RPC directly.
fn cookie_header(user_id: &str, access_token: &str) -> String {
    format!("WorkosCursorSessionToken={user_id}%3A%3A{access_token}")
}

#[derive(Debug, Deserialize)]
struct UsageSummary {
    #[serde(rename = "billingCycleEnd")]
    billing_cycle_end: Option<String>,
    #[serde(rename = "individualUsage")]
    individual_usage: Option<IndividualUsage>,
}

#[derive(Debug, Deserialize)]
struct IndividualUsage {
    plan: Option<PlanUsage>,
    #[serde(rename = "onDemand")]
    on_demand: Option<OnDemandUsage>,
}

#[derive(Debug, Deserialize)]
struct PlanUsage {
    used: Option<i64>,
    limit: Option<i64>,
    #[serde(rename = "totalPercentUsed")]
    total_percent_used: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct OnDemandUsage {
    used: Option<i64>,
    limit: Option<i64>,
}

async fn fetch_usage_summary_via_cookie(cookie: &str) -> Result<UsageSummary, String> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://cursor.com/api/usage-summary")
        .header("Cookie", cookie)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    parse_usage_response(resp).await
}

/// Best-effort: tries the CLI's opaque API key as a bearer token against the
/// same web-session endpoint the IDE uses. Cursor doesn't document a usage
/// endpoint for personal API keys, so this may simply not be authorized —
/// that's fine, it just falls through like any other failed source.
async fn fetch_usage_summary_via_bearer(token: &str) -> Result<UsageSummary, String> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://cursor.com/api/usage-summary")
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    parse_usage_response(resp).await
}

async fn parse_usage_response(resp: reqwest::Response) -> Result<UsageSummary, String> {
    let status = resp.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err("unauthorized".into());
    }
    if !status.is_success() {
        return Err(format!("http {status}"));
    }
    resp.json::<UsageSummary>().await.map_err(|e| e.to_string())
}

fn reset_label(date: &str) -> String {
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .or_else(|_| {
            DateTime::parse_from_rfc3339(date).map(|d| d.date_naive())
        })
        .map(|d| d.format("%b %-d").to_string())
        .unwrap_or_else(|_| date.to_string())
}

fn build_usage(summary: UsageSummary) -> ToolUsage {
    let plan = summary.individual_usage.as_ref().and_then(|u| u.plan.as_ref());
    let used_pct = plan
        .and_then(|p| p.total_percent_used)
        .or_else(|| {
            plan.and_then(|p| match (p.used, p.limit) {
                (Some(used), Some(limit)) if limit > 0 => Some((used as f64 / limit as f64) * 100.0),
                _ => None,
            })
        })
        .unwrap_or(0.0) as f32;

    let resets_label = summary
        .billing_cycle_end
        .as_deref()
        .map(reset_label)
        .unwrap_or_default();

    let on_demand = summary.individual_usage.as_ref().and_then(|u| u.on_demand.as_ref());
    let note = on_demand.and_then(|od| match od.used {
        Some(used) if used > 0 => {
            let used_dollars = used as f64 / 100.0;
            Some(match od.limit {
                Some(limit) if limit > 0 => {
                    format!("+${:.2} on-demand of ${:.2}", used_dollars, limit as f64 / 100.0)
                }
                _ => format!("+${used_dollars:.2} on-demand"),
            })
        }
        _ => None,
    });

    ToolUsage {
        tool: "cursor".into(),
        status: ToolStatus::Ok,
        five_hour: None,
        weekly: None,
        monthly: Some(UsageWindow {
            used_pct,
            resets_label,
            resets_at: None,
        }),
        note,
        source: DataSource::Live,
        fetched_at: Utc::now(),
        message: None,
    }
}

const AUTH_EXPIRED_MESSAGE: &str = "Cursor auth expired — reopen Cursor to refresh";

/// Cursor has no unified login flow to shell out to and no refresh token we
/// can drive programmatically. Per the brief: any failure here — no
/// credential found anywhere, decode failure, or a failed HTTP call —
/// surfaces the same explicit "auth expired" state rather than a stale or
/// fabricated number.
///
/// Two independent local sessions can provide a credential: the Cursor IDE
/// (`state.vscdb`) and the `cursor-agent` CLI (its own local credential
/// file). Both are tried automatically — whichever is present and working
/// wins — so this works whether someone uses the IDE, the CLI, or both. The
/// IDE's JWT-shaped token is used to derive a web session cookie; the CLI's
/// opaque API key is tried as a bearer token instead.
pub async fn refresh() -> ToolUsage {
    let mut candidates = Vec::new();
    if let Some(token) = read_ide_token() {
        candidates.push(token);
    }
    if let Some(token) = read_cli_token() {
        candidates.push(token);
    }

    if candidates.is_empty() {
        return ToolUsage::auth_expired("cursor", AUTH_EXPIRED_MESSAGE);
    }

    for token in candidates {
        let result = if let Some(user_id) = user_id_from_token(&token) {
            fetch_usage_summary_via_cookie(&cookie_header(&user_id, &token)).await
        } else {
            fetch_usage_summary_via_bearer(&token).await
        };
        if let Ok(summary) = result {
            return build_usage(summary);
        }
    }

    ToolUsage::auth_expired("cursor", AUTH_EXPIRED_MESSAGE)
}
