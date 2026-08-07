use std::path::PathBuf;
use std::process::{Command, Stdio};

use chrono::{DateTime, Local, Utc};
use serde::Deserialize;

use crate::models::{DataSource, ToolStatus, ToolUsage, UsageWindow};
use crate::providers::local_estimate;

/// macOS Keychain service name Claude Code itself writes to (same one its own
/// `/usage` command reads) — see `security find-generic-password -s "Claude Code-credentials" -w`.
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

#[derive(Debug, Deserialize)]
struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<OAuthCreds>,
}

#[derive(Debug, Deserialize, Clone)]
struct OAuthCreds {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "refreshToken")]
    refresh_token: Option<String>,
    /// Milliseconds since epoch.
    #[serde(rename = "expiresAt")]
    expires_at: Option<f64>,
}

fn config_root() -> PathBuf {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    dirs::home_dir().unwrap_or_default().join(".claude")
}

#[cfg(target_os = "macos")]
fn read_from_keychain() -> Option<OAuthCreds> {
    let output = Command::new("security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8(output.stdout).ok()?;
    let parsed: CredentialsFile = serde_json::from_str(raw.trim()).ok()?;
    parsed.claude_ai_oauth
}

fn read_from_file() -> Option<OAuthCreds> {
    let path = config_root().join(".credentials.json");
    let content = std::fs::read_to_string(path).ok()?;
    let parsed: CredentialsFile = serde_json::from_str(&content).ok()?;
    parsed.claude_ai_oauth
}

/// Claude Code itself prioritizes this env var (set via `claude setup-token`)
/// over the Keychain/file credential — a long-lived token meant for exactly
/// this kind of non-interactive use. We match that priority order. Note
/// this only has any effect when the widget is launched from a shell that
/// has the var exported (e.g. via Terminal) — a normal double-click/Finder
/// launch doesn't inherit shell environment variables at all, which is a
/// macOS launch-environment limitation, not something this app controls.
fn read_from_env() -> Option<OAuthCreds> {
    let token = std::env::var("CLAUDE_CODE_OAUTH_TOKEN").ok()?;
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    eprintln!("[claude] using CLAUDE_CODE_OAUTH_TOKEN from environment");
    Some(OAuthCreds {
        access_token: token.to_string(),
        refresh_token: None,
        expires_at: None,
    })
}

fn read_credential() -> Option<OAuthCreds> {
    if let Some(creds) = read_from_env() {
        return Some(creds);
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(creds) = read_from_keychain() {
            return Some(creds);
        }
    }
    read_from_file()
}

/// Claude Code CLI's own OAuth client ID — a public identifier, not a
/// secret, used here only for the narrow "refresh an already-granted
/// session" operation (not a full interactive OAuth flow, which is the thing
/// the brief explicitly said not to build ourselves).
const OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const TOKEN_REFRESH_ENDPOINT: &str = "https://platform.claude.com/v1/oauth/token";

#[derive(Debug, Deserialize)]
struct TokenRefreshResponse {
    access_token: String,
}

/// Refreshes an expired access token using its refresh token. The result is
/// used in-memory for this fetch only — never written back to Claude Code's
/// own keychain entry or credentials file, since that store belongs to the
/// CLI, not us.
async fn refresh_access_token(refresh_token: &str) -> Result<String, String> {
    let client = reqwest::Client::new();
    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", OAUTH_CLIENT_ID),
    ];
    let resp = client
        .post(TOKEN_REFRESH_ENDPOINT)
        .form(&params)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        eprintln!("[claude] token refresh endpoint returned http {status}: {body}");
        return Err(format!("http {status}"));
    }
    resp.json::<TokenRefreshResponse>()
        .await
        .map(|r| r.access_token)
        .map_err(|e| e.to_string())
}

/// True once the token is expired or within a short buffer of expiring.
/// `None` (no `expiresAt` in the credential) is treated as "unknown, don't
/// assume expired" — the live call's own 401 handling covers that case.
fn is_expired(creds: &OAuthCreds) -> bool {
    match creds.expires_at {
        Some(expires_at_ms) => {
            let now_ms = Utc::now().timestamp_millis() as f64;
            now_ms >= expires_at_ms - 30_000.0
        }
        None => false,
    }
}

