use std::time::Duration;

/// A `reqwest::Client` with an explicit request timeout. `reqwest::Client`
/// has no default timeout at all — without this, a single hung request
/// (waking from sleep, a stalled VPN reconnect, a firewall silently
/// dropping packets) could block a poll cycle forever, since the scheduler
/// loop `.await`s each cycle in sequence.
pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}
