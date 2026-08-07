use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

use crate::models::AllUsage;
use crate::providers::{claude, codex, cursor};
use crate::store::Store;

/// A glance tool, not a real-time dashboard — poll every few minutes rather
/// than aggressively, per the brief.
const POLL_INTERVAL_SECS: u64 = 5 * 60;

pub async fn refresh_all(app: &AppHandle) -> AllUsage {
    let (claude_usage, codex_usage, cursor_usage) =
        tokio::join!(claude::refresh(), codex::refresh(), cursor::refresh());

    let store = app.state::<Store>();
    store.update("claude", claude_usage);
    store.update("codex", codex_usage);
    store.update("cursor", cursor_usage);

    let snapshot = store.snapshot();
    let _ = app.emit("usage-updated", &snapshot);
    snapshot
}

/// Sleeps before the first fetch rather than firing immediately: the
/// frontend already triggers one fetch on load (`refresh_now`), and undoing
/// that redundancy matters here specifically because Claude's usage endpoint
/// is aggressively rate-limited — two near-simultaneous calls to it on every
/// launch was enough to trip a 429.
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_SECS)).await;
            refresh_all(&app).await;
        }
    });
}
