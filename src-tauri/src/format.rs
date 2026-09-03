use chrono::{DateTime, Duration, Local, Utc};

/// A reset-time label that can't be misread at a glance.
///
/// The trigger for this: a rolling 7-day window that happens to reset on the
/// same weekday name it currently is — a bare "Wed" shown on a Wednesday
/// reads as "later today" when it actually means days away. So weekday names
/// are never used: a reset later *today* shows a clock time, and anything on
/// a later day shows an explicit calendar date ("Sep 6"), the same form
/// Cursor's monthly reset already uses.
pub fn reset_label(resets_at: DateTime<Utc>) -> String {
    let now = Utc::now();
    if resets_at <= now {
        return "now".to_string();
    }

    let local = resets_at.with_timezone(&Local);
    let days = local
        .date_naive()
        .signed_duration_since(Local::now().date_naive())
        .num_days();

    match days {
        0 => clock(local),
        // A short rolling window (Claude/Codex 5-hour) that rolls just past
        // midnight is still "soon" — a clock time reads better than a date.
        1 if resets_at - now <= Duration::hours(8) => clock(local),
        _ => local.format("%b %-d").to_string(),
    }
}

fn clock(dt: DateTime<Local>) -> String {
    dt.format("%-I:%M%p").to_string().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label_in(d: Duration) -> String {
        reset_label(Utc::now() + d)
    }

    #[test]
    fn past_or_now_reads_as_now() {
        assert_eq!(label_in(Duration::seconds(-5)), "now");
    }

    #[test]
    fn same_day_reads_as_a_clock_time() {
        // A 5-hour window resets within hours — show the time of day.
        let label = label_in(Duration::hours(3));
        assert!(
            label.contains(':') && (label.ends_with("am") || label.ends_with("pm")),
            "expected a clock time, got {label:?}"
        );
    }

    fn assert_is_date(label: &str) {
        let weekdays = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
        assert!(
            !weekdays.contains(&label),
            "a later day should be an explicit date, got a bare weekday {label:?}"
        );
        assert!(
            label.chars().next().unwrap().is_alphabetic()
                && label.chars().any(|c| c.is_ascii_digit()),
            "expected a month/day date like \"Sep 6\", got {label:?}"
        );
    }

    #[test]
    fn a_few_days_out_is_a_date_not_a_weekday() {
        // The reported case: Claude's weekly window showing "Tue" — it should
        // read like Cursor's "Sep 6" instead.
        assert_is_date(&label_in(Duration::days(3) + Duration::hours(2)));
    }

    #[test]
    fn a_week_out_is_a_date() {
        assert_is_date(&label_in(Duration::days(7)));
    }
}
