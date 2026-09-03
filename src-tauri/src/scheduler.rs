use std::time::{Duration, SystemTime};

use tauri::{AppHandle, Emitter, Manager};

use crate::models::AllUsage;
use crate::providers::{claude, codex, cursor};
use crate::store::Store;

/// A glance tool, not a real-time dashboard — poll every few minutes rather
/// than aggressively, per the brief. 30 minutes specifically because Claude's
/// and Codex's usage endpoints are undocumented and rate-limited in ways
/// that aren't publicly specified; a slower cadence means fewer chances to
/// trip them in the first place.
const POLL_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// How often the loop wakes to check whether a poll is due. Short so the
/// widget recovers within a minute of the machine waking from sleep, rather
/// than waiting out the remainder of a 30-minute timer that was frozen the
/// whole time the lid was closed (see `start`).
const TICK: Duration = Duration::from_secs(60);

/// Generous upper bound on a whole cycle (all three providers, including
/// Claude's worst case of up to four sequential HTTP calls at 15s each).
/// This is a second, broader safety net beyond the per-request HTTP
/// timeouts — it also covers anything an HTTP timeout wouldn't, like the
/// synchronous `security` Keychain CLI calls, which have no timeout of
/// their own and could in principle hang on a permission-prompt dialog.
const CYCLE_TIMEOUT_SECS: u64 = 90;

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

/// Waits out a full interval before the first fetch rather than firing
/// immediately: the frontend already triggers one fetch on load
/// (`refresh_now`), and undoing that redundancy matters here specifically
/// because Claude's usage endpoint is aggressively rate-limited — two
/// near-simultaneous calls to it on every launch was enough to trip a 429.
///
/// Due-ness is tracked against the wall clock (`SystemTime`), not a single
/// long `tokio::time::sleep`. A monotonic sleep is *frozen* while macOS is
/// asleep, so a 30-minute sleep begun before a two-hour lid-close only fires
/// 30 minutes after waking — the widget just sits at "Updated Xh ago" the
/// whole time with nothing visibly wrong. Waking on a short `TICK` and
/// comparing wall-clock elapsed means a wake is noticed as "we're overdue"
/// within one tick, and the widget refreshes on its own.
///
/// Each poll cycle still runs in its own spawned task rather than inline in
/// this loop. That's not just style: this loop runs for the entire lifetime
/// of the app, so if a single cycle ever panicked — anywhere, for any
/// reason, including a bug we haven't hit yet — an inline call would take
/// the whole loop down with it, permanently, with nothing visibly wrong
/// (the rest of the UI keeps running, it just silently stops refreshing
/// forever). Spawning isolates a panic to that one cycle; we log it and
/// keep polling.
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut last_poll = SystemTime::now();
        loop {
            tokio::time::sleep(TICK).await;

            let overdue = SystemTime::now()
                .duration_since(last_poll)
                .map(|elapsed| elapsed >= POLL_INTERVAL)
                .unwrap_or(true); // clock moved backwards — just poll
            if !overdue {
                continue;
            }
            last_poll = SystemTime::now();

            let app_for_cycle = app.clone();
            let handle = tauri::async_runtime::spawn(async move {
                refresh_all(&app_for_cycle).await;
            });
            match tokio::time::timeout(Duration::from_secs(CYCLE_TIMEOUT_SECS), handle).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    crate::debug_log!("[scheduler] a poll cycle failed unexpectedly and was recovered: {e:?}");
                }
                Err(_) => {
                    crate::debug_log!(
                        "[scheduler] a poll cycle exceeded {CYCLE_TIMEOUT_SECS}s and was abandoned; resuming normal polling"
                    );
                }
            }
        }
    });
}
