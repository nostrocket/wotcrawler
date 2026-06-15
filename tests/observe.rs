//! Observability: metrics + health endpoints + progress summary (filled in 04-03/04-04).
//!
//! Wave 0 scaffold: named `#[ignore]` stubs that plans 04-03/04-04 fill once the
//! `daemon::observe` axum router, the Prometheus recorder handle, and the
//! `daemon::sampler` gauge/progress emitters exist. The HTTP tests will drive the
//! router in-process via `tower::ServiceExt::oneshot` against a `build_recorder()`
//! handle (NOT a global recorder install — RESEARCH §Test Seams).

mod common;

/// OBS-01: the `/metrics` endpoint exposes the expected Prometheus series.
#[tokio::test]
#[ignore = "filled in 04-03/04-04"]
async fn metrics_endpoint_exposes_series() {
    unimplemented!("04-03: /metrics exposes the exported series");
}

/// OBS-03: `/health/live` always returns OK while the process is up.
#[tokio::test]
#[ignore = "filled in 04-03/04-04"]
async fn live_always_ok() {
    unimplemented!("04-03: /health/live always 200 while up");
}

/// OBS-03: `/health/ready` requires both DB reachability AND a running loop.
#[tokio::test]
#[ignore = "filled in 04-03/04-04"]
async fn ready_requires_db_and_loop() {
    unimplemented!("04-03: /health/ready gated on DB + loop running");
}

/// OBS-02: JSON log format is selectable via config.
#[tokio::test]
#[ignore = "filled in 04-03/04-04"]
async fn json_format_selected() {
    unimplemented!("04-03: JSON log format selectable");
}

/// OBS-04: the progress summary reports correct frontier/coverage counts.
#[tokio::test]
#[ignore = "filled in 04-03/04-04"]
async fn progress_summary_counts() {
    unimplemented!("04-04: progress summary frontier/coverage counts");
}

/// OBS-05: the committed Grafana dashboard JSON is valid.
#[tokio::test]
#[ignore = "filled in 04-03/04-04"]
async fn dashboard_json_valid() {
    unimplemented!("04-05: Grafana dashboard JSON parses");
}
