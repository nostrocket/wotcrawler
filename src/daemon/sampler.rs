//! Periodic observability + maintenance timers (OBS-01 / OBS-04 / FRESH-02 /
//! OPS-02).
//!
//! These are the daemon's coarse-interval background tasks, each a
//! `tokio::time::interval` loop that `select!`s on a [`CancellationToken`] so it
//! stops promptly on shutdown:
//!
//! - [`sample_gauges`] (OBS-01): one cheap `GROUP BY status` aggregate per tick →
//!   emits the frontier-depth / coverage gauges, a staleness-age histogram, and
//!   relay-health gauges sourced from the [`RateLimiterRegistry`]. Aggregate gauges
//!   only — NO per-pubkey labels (Pitfall 7 / T-04-06), and a COARSE interval so
//!   the aggregate never starves the crawl (Pitfall 6 / T-04-10).
//! - [`progress_summary`] (OBS-04): logs a periodic crawl-progress line (frontier
//!   size, coverage %) from the same cheap counts.
//! - [`staleness_timer`] (FRESH-02): periodically re-enqueues stale terminal rows
//!   via [`reclaim_stale_by_ttl`].
//! - [`in_run_reclaim_timer`] (OPS-02): periodically reclaims age-orphaned
//!   `in_progress` leases via [`reclaim_in_progress_older_than`].
//!
//! All four read the same frontier-state primitives; none own raw edge SQL beyond
//! the single status aggregate ([`frontier_counts`]) and the reused sweep
//! functions.

use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

use crate::crawl::frontier::{reclaim_in_progress_older_than, reclaim_stale_by_ttl};
use crate::daemon::observe::{
    METRIC_COVERAGE, METRIC_FRONTIER_DEPTH, METRIC_RELAY_ACTIVE, METRIC_RELAY_FAILURES,
    METRIC_STALENESS_AGE,
};
use crate::error::StoreError;
use crate::relay::rate_limit::RateLimiterRegistry;

/// A snapshot of the frontier by status, plus the total, from a single
/// `GROUP BY status` aggregate.
///
/// Coverage (OBS-01) is `fetched / total`; frontier depth is `discovered`. The
/// terminal counts (`not_found`, `failed`) round out the picture for the progress
/// summary. One aggregate feeds every gauge so a tick is one cheap query
/// (Pitfall 6).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrontierCounts {
    /// Rows in the frontier awaiting a fetch (`status = 'discovered'`).
    pub discovered: i64,
    /// Rows currently leased to a worker (`status = 'in_progress'`).
    pub in_progress: i64,
    /// Rows whose follow list has been fetched + applied (`status = 'fetched'`).
    pub fetched: i64,
    /// Rows the relays answered with no kind-3 (`status = 'not_found'`).
    pub not_found: i64,
    /// Rows that exhausted their fetch-retry budget (`status = 'failed'`).
    pub failed: i64,
    /// Total pubkey rows across all statuses.
    pub total: i64,
}

impl FrontierCounts {
    /// Fraction of all known pubkeys whose follow list has been fetched
    /// (0.0..=1.0). Guards `total == 0` (an empty DB before the anchor seed) to
    /// avoid a `NaN` gauge.
    pub fn coverage(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.fetched as f64 / self.total as f64
        }
    }
}

/// Read the frontier state in a single `GROUP BY status` aggregate (OBS-01).
///
/// One query per sample tick (Pitfall 6 — never per-row, never per-second): the
/// partial `pubkeys_status_idx` and the table's row visibility make this a cheap
/// grouped count even at millions of rows. Statuses outside the known set are
/// ignored (there are none today; the CHECK constraint bounds the domain).
pub async fn frontier_counts(pool: &PgPool) -> Result<FrontierCounts, StoreError> {
    let rows = sqlx::query!("SELECT status, COUNT(*) AS count FROM pubkeys GROUP BY status")
        .fetch_all(pool)
        .await?;

    let mut counts = FrontierCounts::default();
    for row in rows {
        let n = row.count.unwrap_or(0);
        match row.status.as_str() {
            "discovered" => counts.discovered = n,
            "in_progress" => counts.in_progress = n,
            "fetched" => counts.fetched = n,
            "not_found" => counts.not_found = n,
            "failed" => counts.failed = n,
            _ => {}
        }
        counts.total += n;
    }
    Ok(counts)
}

