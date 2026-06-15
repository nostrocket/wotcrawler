---
phase: 04-daemon-staleness-loop-observability
plan: 03
subsystem: daemon-observability
tags: [daemon, observability, metrics, prometheus, tracing, axum, health, grafana, obs-01, obs-02, obs-03, obs-05]
requires:
  - "daemon module root (04-01)"
  - "daemon::config::LogFormat (04-02)"
  - "Cargo deps: metrics-exporter-prometheus, axum, tracing, tracing-subscriber, tower (dev), metrics (04-01)"
  - "store::PgPool (Phase 1) for the readiness ping"
  - "existing metrics::counter! sites (ingest/verify.rs, ingest/replaceable.rs, ingest/follow_list.rs, relay/rate_limit.rs)"
provides:
  - "daemon::observe::install_metrics() -> PrometheusHandle (global recorder install + histogram buckets)"
  - "daemon::observe::init_tracing(level, LogFormat) (EnvFilter + human/JSON fmt layer)"
  - "daemon::observe::AppState { handle, loop_alive: Arc<AtomicBool>, pool } + AppState::new"
  - "daemon::observe::router(AppState) -> axum::Router serving /metrics + /health/live + /health/ready"
  - "daemon::observe::build_recorder() -> PrometheusRecorder (test seam, no global install)"
  - "exported metric-name consts: METRIC_FRONTIER_DEPTH, METRIC_COVERAGE, METRIC_FETCH_DURATION, METRIC_STALENESS_AGE, METRIC_RELAY_FAILURES, METRIC_RELAY_ACTIVE"
  - "ops/grafana-dashboard.json (OBS-05 dashboard referencing every OBS-01 series)"
affects:
  - "04-04 (sampler emits gauges/histograms into the exported metric names)"
  - "04-05 (main bootstrap: init_tracing -> install_metrics -> router on metrics_addr)"
tech-stack:
  added:
    - "serde_json 1.0.150 (dev-dep) — parse/validate the committed Grafana dashboard JSON; already resolved transitively"
  patterns:
    - "install_recorder() global recorder, no http-listener feature — axum router owns /metrics (render handle.render() from a GET handler)"
    - "set_buckets_for_metric(Matcher::Full(name), &[..]) for fetch-latency + staleness-age histograms"
    - "EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level)) then registry().with(filter).with(fmt::layer()[.json()]).init()"
    - "readiness = Arc<AtomicBool> loop flag + bounded tokio::time::timeout SELECT 1 ping (Pitfall 8)"
    - "test seam: build_recorder() + metrics::with_local_recorder for render assertions (global recorder installs once per process)"
    - "axum handler tests in-process via tower::ServiceExt::oneshot (no TCP bind)"
key-files:
  created:
    - "src/daemon/observe.rs"
    - "ops/grafana-dashboard.json"
  modified:
    - "src/daemon/mod.rs"
    - "tests/observe.rs"
    - "Cargo.toml"
    - "Cargo.lock"
    - ".sqlx/ (1 new query: SELECT 1 readiness ping)"
decisions:
  - "Metric names locked (CONTEXT discretion): frontier_depth, crawl_coverage_ratio, fetch_duration_seconds (histogram), staleness_age_seconds (histogram), relay_consecutive_failures, relay_active_count"
  - "json_format_selected uses set_default() (scoped guard) not init() — global subscriber installs once per process, so both formats are exercised in one test without a double-init panic"
  - "ready unreachable-DB case modeled with connect_lazy to 127.0.0.1:1 — pool constructs without a server, the bounded ping then fails/times out (exactly the unreachable condition)"
  - "serde_json declared as a dev-dep (already in the lock tree); mirrors 04-02's serde direct-dep declaration — not a new package install"
metrics:
  duration_min: 5
  tasks: 2
  files: 6
  completed: "2026-06-15"
---

# Phase 4 Plan 03: Daemon Observability Summary

