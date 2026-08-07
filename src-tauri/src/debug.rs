pub fn enabled() -> bool {
    std::env::var("AI_USAGE_WIDGET_DEBUG").is_ok()
}

/// Prints only when `AI_USAGE_WIDGET_DEBUG` is set, so a normal launch stays
/// quiet — verbose per-provider diagnostics (credential state, HTTP
/// statuses, raw response bodies) are opt-in, not printed by default.
#[macro_export]
macro_rules! debug_log {
    ($($arg:tt)*) => {
        if $crate::debug::enabled() {
            eprintln!($($arg)*);
        }
    };
}
