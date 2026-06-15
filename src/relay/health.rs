//! Per-relay continuous health scoring (RELAY-06).
//!
//! A [`RelayHealthRegistry`] holds one EWMA (exponentially-weighted moving
//! average) health score in `[0, 1]` per relay url. It is a PARALLEL registry
//! to [`crate::relay::rate_limit::RateLimiterRegistry`] — same
//! `Mutex<HashMap<String, _>>`-behind-`Arc` shape and the same clone-out /
//! drop-lock discipline — but a deliberately SEPARATE, richer signal: the rate
//! limiter tracks consecutive failures for the backoff schedule, while this
//! registry tracks a smooth health score that drives routing
//! (skip-below-threshold + periodic probe) and per-relay concurrency
//! (permit scaling). The two are never entangled (05-RESEARCH Pitfall 3); the
//! only contact point is the NOTICE consumer, which pings
//! [`RelayHealthRegistry::record_rate_limited`] beside the limiter's
//! `record_notice`.
//!
//! Score semantics (05-RESEARCH Patterns 5-6):
//! - An UNKNOWN relay scores `1.0` (healthy by default) so a never-seen relay
//!   is not skipped before it has had a chance.
//! - A success raises the score, penalized by latency
//!   (`sample = 1 / (1 + latency_secs / LATENCY_SCALE_SECS)`): a fast success
//!   is ~1.0, a slow-but-successful fetch is dragged toward 0.5.
//! - A timeout or connect-failure samples `0.0` (drives the score toward 0).
//! - A rate-limit hit samples `0.2` (degrade, do NOT zero — a rate-limited
//!   relay is still usable, mirroring the `RateLimited` vs `Blocked` split).
//! - EWMA update: `score = alpha * sample + (1 - alpha) * prev`.
//!
//! Routing (Pattern 7): a relay below `relay_health_threshold` is skipped in
//! the fan-out UNLESS a probe is due (its last attempt is older than
//! [`PROBE_INTERVAL`], or it has never been attempted), so a recovered relay
//! periodically gets one request and can climb back.
//!
//! Permits (Pattern 8): `permits = max(1, round(per_relay_concurrency * score))`
//! — a degraded relay gets proportionally fewer concurrent slots but always at
//! least one probe slot. The `in_use` counter is the per-relay
//! concurrency-in-use gauge source and the admission gate 05-04 reads.
//!
//! Implemented in plan 05-02 Task 1.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Default EWMA smoothing factor in `(0, 1]`. Conservative — slower to react
/// (more memory) so a single blip does not whipsaw routing (05-RESEARCH A1).
pub const DEFAULT_HEALTH_ALPHA: f64 = 0.3;

/// Default routing threshold: a relay scoring below this is skipped (except for
/// a periodic probe). Conservative/relay-polite (05-CONTEXT / 05-RESEARCH A1).
pub const DEFAULT_RELAY_HEALTH_THRESHOLD: f64 = 0.25;

/// Default per-relay concurrency cap. Permits scale DOWN from this with health.
pub const DEFAULT_PER_RELAY_CONCURRENCY: usize = 4;

/// Default cap on the number of NIP-65 write relays a fallback fans out to
/// (05-04 routing). Kept conservative to bound relay-side load.
pub const DEFAULT_NIP65_MAX_WRITE_RELAYS: usize = 3;

/// Default for whether the NIP-65 write-relay fallback is enabled at all.
pub const DEFAULT_NIP65_FALLBACK_ENABLED: bool = true;

/// Latency normalization scale (seconds) for the success sample: a fetch taking
/// `LATENCY_SCALE_SECS` scores ~0.5, faster is closer to 1.0 (05-RESEARCH A1,
/// Claude's discretion — tunable, no correctness impact).
const LATENCY_SCALE_SECS: f64 = 3.0;

/// How long a degraded relay is skipped before it is allowed one probe request
/// (05-RESEARCH A1, Claude's discretion). A never-attempted relay is always
/// considered probe-due.
const PROBE_INTERVAL: Duration = Duration::from_secs(60);

/// Per-relay EWMA health registry (RELAY-06).
///
/// Cheap to share behind an `Arc`; all interior state is `Mutex`-guarded. All
/// updates are synchronous, so no lock is ever held across an `.await` — keep
/// every lock scoped to the smallest block (mirrors the `rate_limit.rs`
/// discipline even though no await arises here).
pub struct RelayHealthRegistry {
    /// EWMA smoothing factor in `(0, 1]`.
    alpha: f64,
    /// Current health score per relay url, in `[0, 1]`. Absent => unknown => 1.0.
    scores: Mutex<HashMap<String, f64>>,
    /// Per-relay concurrency-in-use counter (the admission gate + gauge source).
    in_use: Mutex<HashMap<String, u32>>,
    /// Last time a request was attempted against each relay (probe bookkeeping).
    last_probe: Mutex<HashMap<String, Instant>>,
}