The operator's trust surface (OBS-01/02/03/05): a single axum router serving `/metrics` (rendering the installed `metrics-exporter-prometheus` handle), `/health/live` (200 while up), and `/health/ready` (200 only when the crawl loop is alive AND the DB pings, else 503); `tracing` init with an `EnvFilter` and a runtime human/JSON format switch; six exported metric-name constants the 04-04 sampler emits into; and a committed Grafana dashboard referencing every OBS-01 series. The six pre-existing `metrics::counter!` sites become live the instant the recorder installs — no edits to them.

## What Was Built

**Task 1 — observe.rs: recorder install, tracing init, axum router + handlers (commit 0ea5417):** `src/daemon/observe.rs` exports `install_metrics() -> PrometheusHandle` (builds `PrometheusBuilder::new()`, sets histogram buckets for `fetch_duration_seconds` and `staleness_age_seconds` via `set_buckets_for_metric(Matcher::Full(..))`, then `install_recorder()` — no `http-listener`, the axum router owns `/metrics`), `init_tracing(level, LogFormat)` (EnvFilter from `RUST_LOG`-or-level, then `registry().with(filter)` finished with `fmt::layer().json()` for `Json` / `fmt::layer()` for `Human`, `.init()`), `AppState { handle, loop_alive: Arc<AtomicBool>, pool }` (derives `Clone`, `AppState::new` constructor), and `router(state)` wiring `GET /metrics` → `handle.render()`, `/health/live` → `OK`, `/health/ready` → loop-alive check then a `tokio::time::timeout(500ms, SELECT 1)` bounded ping (Pitfall 8 / T-04-07). Exported six metric-name `pub const`s (`METRIC_FRONTIER_DEPTH="frontier_depth"`, `METRIC_COVERAGE="crawl_coverage_ratio"`, `METRIC_FETCH_DURATION="fetch_duration_seconds"`, `METRIC_STALENESS_AGE="staleness_age_seconds"`, `METRIC_RELAY_FAILURES="relay_consecutive_failures"`, `METRIC_RELAY_ACTIVE="relay_active_count"`) plus a `build_recorder()` test seam. Registered `pub mod observe;` in `src/daemon/mod.rs` (additive — `pub mod config;` from 04-02 untouched). `.sqlx` regenerated for the `SELECT 1` readiness query (20 prior + 1 new = 21 files, zero deletions).

**Task 2 — observe tests + Grafana dashboard (commit e80ede0):** `tests/observe.rs` fills the five Wave 0 `#[ignore]` stubs this plan owns: `metrics_endpoint_exposes_series` (fires a gauge/histogram/counter for each exported name into a `build_recorder()` local recorder via `metrics::with_local_recorder`, asserts each name appears in `handle.render()`), `live_always_ok` (200 even with `loop_alive=false` + unreachable pool — proves `/health/live` ignores readiness), `ready_requires_db_and_loop` (loop-down → 503, loop-up + unreachable DB → 503, loop-up + live testcontainers DB → 200), `json_format_selected` (both formats build + attach via `set_default()` scoped guards, emitting a real event each), `dashboard_json_valid` (parses `ops/grafana-dashboard.json` with `serde_json` and asserts every OBS-01 series + the existing ingest/relay counter names are referenced). Health tests drive `observe::router(state)` in-process via `tower::ServiceExt::oneshot`. `ops/grafana-dashboard.json` is a valid schema-39 Grafana dashboard with six panels (frontier depth, coverage, fetch rate/latency, staleness distribution heatmap, relay health, validation failures), each panel's `targets[].expr` referencing the series name.

## Verification Results

