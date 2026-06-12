//! Per-relay rate limiting + rate-limited-notice backoff (RELAY-04).
//!
//! One `governor` GCRA limiter per relay url gates every outbound REQ so the
//! crawler stays polite ("each list fetched roughly once" — PROJECT.md). On a
//! machine-readable `rate-limited` CLOSED/NOTICE prefix, the offending relay is
//! backed off (capped exponential + jitter); `blocked`/`restricted` stops
//! hitting the relay and surfaces a metric.
//!
//! Stub bodies in plan 02-01; implemented in plan 02-03 Task 1/3.

use crate::error::RelayError;

/// Await this relay's rate-limit token before issuing a REQ.
///
/// Throttles via the per-relay GCRA limiter so concurrent fetches never exceed
/// the configured per-relay quota.
pub async fn acquire(_relay_url: &str) -> Result<(), RelayError> {
    todo!("plan 02-03 Task 1: per-relay governor GCRA acquire")
}

/// Apply backoff for a relay that returned a `rate-limited` notice.
///
/// Computes the next capped-exponential delay (with jitter) for `relay_url` and
/// waits it out before the relay is eligible again.
pub async fn backoff(_relay_url: &str) -> Result<(), RelayError> {
    todo!("plan 02-03 Task 3: rate-limited-notice backoff with jitter")
}