/// Sample the frontier + relay-health aggregates on a coarse interval and emit
/// them as Prometheus gauges/histograms (OBS-01).
///
/// Each tick runs ONE [`frontier_counts`] aggregate and emits:
/// - [`METRIC_FRONTIER_DEPTH`] gauge ← `discovered`,
/// - [`METRIC_COVERAGE`] gauge ← `fetched / total` (guarded),
/// - [`METRIC_STALENESS_AGE`] histogram ← per-row age buckets from a cheap age
///   aggregate (capped sample so the histogram never enumerates the whole table),
/// - [`METRIC_RELAY_ACTIVE`] gauge ← [`RateLimiterRegistry::active_relay_count`],
/// - [`METRIC_RELAY_FAILURES`] gauge ← max consecutive-failure count across the
///   curated relay set (NO per-relay label here — the aggregate max is the health
///   signal; per-relay failure counters already exist as `relay_rate_limited`).
///
/// `interval` MUST be coarse (15–60s) so the aggregate never competes with the
/// crawl for connections (Pitfall 6 / T-04-10). Stops on `token.cancelled()`.
pub async fn sample_gauges(
    pool: PgPool,
    registry: Arc<RateLimiterRegistry>,
    relays: Vec<String>,
    token: CancellationToken,
    interval: Duration,
) {
    let mut ticker = tokio::time::interval(interval);
    loop {
        tokio::select! {
            _ = token.cancelled() => break,
            _ = ticker.tick() => {}
        }

        match frontier_counts(&pool).await {
            Ok(counts) => {
                metrics::gauge!(METRIC_FRONTIER_DEPTH).set(counts.discovered as f64);
                metrics::gauge!(METRIC_COVERAGE).set(counts.coverage());
            }
            Err(e) => {
                // A sampler read failure is non-fatal: log and keep ticking so a
                // transient DB blip never tears down observability.
                tracing::warn!(error = %e, "frontier_counts sample failed");
            }
        }

        // Staleness-age histogram: one cheap aggregate of per-row ages (seconds
        // since last_fetched_at) for the terminal-status population, observed into
        // the configured buckets. Capped so the histogram never walks every row.
        match staleness_ages(&pool).await {
            Ok(ages) => {
                for age in ages {
                    metrics::histogram!(METRIC_STALENESS_AGE).record(age);
                }
            }
            Err(e) => tracing::warn!(error = %e, "staleness_ages sample failed"),
        }

        // Relay health (aggregate only — Pitfall 7): active relay count + the max
        // consecutive-failure count across the curated set.
        metrics::gauge!(METRIC_RELAY_ACTIVE).set(registry.active_relay_count() as f64);
        let max_failures = relays
            .iter()
            .map(|r| registry.failure_count(r))
            .max()
            .unwrap_or(0);
        metrics::gauge!(METRIC_RELAY_FAILURES).set(max_failures as f64);
    }
}

/// One cheap, capped aggregate of staleness ages (seconds since `last_fetched_at`)
/// for the terminal-status population, for the staleness-age histogram (OBS-01).
///
/// Capped at a bounded sample (`LIMIT`) so the histogram emit per tick is O(cap),
/// not O(table) — the distribution shape, not an exact per-row enumeration, is
/// what the operator dashboard needs (Pitfall 6).
async fn staleness_ages(pool: &PgPool) -> Result<Vec<f64>, StoreError> {
    let rows = sqlx::query!(
        // Cast the EXTRACT(EPOCH ...) NUMERIC to `double precision` IN SQL so sqlx
        // infers a plain `f64` bind (no bigdecimal dep). The age is seconds since
        // the row's last_fetched_at.
        "SELECT EXTRACT(EPOCH FROM (now() - last_fetched_at))::double precision AS age_secs \
         FROM pubkeys \
         WHERE status IN ('fetched','not_found','failed') \
           AND last_fetched_at IS NOT NULL \
         ORDER BY last_fetched_at ASC \
         LIMIT 1000"
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().filter_map(|r| r.age_secs).collect())
}

