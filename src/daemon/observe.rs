//! Observability surface (OBS-01 / OBS-02 / OBS-03).
//!
//! This module owns the daemon's operator-facing trust surface for an
//! unattended multi-day run:
//!
//! - **OBS-01 — metrics.** [`install_metrics`] installs the *global*
//!   `metrics-exporter-prometheus` recorder (no second hyper server — the axum
//!   router below owns the `/metrics` route) and returns a [`PrometheusHandle`].
//!   The six existing fire-and-forget `metrics::counter!` sites
//!   (`ingest/verify.rs`, `ingest/replaceable.rs`, `ingest/follow_list.rs`,
//!   `relay/rate_limit.rs`) become live the instant the recorder is installed —
//!   *no edits to those sites*. Aggregate gauges/histograms (frontier depth,
//!   coverage, fetch latency, staleness distribution, relay health) are emitted
//!   by the sampler (04-04) into the metric names exported as `pub const`s here.
//! - **OBS-02 — structured logging.** [`init_tracing`] installs a `tracing`
//!   subscriber with an `EnvFilter` (config level or `RUST_LOG`) and a runtime
//!   switch between a human `fmt` layer and a JSON layer.
//! - **OBS-03 — health.** [`router`] serves `GET /metrics`, `/health/live`
//!   (200 while the process answers HTTP), and `/health/ready` (200 only when
//!   the crawl loop is alive *and* the DB is reachable, else 503).
//!
//! # Ordering invariant (RESEARCH Pitfall 1)
//!
//! [`install_metrics`] MUST run before any code path fires a `metrics::counter!`
//! / `gauge!` / `histogram!`. Metrics emitted before the recorder is installed
//! are sunk into the no-op recorder and lost. `main` (04-05) installs the
//! recorder first thing after tracing init.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use sqlx::PgPool;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{fmt, EnvFilter};

use crate::daemon::config::LogFormat;

// ---------------------------------------------------------------------------
// Exported metric-name constants (OBS-01).
//
// The sampler (04-04) emits into these names; the Grafana dashboard (OBS-05)
// references them; the render test asserts they appear in the exposition. Names
// are at Claude's discretion per CONTEXT — documented here as the single source
// of truth. Aggregate gauges only — NO per-pubkey labels (Pitfall 7 / T-04-06).
// ---------------------------------------------------------------------------

/// Gauge: current crawl frontier depth (rows in `pubkeys.status='discovered'`).
pub const METRIC_FRONTIER_DEPTH: &str = "frontier_depth";

/// Gauge: fraction of reachable pubkeys whose follow list has been fetched
/// (0.0..=1.0).
pub const METRIC_COVERAGE: &str = "crawl_coverage_ratio";

/// Histogram: per-batch relay fetch latency, in seconds.
pub const METRIC_FETCH_DURATION: &str = "fetch_duration_seconds";

/// Histogram: distribution of follow-list staleness (age since `last_fetched_at`),
/// in seconds.
pub const METRIC_STALENESS_AGE: &str = "staleness_age_seconds";

/// Gauge: max consecutive-failure count across the curated relay set (relay
/// health). Source: [`crate::relay::rate_limit::RateLimiterRegistry::failure_count`].
pub const METRIC_RELAY_FAILURES: &str = "relay_consecutive_failures";

/// Gauge: number of relays with a live limiter (active relay count). Source:
/// [`crate::relay::rate_limit::RateLimiterRegistry::active_relay_count`].
pub const METRIC_RELAY_ACTIVE: &str = "relay_active_count";

/// Latency buckets (seconds) for the per-batch fetch-duration histogram.
const FETCH_DURATION_BUCKETS: &[f64] = &[0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0];

/// Age buckets (seconds) for the staleness-age histogram: minutes → hours → a
/// few days, spanning the TTL window an operator tunes around.
const STALENESS_AGE_BUCKETS: &[f64] = &[
    300.0,     // 5m
    900.0,     // 15m
    3_600.0,   // 1h
    21_600.0,  // 6h
    43_200.0,  // 12h
    86_400.0,  // 1d
    172_800.0, // 2d
    259_200.0, // 3d
    604_800.0, // 7d
];

/// Bound on the `/health/ready` DB ping so a slow/hung DB cannot stall a
/// graceful shutdown (RESEARCH Pitfall 8 / T-04-07).
const READY_PING_TIMEOUT: Duration = Duration::from_millis(500);