#[derive(Debug, Deserialize)]
struct UsageWindowResponse {
    utilization: Option<f64>,
    resets_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UsageResponse {
    five_hour: Option<UsageWindowResponse>,
    seven_day: Option<UsageWindowResponse>,
}

async fn fetch_live(token: &str) -> Result<UsageResponse, String> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://api.anthropic.com/api/oauth/usage")
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header("anthropic-beta", "oauth-2025-04-20")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        eprintln!("[claude] usage endpoint returned http {status}: {body}");
        if status.as_u16() == 401 {
            return Err("unauthorized".into());
        }
        return Err(format!("http {status}"));
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

fn to_window(resp: &UsageWindowResponse) -> Option<UsageWindow> {
    // `utilization` is already on a 0-100 scale, not a 0-1 fraction —
    // confirmed against Anthropic's own field usage (no further scaling
    // applied before display). Multiplying by 100 here previously inflated
    // every reading 100x (e.g. a real 1% showed as 100%).
    let utilization = resp.utilization?;
    let resets_at = resp
        .resets_at
        .as_ref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc));
    Some(UsageWindow {
        used_pct: utilization as f32,
        resets_label: resets_at.map(reset_label).unwrap_or_default(),
        resets_at,
    })
}

/// Shells out to `claude login`, which opens the browser and, once the user
/// finishes, writes its own credential file/keychain entry — we never
/// implement OAuth ourselves. Runs detached; the next scheduled refresh
/// picks up the new credential.
#[tauri::command]
pub fn claude_login() -> Result<(), String> {
    Command::new("claude")
        .arg("login")
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Couldn't launch `claude login`: {e}"))
}

fn usage_from_response(resp: UsageResponse) -> ToolUsage {
    ToolUsage {
        tool: "claude".into(),
        status: ToolStatus::Ok,
        five_hour: resp.five_hour.as_ref().and_then(to_window),
        weekly: resp.seven_day.as_ref().and_then(to_window),
        monthly: None,
        note: None,
        source: DataSource::Live,
        fetched_at: Utc::now(),
        message: None,
    }
}

pub async fn refresh() -> ToolUsage {
    let Some(creds) = read_credential() else {
        eprintln!("[claude] no credential found (keychain and file both empty/unparsable)");
        return match local_estimate::claude_estimate() {
            Some(mut usage) => {
                usage.message = Some("Not logged in — showing a rough local estimate. Run `claude login` for live numbers.".into());
                usage
            }
            None => ToolUsage::not_logged_in("claude", "Not logged in — run `claude login`"),
        };
    };
    eprintln!(
        "[claude] credential found: access_token_len={} has_refresh_token={} expires_at={:?} is_expired={}",
        creds.access_token.len(),
        creds.refresh_token.is_some(),
        creds.expires_at,
        is_expired(&creds)
    );

    // Access tokens are short-lived and Claude Code CLI normally refreshes
    // its own silently whenever it runs — but nothing prompts that refresh
    // if this widget is the only thing reading the credential. Refresh
    // proactively when we can tell locally, so most polls never hit a 401
    // at all. This is purely an optimization, though: if the local expiry
    // read or the refresh call itself is wrong for any reason, we must not
    // let that override the actual access token — fall through and let the
    // real live call (with its own reactive refresh-and-retry below) be the
    // only thing that can ever declare the session actually expired.
    let mut access_token = creds.access_token.clone();
    if is_expired(&creds) {
        if let Some(refresh_token) = &creds.refresh_token {
            match refresh_access_token(refresh_token).await {
                Ok(fresh_token) => {
                    eprintln!("[claude] proactive refresh succeeded");
                    access_token = fresh_token;
                }
                Err(e) => eprintln!("[claude] proactive refresh failed: {e}"),
            }
        }
    }

    match fetch_live(&access_token).await {
        Ok(resp) => {
            eprintln!("[claude] fetch_live succeeded");
            usage_from_response(resp)
        }
        Err(e) if e == "unauthorized" => {
            eprintln!("[claude] fetch_live returned 401; attempting reactive refresh-and-retry");
            // Reactive fallback: the token looked valid locally but the
            // server disagreed (e.g. revoked early). Try one refresh-and-retry
            // before giving up.
            if let Some(refresh_token) = &creds.refresh_token {
                match refresh_access_token(refresh_token).await {
                    Ok(fresh_token) => match fetch_live(&fresh_token).await {
                        Ok(resp) => {
                            eprintln!("[claude] reactive refresh-and-retry succeeded");
                            return usage_from_response(resp);
                        }
                        Err(e) => eprintln!("[claude] retry after reactive refresh still failed: {e}"),
                    },
                    Err(e) => eprintln!("[claude] reactive refresh failed: {e}"),
                }
            } else {
                eprintln!("[claude] no refresh_token available for reactive refresh");
            }
            ToolUsage::auth_expired("claude", "Claude session expired — run `claude login`")
        }
        Err(e) => {
            eprintln!("[claude] fetch_live failed (non-401): {e}");
            ToolUsage::error("claude", format!("Couldn't reach Anthropic ({e})"))
        }
    }
}
