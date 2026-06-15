//! Observability tests (OBS-01 / OBS-02 / OBS-03 / OBS-05).
//!
//! These fill the Wave 0 `#[ignore]` stubs from 04-01 (the OBS-04 progress test
//! is filled in 04-04). HTTP tests drive `observe::router` in-process via
//! `tower::ServiceExt::oneshot` — no real TCP bind. Metric-render assertions use
//! a *local* recorder built from `observe::build_recorder()` (NOT the global
//! install, which is install-once-per-process — RESEARCH §Test Seams).

mod common;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt; // for `oneshot`
use web_of_trust::daemon::config::LogFormat;
use web_of_trust::daemon::observe::{
    self, AppState, METRIC_COVERAGE, METRIC_FETCH_DURATION, METRIC_FRONTIER_DEPTH,
    METRIC_NIP65_RECOVERED, METRIC_RELAY_ACTIVE, METRIC_RELAY_CONCURRENCY, METRIC_RELAY_FAILURES,
    METRIC_RELAY_HEALTH, METRIC_STALENESS_AGE,
};

/// OBS-01: a local recorder, scoped via `metrics::with_local_recorder`, captures
/// the exported series and renders them into the Prometheus exposition.
#[tokio::test]
async fn metrics_endpoint_exposes_series() {
    let recorder = observe::build_recorder();
    let handle = recorder.handle();

    // Fire one of each metric kind into the LOCAL recorder only. `with_local_recorder`
    // is synchronous + thread-local, so the closure body must do the emission.
    metrics::with_local_recorder(&recorder, || {
        metrics::gauge!(METRIC_FRONTIER_DEPTH).set(42.0);
        metrics::gauge!(METRIC_COVERAGE).set(0.5);
        metrics::gauge!(METRIC_RELAY_FAILURES).set(3.0);
        metrics::gauge!(METRIC_RELAY_ACTIVE).set(7.0);
        metrics::histogram!(METRIC_FETCH_DURATION).record(1.5);
        metrics::histogram!(METRIC_STALENESS_AGE).record(3_600.0);
        // Also exercise one of the existing fire-and-forget counter sites' name.
        metrics::counter!("ingest_invalid_signature").increment(1);
    });

    let rendered = handle.render();
    assert!(!rendered.is_empty(), "exposition must be non-empty");
    for name in [
        METRIC_FRONTIER_DEPTH,
        METRIC_COVERAGE,
        METRIC_RELAY_FAILURES,
        METRIC_RELAY_ACTIVE,
        METRIC_FETCH_DURATION,
        METRIC_STALENESS_AGE,
        "ingest_invalid_signature",
    ] {
        assert!(
            rendered.contains(name),
            "rendered exposition must contain series {name}; got:\n{rendered}"
        );
    }
}

