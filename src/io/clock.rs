#![allow(clippy::disallowed_methods)]

use jiff::Timestamp;
use jiff::tz::TimeZone;

pub async fn sleep(duration: std::time::Duration) {
    tokio::time::sleep(duration).await;
}

/// Keep asynchronous time limits behind the same I/O boundary as clock reads.
pub async fn timeout<F: Future>(
    duration: std::time::Duration,
    future: F,
) -> Result<F::Output, tokio::time::error::Elapsed> {
    tokio::time::timeout(duration, future).await
}

/// `PALEPALE_FAKE_NOW` (RFC 3339) pins the clock so tests can exercise expiry without sleeping.
pub fn now() -> Timestamp {
    crate::settings::env("PALEPALE_FAKE_NOW")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(Timestamp::now)
}

/// The user's local date, e.g. `2026-09-28`.
pub fn today() -> String {
    now().to_zoned(TimeZone::system()).date().to_string()
}

/// A monotonic instant, for measuring durations.
pub fn instant() -> std::time::Instant {
    std::time::Instant::now()
}