/// Log a periodic crawl-progress summary (OBS-04) from the cheap frontier counts.
///
/// Same coarse `interval` + `select!`-on-cancel shape as [`sample_gauges`]; emits
/// a single `tracing::info!` line per tick with frontier depth, coverage %, and
/// the terminal-status breakdown so an operator tailing logs sees forward
/// progress without scraping Prometheus.
pub async fn progress_summary(pool: PgPool, token: CancellationToken, interval: Duration) {
    let mut ticker = tokio::time::interval(interval);
    loop {
        tokio::select! {
            _ = token.cancelled() => break,
            _ = ticker.tick() => {}
        }

        match frontier_counts(&pool).await {
            Ok(c) => tracing::info!(
                frontier = c.discovered,
                in_progress = c.in_progress,
                fetched = c.fetched,
                not_found = c.not_found,
                failed = c.failed,
                total = c.total,
                coverage_pct = c.coverage() * 100.0,
                "crawl progress"
            ),
            Err(e) => tracing::warn!(error = %e, "progress summary sample failed"),
        }
    }
}

/// Periodically re-enqueue stale terminal rows into the frontier (FRESH-02).
///
/// Calls [`reclaim_stale_by_ttl`] every `interval`, increments a
/// `staleness_reenqueued` counter by the number re-enqueued, and logs the
/// sweep result. Stops on `token.cancelled()`. `ttl_secs` is the uniform staleness
/// TTL: rows whose `last_fetched_at` is older are flipped back to `discovered`.
pub async fn staleness_timer(
    pool: PgPool,
    ttl_secs: i64,
    interval: Duration,
    token: CancellationToken,
) {
    let mut ticker = tokio::time::interval(interval);
    loop {
        tokio::select! {
            _ = token.cancelled() => break,
            _ = ticker.tick() => {}
        }

        match reclaim_stale_by_ttl(&pool, ttl_secs).await {
            Ok(n) => {
                // Name WITHOUT a manual `_total`: metrics-exporter-prometheus
                // appends `_total` to counters in exposition, so this exports as
                // `staleness_reenqueued_total` (WR-02 — avoids the doubled suffix).
                metrics::counter!("staleness_reenqueued").increment(n);
                tracing::info!(reenqueued = n, ttl_secs, "staleness scan re-enqueued stale rows");
            }
            Err(e) => tracing::warn!(error = %e, "staleness scan failed"),
        }
    }
}

/// Periodically reclaim age-orphaned `in_progress` leases (OPS-02).
///
/// Calls [`reclaim_in_progress_older_than`] every `interval`; the `age_secs`
/// threshold is set comfortably above the fetch timeout so a freshly-claimed live
/// lease is NEVER reset out from under an in-flight fetch (T-04-02). Increments an
/// `in_run_reclaimed` counter and logs the result. Stops on
/// `token.cancelled()`.
pub async fn in_run_reclaim_timer(
    pool: PgPool,
    age_secs: i64,
    interval: Duration,
    token: CancellationToken,
) {
    let mut ticker = tokio::time::interval(interval);
    loop {
        tokio::select! {
            _ = token.cancelled() => break,
            _ = ticker.tick() => {}
        }

        match reclaim_in_progress_older_than(&pool, age_secs).await {
            Ok(n) => {
                // Name WITHOUT a manual `_total`: the exporter appends `_total`,
                // exporting this as `in_run_reclaimed_total` (WR-02).
                metrics::counter!("in_run_reclaimed").increment(n);
                if n > 0 {
                    tracing::info!(reclaimed = n, age_secs, "in-run reclaim reset orphaned leases");
                }
            }
            Err(e) => tracing::warn!(error = %e, "in-run reclaim failed"),
        }
    }
}
