use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

use crate::models::{DataSource, ToolStatus, ToolUsage, UsageWindow};

/// Rough, undocumented-formula baselines used only to turn a raw recent-request
/// count into *something* resembling a percentage when there is no live data
/// and no cache at all (e.g. first run, fully offline). This is explicitly a
/// best-effort fallback, not a real quota calculation — see README.
const FIVE_HOUR_BASELINE_REQUESTS: f32 = 50.0;
const WEEKLY_BASELINE_REQUESTS: f32 = 250.0;

fn claude_projects_dir() -> PathBuf {
    let root = std::env::var("CLAUDE_CONFIG_DIR")
        .ok()
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".claude"));
    root.join("projects")
}

#[derive(Debug, Deserialize)]
struct LogLine {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(rename = "type", default)]
    line_type: Option<String>,
}

/// Counts assistant-turn entries across all session transcripts newer than
/// `since`. Best-effort: any parse failure for a line/file is skipped rather
/// than surfaced, since this is only ever a secondary signal.
fn count_recent_entries(since: DateTime<Utc>) -> u32 {
    let dir = claude_projects_dir();
    let mut count = 0u32;
    let Ok(project_entries) = fs::read_dir(&dir) else {
        return 0;
    };
    for project in project_entries.flatten() {
        let path = project.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(files) = fs::read_dir(&path) else {
            continue;
        };
        for file in files.flatten() {
            let file_path = file.path();
            if file_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(content) = fs::read_to_string(&file_path) else {
                continue;
            };
            // Recent activity clusters at the end of the file; capping the scan
            // keeps this cheap even for long-running project sessions.
            for line in content.lines().rev().take(2000) {
                let Ok(entry) = serde_json::from_str::<LogLine>(line) else {
                    continue;
                };
                if entry.line_type.as_deref() != Some("assistant") {
                    continue;
                }
                let Some(ts) = entry
                    .timestamp
                    .as_ref()
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                else {
                    continue;
                };
                if ts.with_timezone(&Utc) >= since {
                    count += 1;
                } else {
                    // Lines are chronological, so once we're past the window
                    // (scanning newest-first) nothing older in this file matters.
                    break;
                }
            }
        }
    }
    count
}

/// Returns `None` when there's no local activity signal at all, so the caller
/// can fall back to a plain "not logged in" message instead of a fabricated
/// zero.
pub fn claude_estimate() -> Option<ToolUsage> {
    let now = Utc::now();
    let five_hour_count = count_recent_entries(now - Duration::hours(5));
    let weekly_count = count_recent_entries(now - Duration::days(7));
    if five_hour_count == 0 && weekly_count == 0 {
        return None;
    }

    let five_hour_pct = ((five_hour_count as f32 / FIVE_HOUR_BASELINE_REQUESTS) * 100.0).min(100.0);
    let weekly_pct = ((weekly_count as f32 / WEEKLY_BASELINE_REQUESTS) * 100.0).min(100.0);

    Some(ToolUsage {
        tool: "claude".into(),
        status: ToolStatus::Ok,
        five_hour: Some(UsageWindow {
            used_pct: five_hour_pct,
            resets_label: "~5h".into(),
            resets_at: None,
        }),
        weekly: Some(UsageWindow {
            used_pct: weekly_pct,
            resets_label: "~wk".into(),
            resets_at: None,
        }),
        monthly: None,
        note: None,
        source: DataSource::LocalEstimate,
        fetched_at: now,
        message: None,
        stale: false,
    })
}
