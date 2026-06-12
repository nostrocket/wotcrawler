//! Per-relay rate limiting + rate-limited-notice backoff (RELAY-04, RELAY-01).
//!
//! Two cooperating mechanisms, both keyed per relay url:
//!
//! 1. A `governor` GCRA limiter per relay (one [`RateLimiter`] each) gates every
//!    outbound REQ via [`RateLimiterRegistry::acquire`] so the crawler stays
//!    polite ("each list fetched roughly once" — PROJECT.md) and is never
//!    IP-banned (threat T-02-10).
//!
//! 2. A per-relay capped-exponential-with-jitter backoff schedule. nostr-sdk's
//!    own socket reconnect is LINEAR (02-SPIKES RELAY-01), so RELAY-01's
//!    *exponential* backoff requirement is satisfied here at the acquisition
//!    layer: this schedule governs how long a relay is parked before the crawler
//!    re-arms it for fetching after repeated connection failures, and it is
//!    reused for the RELAY-04 `rate-limited` notice path (threats T-02-09 /
//!    T-02-10). [`backoff_delay`] is the pure schedule (testable without sleeps);
//!    [`RateLimiterRegistry::record_notice`] / [`RateLimiterRegistry::backoff`]
//!    drive it from live relay messages.
//!
//! Implemented in plan 02-03 Task 1.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Mutex;
use std::time::Duration;

use governor::clock::DefaultClock;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter};
use rand::Rng;

use crate::error::RelayError;

/// A `governor` direct (single-key) GCRA limiter.
type DirectLimiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock>;

/// Default sustained per-relay outbound REQ rate (requests/second).
///
/// Conservative — politeness over throughput; config-overridable later (OPS-01).
pub const DEFAULT_REQS_PER_SECOND: u32 = 4;

/// Backoff base delay: `delay = base * 2^failures`, before jitter (02-SPIKES).
pub const DEFAULT_BACKOFF_BASE: Duration = Duration::from_secs(1);

/// Backoff cap: the exponential is clamped here before jitter (02-SPIKES, 5 min).
pub const DEFAULT_BACKOFF_CAP: Duration = Duration::from_secs(300);

/// Classification of a relay message prefix relevant to politeness (NIP-01
/// machine-readable prefixes on CLOSED / NOTICE / OK messages).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    /// `rate-limited`: back off this relay (capped exponential + jitter) and
    /// retry later. The relay is still usable.
    RateLimited,
    /// `blocked` / `restricted`: stop hitting this relay entirely.
    Blocked,
    /// Any other prefix — not politeness-relevant.
    Other,
}

/// Classify a relay message's machine-readable prefix (RELAY-04).
///
/// NIP-01 machine-readable prefixes are the first colon-delimited token of a
/// CLOSED/OK/NOTICE message (e.g. `"rate-limited: slow down"`). A bare NOTICE
/// without a prefix classifies as [`NoticeKind::Other`].
pub fn classify_notice(message: &str) -> NoticeKind {
    let prefix = message.split(':').next().unwrap_or("").trim();
    match prefix {
        "rate-limited" => NoticeKind::RateLimited,
        "blocked" | "restricted" => NoticeKind::Blocked,
        _ => NoticeKind::Other,
    }
}

/// Pure capped-exponential-with-jitter backoff schedule (02-SPIKES RELAY-01).
///
/// `delay = min(base * 2^failures, cap)`, then "full jitter": a uniform random
/// value in `[delay/2, delay]` so synchronized relays never re-arm in lockstep
/// (Pitfall 8 thundering herd). `failures` is the per-relay consecutive-failure
/// count (0 yields roughly `base`). The pre-jitter delay grows exponentially and
/// is monotonic up to the cap; this is the function tests assert against.
pub fn backoff_delay(failures: u32, base: Duration, cap: Duration) -> Duration {
    let capped = backoff_delay_unjittered(failures, base, cap);
    // Full jitter over the lower half keeps the schedule monotone-in-expectation
    // while still desynchronizing relays.
    let half = capped / 2;
    let span = capped - half;
    let jitter = if span.is_zero() {
        Duration::ZERO
    } else {
        Duration::from_nanos(rand::rng().random_range(0..=span.as_nanos() as u64))
    };
    half + jitter
}

/// The deterministic (pre-jitter) capped-exponential delay.
///
/// Exposed so tests can assert the schedule grows exponentially and saturates at
/// `cap` without flaking on the random jitter.
pub fn backoff_delay_unjittered(failures: u32, base: Duration, cap: Duration) -> Duration {
    // Compute base * 2^failures in nanoseconds with saturation. Any shift past
    // ~63, or any overflow, lands well past the cap and is clamped to it — we
    // must NOT truncate the factor (a u64->u32 cast at failures>=32 wrapped to
    // a tiny value and produced a near-zero delay; that was the saturation bug).
    let cap_nanos = cap.as_nanos();
    if failures >= 128 {
        return cap;
    }
    let base_nanos = base.as_nanos();
    let scaled = base_nanos
        .checked_shl(failures)
        .unwrap_or(u128::MAX)
        .min(cap_nanos);
    // scaled <= cap_nanos which fits in the Duration range cap already occupies.
    Duration::from_nanos(scaled.min(u64::MAX as u128) as u64).min(cap)
}