- `SQLX_OFFLINE=true cargo build --lib` — exit 0.
- `SQLX_OFFLINE=true cargo build --all-targets` — exit 0.
- `SQLX_OFFLINE=true cargo test --test observe -- --test-threads=2` — 5 passed (`dashboard_json_valid`, `json_format_selected`, `metrics_endpoint_exposes_series`, `live_always_ok`, `ready_requires_db_and_loop`); the live-DB readiness case ran against a real testcontainers Postgres.
- `cargo clippy --all-targets` — 0 warnings on `src/daemon/observe.rs` / `tests/observe.rs` (the one `unnecessary_to_owned` in `init_tracing` was fixed before the Task 1 commit).
- `cargo sqlx prepare -- --all-targets` — clean, 21 `.sqlx` files (20 prior + 1 new `SELECT 1`), zero deletions.
- Acceptance greps: `install_metrics`/`init_tracing`/`router`/`install_recorder`/`.render()`/`SERVICE_UNAVAILABLE` all match; `grep -c http-listener src/daemon/observe.rs Cargo.toml` = 0; all three routes present; dashboard series grep = 8 line-matches across the five core OBS-01 series.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] `serde_json` not declared as a dependency**
- **Found during:** Task 2 (`dashboard_json_valid` test needs `serde_json::from_str`).
- **Issue:** `serde_json` was only present transitively (via `config`/`nostr-sdk`), not declared, so the integration test could not `use serde_json`.
- **Fix:** Added `serde_json = "1.0.150"` to `[dev-dependencies]`, pinned to the already-resolved lock version. A declaration of an already-present, audited crate — not a new package install (mirrors 04-02's `serde` direct-dep fix).
- **Files modified:** `Cargo.toml`, `Cargo.lock`.
- **Commit:** e80ede0.

**2. [Rule 1 - Bug] `json_format_selected` double-init panic + `fmt::layer()` inference**
- **Found during:** Task 2 (test compile + design of the format-selection assertion).
- **Issue:** The global `tracing` subscriber installs once per process, so calling `init_tracing` for both formats in one test would panic on the second `.init()`. A first attempt mirroring only the layer construction failed to compile (`fmt::layer()` cannot infer its subscriber type param outside a registry).
- **Fix:** Built the same `EnvFilter` + format-specific `fmt` layer `init_tracing` uses, attached to a `registry()`, and installed it with `set_default()` (a scoped drop guard) rather than the global `.init()`. Both formats are exercised in one test (each emits a real event), with no double-init. This proves the exact format-selection branch `init_tracing` takes without contending for the global subscriber.
- **Files modified:** `tests/observe.rs`.
- **Commit:** e80ede0.

## Threat Mitigations Applied

- **T-04-05** (Information Disclosure — `/metrics` exposes relay urls + coverage): observe.rs adds NO auth by design on a trusted bind; the loopback default binding lives in config (04-02 `metrics_addr = "127.0.0.1:9100"`) and the server bind (04-05). observe.rs only exposes the router.
- **T-04-06** (DoS — high-cardinality per-pubkey labels OOM the recorder): only aggregate gauges/histograms are introduced (`frontier_depth`, `crawl_coverage_ratio`, `relay_consecutive_failures`, `relay_active_count` as scalars; latency/staleness as histograms). No per-pubkey labels. The existing `relay_rate_limited{relay=…}` / `relay_blocked{relay=…}` labels are bounded by the curated relay set (untouched).
- **T-04-07** (DoS self — slow `/health/ready` DB ping stalls graceful shutdown): `ready_handler` wraps the `SELECT 1` ping in `tokio::time::timeout(500ms, ..)`; a timeout maps to 503, so a hung DB cannot block shutdown. The `unreachable-DB → 503` test case exercises this path.

## Known Stubs

None. The five `tests/observe.rs` stubs this plan owns are now real, passing tests (zero `#[ignore]` remain among them). `progress_summary_counts` remains a named `#[ignore]` stub by design — it is filled in 04-04 (the OBS-04 sampler/progress plan), as annotated in the file. The exported metric-name constants are forward wiring points the 04-04 sampler emits into — not stubs.

## TDD Gate Compliance

Task 2 is `tdd="true"` but is a test-authoring-plus-dashboard task against the already-built Task 1 surface (the behavior under test — the router, handlers, recorder install — shipped in Task 1 / commit 0ea5417). The tests were written and verified green in the same commit (e80ede0); there is no separate RED commit because the implementation (Task 1) preceded the test plan by design (the plan sequences observe.rs first, then its tests). All five tests pass against the committed implementation.

## Self-Check: PASSED

- Created files exist: `src/daemon/observe.rs`, `ops/grafana-dashboard.json` — both FOUND.
- Commits present in git history: 0ea5417 (Task 1), e80ede0 (Task 2) — both FOUND.