/// Build a [`PrometheusBuilder`] configured with the project's histogram buckets.
///
/// Shared by [`install_metrics`] (global install) and tests (which call
/// [`PrometheusBuilder::build_recorder`] on the returned builder to get a local
/// handle without touching the global recorder — RESEARCH §Test Seams).
fn configured_builder() -> PrometheusBuilder {
    PrometheusBuilder::new()
        .set_buckets_for_metric(
            Matcher::Full(METRIC_FETCH_DURATION.to_string()),
            FETCH_DURATION_BUCKETS,
        )
        .expect("fetch-duration buckets are non-empty and valid")
        .set_buckets_for_metric(
            Matcher::Full(METRIC_STALENESS_AGE.to_string()),
            STALENESS_AGE_BUCKETS,
        )
        .expect("staleness-age buckets are non-empty and valid")
}

/// Install the global Prometheus recorder and return its render handle (OBS-01).
///
/// MUST run before any `metrics::*!` macro fires (Pitfall 1); installs the
/// global recorder exactly once for the process and configures the histogram
/// buckets for [`METRIC_FETCH_DURATION`] and [`METRIC_STALENESS_AGE`]. No HTTP
/// listener is started — the [`router`] owns the `/metrics` route.
pub fn install_metrics() -> PrometheusHandle {
    configured_builder()
        .install_recorder()
        .expect("the global Prometheus recorder installs exactly once per process")
}

/// Initialize the global `tracing` subscriber (OBS-02).
///
/// Builds an [`EnvFilter`] from `RUST_LOG` if set, else from the configured
/// `level` directive (e.g. `"info"`), then finishes the registry with either a
/// human `fmt` layer ([`LogFormat::Human`]) or a JSON layer ([`LogFormat::Json`]).
/// Installs the subscriber globally; call once at process start, before the first
/// span/log. The DB URL is never a tracing field (T-04-03).
pub fn init_tracing(level: &str, format: LogFormat) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    let registry = tracing_subscriber::registry().with(filter);
    match format {
        LogFormat::Json => registry.with(fmt::layer().json()).init(),
        LogFormat::Human => registry.with(fmt::layer()).init(),
    }
}

/// Shared state for the observability router: the render handle, the loop-alive
/// flag (set `true` by the daemon loop once seeding succeeds), and the DB pool
/// (for the readiness ping).
#[derive(Clone)]
pub struct AppState {
    handle: PrometheusHandle,
    loop_alive: Arc<AtomicBool>,
    pool: PgPool,
}

impl AppState {
    /// Bundle the render handle, loop-alive flag, and DB pool into router state.
    pub fn new(handle: PrometheusHandle, loop_alive: Arc<AtomicBool>, pool: PgPool) -> Self {
        Self {
            handle,
            loop_alive,
            pool,
        }
    }
}

/// Build the observability router: one axum server, three routes (OBS-01/03).
///
/// - `GET /metrics` → Prometheus exposition rendered from the handle.
/// - `GET /health/live` → 200 unconditionally.
/// - `GET /health/ready` → 200 iff the loop is alive AND the DB pings, else 503.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/health/live", get(live_handler))
        .route("/health/ready", get(ready_handler))
        .with_state(state)
}

/// `GET /metrics`: render the Prometheus text exposition from the handle.
async fn metrics_handler(State(st): State<AppState>) -> String {
    st.handle.render()
}

/// `GET /health/live`: the process answers HTTP, so it is alive (OBS-03).
async fn live_handler() -> StatusCode {
    StatusCode::OK
}

/// `GET /health/ready`: ready iff the loop is alive AND the DB is reachable.
///
/// The `SELECT 1` ping is bounded by [`READY_PING_TIMEOUT`] so a hung DB cannot
/// stall graceful shutdown (Pitfall 8 / T-04-07).
async fn ready_handler(State(st): State<AppState>) -> StatusCode {
    if !st.loop_alive.load(Ordering::Relaxed) {
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    let ping = sqlx::query_scalar!("SELECT 1 AS one").fetch_one(&st.pool);
    match tokio::time::timeout(READY_PING_TIMEOUT, ping).await {
        Ok(Ok(_)) => StatusCode::OK,
        // DB error OR ping timeout → not ready.
        Ok(Err(_)) | Err(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}

/// Build a *local* (non-global) recorder with the project's histogram buckets,
/// for tests that assert on rendered output without installing the global
/// recorder (RESEARCH §Test Seams — the global recorder installs once per
/// process, so assertion-only tests must not call [`install_metrics`]).
///
/// Returns the recorder (which provides a [`PrometheusHandle`] via its `handle`)
/// so a test can scope metric emission to it with `metrics::with_local_recorder`.
#[doc(hidden)]
pub fn build_recorder() -> metrics_exporter_prometheus::PrometheusRecorder {
    configured_builder().build_recorder()
}