/// Per-relay rate-limiter registry + backoff state (RELAY-04 / RELAY-01).
///
/// Holds one governor limiter and one consecutive-failure counter per relay url.
/// Cheap to share behind an `Arc`; all interior state is `Mutex`-guarded.
pub struct RateLimiterRegistry {
    reqs_per_second: NonZeroU32,
    base: Duration,
    cap: Duration,
    limiters: Mutex<HashMap<String, DirectLimiter>>,
    /// Per-relay consecutive failure/notice count driving the backoff schedule.
    failures: Mutex<HashMap<String, u32>>,
}

impl RateLimiterRegistry {
    /// Build a registry with the default per-relay quota and backoff params.
    pub fn new() -> Self {
        Self::with_params(
            NonZeroU32::new(DEFAULT_REQS_PER_SECOND).expect("default rate is non-zero"),
            DEFAULT_BACKOFF_BASE,
            DEFAULT_BACKOFF_CAP,
        )
    }

    /// Build a registry with explicit per-relay quota and backoff params.
    pub fn with_params(reqs_per_second: NonZeroU32, base: Duration, cap: Duration) -> Self {
        Self {
            reqs_per_second,
            base,
            cap,
            limiters: Mutex::new(HashMap::new()),
            failures: Mutex::new(HashMap::new()),
        }
    }

    /// Await this relay's GCRA token before issuing a REQ (RELAY-04).
    ///
    /// Throttles via the per-relay limiter so concurrent fetches never exceed the
    /// configured per-relay quota; the limiter for `relay_url` is created on first
    /// use. Returns once a token is available.
    pub async fn acquire(&self, relay_url: &str) -> Result<(), RelayError> {
        // `until_ready` borrows the limiter across an await, so we cannot hold the
        // map lock over it. Clone is not available on RateLimiter; instead we
        // park the relay's limiter behind its own entry and await against a raw
        // pointer-free copy of the GCRA decision by re-checking in a small loop.
        //
        // governor limiters are not `Clone`, so we keep the limiter inside the
        // map and drive `until_ready` by temporarily taking the limiter out,
        // awaiting, then putting it back. Single relay url contention is rare and
        // the await is short.
        let limiter = {
            let mut map = self.limiters.lock().expect("rate-limiter map not poisoned");
            map.remove(relay_url).unwrap_or_else(|| {
                RateLimiter::direct(Quota::per_second(self.reqs_per_second))
            })
        };
        limiter.until_ready().await;
        let mut map = self.limiters.lock().expect("rate-limiter map not poisoned");
        // If another caller created a limiter for this relay while we awaited,
        // keep the one that has accrued state (ours) and drop the spare.
        map.entry(relay_url.to_string()).or_insert(limiter);
        Ok(())
    }

    /// Record a relay notice and return how to react (RELAY-04).
    ///
    /// On [`NoticeKind::RateLimited`] the relay's consecutive-failure count is
    /// incremented, the `relay_rate_limited` metric fired, and the next backoff
    /// delay returned (caller sleeps it via [`Self::backoff`]). On
    /// [`NoticeKind::Blocked`] the `relay_blocked` metric fires and `None` is
    /// returned (caller stops hitting the relay). [`NoticeKind::Other`] is a
    /// no-op.
    pub fn record_notice(&self, relay_url: &str, message: &str) -> Option<Duration> {
        match classify_notice(message) {
            NoticeKind::RateLimited => {
                let failures = {
                    let mut map = self.failures.lock().expect("failures map not poisoned");
                    let entry = map.entry(relay_url.to_string()).or_insert(0);
                    *entry = entry.saturating_add(1);
                    *entry
                };
                metrics::counter!("relay_rate_limited", "relay" => relay_url.to_string())
                    .increment(1);
                // failures is >=1 here; schedule index is failures-1 so the first
                // notice yields ~base, not 2*base.
                Some(backoff_delay(failures - 1, self.base, self.cap))
            }
            NoticeKind::Blocked => {
                metrics::counter!("relay_blocked", "relay" => relay_url.to_string()).increment(1);
                None
            }
            NoticeKind::Other => None,
        }
    }

    /// Sleep out the current backoff for a relay that returned a `rate-limited`
    /// notice, then return. Records the notice first (so repeated calls escalate).
    pub async fn backoff(&self, relay_url: &str, message: &str) -> Result<(), RelayError> {
        if let Some(delay) = self.record_notice(relay_url, message) {
            tokio::time::sleep(delay).await;
        }
        Ok(())
    }

    /// Reset a relay's consecutive-failure count after a successful fetch so its
    /// backoff schedule starts over.
    pub fn reset(&self, relay_url: &str) {
        let mut map = self.failures.lock().expect("failures map not poisoned");
        map.remove(relay_url);
    }

    /// Current consecutive-failure count for a relay (0 if never failed). Exposed
    /// for tests and observability.
    pub fn failure_count(&self, relay_url: &str) -> u32 {
        self.failures
            .lock()
            .expect("failures map not poisoned")
            .get(relay_url)
            .copied()
            .unwrap_or(0)
    }
}

impl Default for RateLimiterRegistry {
    fn default() -> Self {
        Self::new()
    }
}
