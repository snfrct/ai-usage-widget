use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use tauri::{AppHandle, Manager};

use crate::models::AllUsage;

/// Persists the last-known-good usage snapshot for each tool to disk so the
/// popover has something to show instantly on launch, before the first live
/// fetch completes (or if a live fetch fails).
///
/// This never holds any credential — no token from any of the three tools is
/// ever written to disk by this app. Each provider re-reads its source of
/// truth (keychain / credential file / Cursor's own SQLite db) on every
/// refresh instead of us caching it ourselves.
pub struct Store {
    cache_path: PathBuf,
    cached: Mutex<AllUsage>,
}

impl Store {
    pub fn new(app: &AppHandle) -> Self {
        let dir = app
            .path()
            .app_data_dir()
            .unwrap_or_else(|_| std::env::temp_dir());
        let _ = fs::create_dir_all(&dir);
        let cache_path = dir.join("usage-cache.json");
        let cached = Self::read_from_disk(&cache_path);
        Self {
            cache_path,
            cached: Mutex::new(cached),
        }
    }

    fn read_from_disk(path: &PathBuf) -> AllUsage {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn snapshot(&self) -> AllUsage {
        self.cached.lock().unwrap().clone()
    }

    /// Applies a fresh provider result. A transient `Error` status doesn't
    /// clobber a previously-good snapshot — the popover keeps showing the
    /// last live numbers rather than flashing an error on every network
    /// hiccup. Deliberate states (`NotLoggedIn`/`AuthExpired`/`Ok`) always
    /// replace, since those reflect the real current credential state.
    pub fn update(&self, tool: &str, usage: crate::models::ToolUsage) {
        {
            let mut all = self.cached.lock().unwrap();
            let slot = match tool {
                "claude" => &mut all.claude,
                "codex" => &mut all.codex,
                "cursor" => &mut all.cursor,
                _ => return,
            };
            let keep_previous = usage.status == crate::models::ToolStatus::Error
                && slot
                    .as_ref()
                    .is_some_and(|prev| prev.status == crate::models::ToolStatus::Ok);
            if !keep_previous {
                *slot = Some(usage);
            }
        }
        self.persist();
    }

    fn persist(&self) {
        let all = self.cached.lock().unwrap();
        if let Ok(json) = serde_json::to_string_pretty(&*all) {
            let _ = fs::write(&self.cache_path, json);
        }
    }
}