impl RelayHealthRegistry {
    /// Build an empty registry with the given EWMA smoothing factor.
    ///
    /// `alpha` is expected to be in `(0, 1]` (the daemon sources it from
    /// `config::Config::health_alpha`, which `validate` guards). A fresh
    /// registry knows no relays, so every relay scores `1.0` until observed.
    pub fn new(alpha: f64) -> Self {
        Self {
            alpha,
            scores: Mutex::new(HashMap::new()),
            in_use: Mutex::new(HashMap::new()),
            last_probe: Mutex::new(HashMap::new()),
        }
    }

    /// Record a successful fetch, penalized by its latency (Pattern 6).
    ///
    /// `sample = 1 / (1 + latency_secs / LATENCY_SCALE_SECS)`: a fast success is
    /// ~1.0; a slow-but-successful fetch is dragged toward 0.5.
    pub fn record_success(&self, relay: &str, latency: Duration) {
        let sample = 1.0 / (1.0 + latency.as_secs_f64() / LATENCY_SCALE_SECS);
        self.update(relay, sample);
    }

    /// Record a fetch timeout (sample `0.0` — drives the score toward 0).
    pub fn record_timeout(&self, relay: &str) {
        self.update(relay, 0.0);
    }

    /// Record a connect/subscribe failure (sample `0.0`, like a timeout).
    pub fn record_connect_failure(&self, relay: &str) {
        self.update(relay, 0.0);
    }

    /// Record a `rate-limited` notice hit (sample `0.2` — degrade, do not zero;
    /// a rate-limited relay is still usable).
    pub fn record_rate_limited(&self, relay: &str) {
        self.update(relay, 0.2);
    }

    /// Apply one EWMA update: `score = alpha * sample + (1 - alpha) * prev`,
    /// where `prev` defaults to `1.0` for an unknown relay. Lock scoped to the
    /// smallest block (no await under the lock).
    fn update(&self, relay: &str, sample: f64) {
        let mut scores = self.scores.lock().expect("health map not poisoned");
        let prev = scores.get(relay).copied().unwrap_or(1.0);
        let next = self.alpha * sample + (1.0 - self.alpha) * prev;
        scores.insert(relay.to_string(), next);
    }

    /// Current health score for a relay; an unknown relay is healthy (`1.0`).
    pub fn score(&self, relay: &str) -> f64 {
        *self
            .scores
            .lock()
            .expect("health map not poisoned")
            .get(relay)
            .unwrap_or(&1.0)
    }

    /// Health-scaled per-relay permit count: `max(1, round(cap * score))`.
    ///
    /// A degraded relay gets proportionally fewer concurrent slots but always at
    /// least one (so a probe can still run and let it recover). 05-04 gates
    /// admission against this count.
    pub fn permits(&self, relay: &str, per_relay_concurrency: usize) -> usize {
        let scaled = (per_relay_concurrency as f64 * self.score(relay)).round() as usize;
        scaled.max(1)
    }

    /// Current per-relay concurrency-in-use count (0 if none in flight). The
    /// gauge source + the admission gate's left-hand side.
    pub fn in_use(&self, relay: &str) -> u32 {
        self.in_use
            .lock()
            .expect("health in_use map not poisoned")
            .get(relay)
            .copied()
            .unwrap_or(0)
    }

    /// Increment the in-use count when admitting a fetch (05-04 admission gate).
    pub fn incr_in_use(&self, relay: &str) {
        let mut map = self.in_use.lock().expect("health in_use map not poisoned");
        let entry = map.entry(relay.to_string()).or_insert(0);
        *entry = entry.saturating_add(1);
    }

    /// Decrement the in-use count when a fetch completes (05-04 admission gate).
    pub fn decr_in_use(&self, relay: &str) {
        let mut map = self.in_use.lock().expect("health in_use map not poisoned");
        if let Some(entry) = map.get_mut(relay) {
            *entry = entry.saturating_sub(1);
        }
    }

    /// Whether a relay may be routed to right now (Pattern 7).
    ///
    /// True when the relay is at or above `threshold`, OR a probe is due (its
    /// last attempt is older than [`PROBE_INTERVAL`], or it has never been
    /// attempted). A degraded relay with a recent attempt returns false (skip).
    pub fn route_allowed(&self, relay: &str, threshold: f64) -> bool {
        if self.score(relay) >= threshold {
            return true;
        }
        // Below threshold: allow only if a probe is due.
        let last = self
            .last_probe
            .lock()
            .expect("health probe map not poisoned")
            .get(relay)
            .copied();
        match last {
            // Never attempted => probe is due.
            None => true,
            Some(t) => t.elapsed() >= PROBE_INTERVAL,
        }
    }

    /// Record that a request was just attempted against `relay`, resetting its
    /// probe clock. Called at the routing decision so the next skip window
    /// starts from this attempt.
    pub fn mark_attempt(&self, relay: &str) {
        self.last_probe
            .lock()
            .expect("health probe map not poisoned")
            .insert(relay.to_string(), Instant::now());
    }
}