/// OBS-03: `/health/live` is 200 unconditionally (the process answers HTTP).
///
/// Uses an unreachable pool — `live` must not touch the DB, so the bad pool is
/// irrelevant to the result.
#[tokio::test]
async fn live_always_ok() {
    let recorder = observe::build_recorder();
    let pool = lazy_unreachable_pool();
    // loop_alive=false to prove /health/live ignores readiness entirely.
    let state = AppState::new(recorder.handle(), Arc::new(AtomicBool::new(false)), pool);

    let resp = observe::router(state)
        .oneshot(
            Request::builder()
                .uri("/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

/// OBS-03: `/health/ready` is 200 only with loop_alive=true AND a reachable DB;
/// 503 when the loop is down, and 503 when the loop is up but the DB is unreachable.
#[tokio::test]
async fn ready_requires_db_and_loop() {
    // --- loop down → 503 (short-circuits before any DB ping) ---
    {
        let recorder = observe::build_recorder();
        let state = AppState::new(
            recorder.handle(),
            Arc::new(AtomicBool::new(false)),
            lazy_unreachable_pool(),
        );
        let resp = ready(state).await;
        assert_eq!(
            resp,
            StatusCode::SERVICE_UNAVAILABLE,
            "loop down must be 503"
        );
    }

    // --- loop up + unreachable DB → 503 (bounded ping fails/times out) ---
    {
        let recorder = observe::build_recorder();
        let state = AppState::new(
            recorder.handle(),
            Arc::new(AtomicBool::new(true)),
            lazy_unreachable_pool(),
        );
        let resp = ready(state).await;
        assert_eq!(
            resp,
            StatusCode::SERVICE_UNAVAILABLE,
            "unreachable DB must be 503"
        );
    }

    // --- loop up + live testcontainers DB → 200 ---
    {
        let (_pg, pool) = match common::fresh_db().await {
            Ok(v) => v,
            Err(e) => panic!("testcontainers DB unavailable (re-run once on container race): {e}"),
        };
        let recorder = observe::build_recorder();
        let state = AppState::new(recorder.handle(), Arc::new(AtomicBool::new(true)), pool);
        let resp = ready(state).await;
        assert_eq!(resp, StatusCode::OK, "loop up + live DB must be 200");
    }
}

/// OBS-02: both log formats build a subscriber without panicking.
///
/// The global subscriber installs once per process, so we cannot call
/// `observe::init_tracing` twice here (the second `.init()` would panic). Instead
/// we assert the builder path for both formats by constructing the same
/// `EnvFilter` + `fmt` layers `init_tracing` uses, proving neither format path
/// panics during layer construction.
#[tokio::test]
async fn json_format_selected() {
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::{fmt, EnvFilter};

    for (level, format) in [("info", LogFormat::Json), ("debug", LogFormat::Human)] {
        // Mirror init_tracing's filter construction (must not panic for either level).
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
        let registry = tracing_subscriber::registry().with(filter);
        // Build + attach the format-specific layer scoped to this thread (NOT a
        // global `.init()`, which can only happen once per process). `set_default`
        // returns a drop guard, so both formats can be exercised in one test.
        let _guard = match format {
            LogFormat::Json => registry.with(fmt::layer().json()).set_default(),
            LogFormat::Human => registry.with(fmt::layer()).set_default(),
        };
        // Emit a span/event so the subscriber path is genuinely exercised.
        tracing::info!(target: "observe_test", fmt = ?format, "format builds");
    }
    // Reaching here means both format paths constructed + attached cleanly.
}

/// OBS-05: the committed Grafana dashboard JSON parses and references every
/// OBS-01 series name.
#[test]
fn dashboard_json_valid() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/ops/grafana-dashboard.json");
    let text = std::fs::read_to_string(path).expect("ops/grafana-dashboard.json must exist");

    let parsed: serde_json::Value =
        serde_json::from_str(&text).expect("ops/grafana-dashboard.json must be valid JSON");
    assert!(parsed.is_object(), "dashboard root must be a JSON object");

    for name in [
        METRIC_FRONTIER_DEPTH,
        METRIC_COVERAGE,
        METRIC_FETCH_DURATION,
        METRIC_STALENESS_AGE,
        METRIC_RELAY_FAILURES,
        METRIC_RELAY_ACTIVE,
        // RELAY-05/06 panels: per-relay health + concurrency gauges + the recovery
        // counter (referenced in PromQL as nip65_recovered_total — substring match).
        METRIC_RELAY_HEALTH,
        METRIC_NIP65_RECOVERED,
        METRIC_RELAY_CONCURRENCY,
        // The existing validation-failure / relay-notice counter series.
        "ingest_invalid_signature",
        "ingest_unsolicited",
        "ingest_oversized_follow_list",
        "ingest_future_dated",
        "relay_rate_limited",
        "relay_blocked",
    ] {
        assert!(
            text.contains(name),
            "dashboard JSON must reference series {name}"
        );
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Drive `/health/ready` once through the router and return its status.
async fn ready(state: AppState) -> StatusCode {
    observe::router(state)
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// A `PgPool` configured lazily against an unreachable address. `lazy` means the
/// pool constructs without a live server; the first query (the readiness ping)
/// then fails/times out, which is exactly the "unreachable DB" condition under
/// test. Never logs the URL (T-04-03 posture).
fn lazy_unreachable_pool() -> sqlx::PgPool {
    // 127.0.0.1:1 is a reserved port nothing listens on → connect attempts fail fast.
    sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(200))
        .connect_lazy("postgres://postgres:postgres@127.0.0.1:1/postgres")
        .expect("lazy pool construction does not require a live server")
}
