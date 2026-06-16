---
phase: 04-daemon-staleness-loop-observability
verified: 2026-06-15T05:20:00Z
status: human_needed
score: 9/9 must-haves verified
overrides_applied: 0
human_verification:
  - test: "Run the full crawler daemon against a live Postgres and real curated relays"
    expected: "Structured tracing startup logs appear; NO database_url/password in output; /health/live returns 200; /health/ready returns 200 once the crawl is running; /metrics returns Prometheus exposition containing frontier_depth, crawl_coverage_ratio, and validation-failure counters after batches process; SIGTERM drains the loop and exits cleanly with zero in_progress rows; periodic progress-summary log lines appear."
    why_human: "Requires a running Postgres, live relay connectivity, and real-time signal delivery — none automatable in a unit/integration context."
  - test: "Import ops/grafana-dashboard.json into a Grafana instance pointed at the running daemon's /metrics endpoint"
    expected: "All dashboard panels populate with data (frontier_depth, crawl_coverage_ratio, staleness_age_seconds, relay_consecutive_failures, fetch_duration_seconds p95, relay_active_count, and ingest/relay counter panels)."
    why_human: "Requires a running Grafana + Prometheus stack scraping the daemon; cannot be automated programmatically."
---

# Phase 4: Daemon Staleness Loop & Observability Verification Report

**Phase Goal:** A single configurable daemon binary runs the initial crawl then continuous TTL-driven refresh, shuts down gracefully, and exposes enough metrics, logs, and health signals for an operator to trust it running unattended for days.
**Verified:** 2026-06-15T05:20:00Z
**Status:** human_needed
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | The crawler binary `crawler` is declared and compiles | VERIFIED | `cargo build --bin crawler` exits 0; `crawler --help` shows `--config <CONFIG>` |
| 2 | A staleness UPDATE re-enqueues only rows past the TTL, resetting claimed_at and fetch_attempts, and leaves fresh rows untouched | VERIFIED | `reclaim_stale_by_ttl` in `frontier.rs:130-149`; 4 passing tests in `tests/staleness.rs` (reenqueues_only_stale, resets_attempts, not_found_and_failed_also_reenqueued, in_progress_only_reclaims_old) |
| 3 | An in-run reclaim UPDATE resets only in_progress leases older than a threshold, never freshly-claimed live rows | VERIFIED | `reclaim_in_progress_older_than` in `frontier.rs:162-177`; `in_progress_only_reclaims_old` test green |
| 4 | Migration 0003 adds a last_fetched_at index and re-running migrations is a no-op | VERIFIED | `migrations/0003_staleness.sql` uses `CREATE INDEX IF NOT EXISTS pubkeys_last_fetched_idx ON pubkeys (last_fetched_at)` — no ADD COLUMN |
| 5 | The daemon loads its full tunable set from a TOML file given by --config; WOT__* env vars override; invalid config fails fast | VERIFIED | `daemon/config.rs` — `load_config` with `prefix("WOT").separator("__")`; `validate()` checks anchor/relays/ttl/database_url/concurrency/batch_size/reqs_per_second; 9/9 `daemon_config` tests green including CR-02 variants |
| 6 | A single axum server serves /metrics, /health/live, and /health/ready with correct semantics | VERIFIED | `daemon/observe.rs` router; `live_always_ok`, `ready_requires_db_and_loop`, `metrics_endpoint_exposes_series` all pass; `ready_handler` uses bounded `tokio::time::timeout` for DB ping |
| 7 | The continuous loop idle-polls empty frontier, resumes on re-enqueue, and drains with zero in_progress on cancel | VERIFIED | `daemon/loop_.rs` `run_daemon_loop`; cancel checked only at claim boundary; `drain_all` helper awaits all handles; `graceful_drain_no_orphan_leases` + `idle_then_resume_after_reenqueue` + `terminal_stamp_reflects_fetch_time_not_spawn` all green |
| 8 | The sampler emits frontier/coverage/staleness/relay gauges; progress summaries log; staleness and reclaim timers run | VERIFIED | `daemon/sampler.rs` all five functions; `progress_summary_counts` test green; gauge/histogram names reference `observe` constants |
| 9 | FRESH-03 churn: a changed follow list bumps change_count/last_changed_at; an unchanged re-fetch bumps fetch_count only | VERIFIED | `tests/graph_writer.rs::churn_recorded_on_change` green |

**Score:** 9/9 truths verified

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `migrations/0003_staleness.sql` | Index supporting staleness scan over last_fetched_at | VERIFIED | Contains `CREATE INDEX IF NOT EXISTS pubkeys_last_fetched_idx`, no ADD COLUMN |
| `src/crawl/frontier.rs` | reclaim_stale_by_ttl and reclaim_in_progress_older_than | VERIFIED | Both functions at lines 130 and 162; `make_interval(secs => $1::double precision)` bind; reset semantics present |
| `src/daemon/mod.rs` | daemon module root with run() orchestrator | VERIFIED | Full `pub async fn run(cfg: Config)` with 8-step bootstrap order, all tasks spawned, SHUTDOWN_TIMEOUT=30s bounded drain |
| `src/daemon/config.rs` | Config struct + load_config + validate | VERIFIED | All fields with `DEFAULT_*` const references; `Debug` hand-impl redacts `database_url`; validate() has all guards including CR-02 additions |
| `src/daemon/observe.rs` | Tracing init, recorder install, axum router | VERIFIED | `install_metrics()`, `init_tracing()`, `router()`, `build_recorder()` for tests; SERVICE_UNAVAILABLE in ready_handler |
| `src/daemon/loop_.rs` | run_daemon_loop — cancellation-aware continuous crawl | VERIFIED | CR-01 fix (per-batch `Timestamp::now()`); WR-03 fix (loop_alive reset to false); WR-04 fix (drain_all helper); cancel only at claim boundary |
| `src/daemon/sampler.rs` | gauge sampler + progress summary + staleness/reclaim timers | VERIFIED | All 5 functions; WR-02 fix (counter names without manual `_total`); gauge! emits into METRIC_* constants |
| `src/main.rs` | Binary entry: clap parse -> load -> validate -> run | VERIFIED | `--config` clap arg; fail-fast `ExitCode::FAILURE` on load/validate error; `database_url` never printed |
| `config.example.toml` | Documented example config covering every field | VERIFIED | All 17 fields present with inline comments; valid per `example_config_is_valid` test |
| `ops/grafana-dashboard.json` | OBS-05 dashboard referencing all OBS-01 series | VERIFIED | 9 occurrences of metric names; WR-02 fix applied (`_total` suffix in counter panel exprs); `dashboard_json_valid` test green |
| `tests/staleness.rs` | 4 staleness integration tests | VERIFIED | All 4 pass against real DB |
| `tests/daemon_config.rs` | Config load/override/default/validation tests | VERIFIED | 9 tests pass including CR-02 variants |
| `tests/observe.rs` | Metrics render, health semantics, json format, dashboard valid | VERIFIED | 5 tests pass |
| `tests/daemon_loop.rs` | Graceful drain + idle/resume + OBS-04 progress tests | VERIFIED | 4 tests pass including CR-01 regression test |
| `tests/graph_writer.rs` | FRESH-03 churn-on-change assertion | VERIFIED | `churn_recorded_on_change` passes |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|----|--------|---------|
| `src/lib.rs` | `src/daemon/mod.rs` | `pub mod daemon` | VERIFIED | `src/lib.rs:12` |
| `src/daemon/mod.rs` | `daemon::config + observe + loop_ + sampler` | `pub mod` declarations | VERIFIED | Lines 23, 29, 35, 42 in `daemon/mod.rs` |
| `src/main.rs` | `daemon::run + config::load_config/validate` | clap `--config` then load+validate then run | VERIFIED | Lines 51, 63, 70 in `main.rs` |
| `src/daemon/mod.rs::run` | `tokio::signal + CancellationToken` | signal task cancels shared token | VERIFIED | Lines 124, 133-147 in `daemon/mod.rs` |
| `src/daemon/loop_.rs::run_daemon_loop` | `crawl::frontier + crawl::apply + crawl::join_worker` | claim_batch/process_batch/semaphore/join_worker | VERIFIED | Lines 85-212 in `loop_.rs` |
| `src/daemon/sampler.rs` | `frontier::reclaim_stale_by_ttl + reclaim_in_progress_older_than` | periodic timer tasks | VERIFIED | Lines 239, 272 in `sampler.rs` |
| `src/daemon/sampler.rs` | `metrics::gauge!` | DB aggregate -> gauge emit | VERIFIED | Lines 132-162 in `sampler.rs` |
| `src/daemon/observe.rs::metrics_handler` | `PrometheusHandle::render` | axum GET /metrics handler | VERIFIED | `st.handle.render()` at line 180 |
| `src/daemon/observe.rs::ready_handler` | pool + loop_alive flag | SELECT 1 ping + AtomicBool | VERIFIED | Lines 193-201 with bounded timeout |
| `src/daemon/mod.rs` production fetch_union | `relay::fetch::fetch_complete_with_timeout` + `connect_curated` | per-relay fan-out concatenating raw events | VERIFIED | Lines 157-219 in `daemon/mod.rs` |
| `src/crawl/frontier.rs::reclaim_stale_by_ttl` | `pubkeys.last_fetched_at` | UPDATE WHERE last_fetched_at < now() - make_interval | VERIFIED | `last_fetched_at < now() - make_interval(secs => $1::double precision)` |

### Data-Flow Trace (Level 4)

| Artifact | Data Variable | Source | Produces Real Data | Status |
|----------|---------------|--------|--------------------|--------|
| `sampler.rs::sample_gauges` | `counts.discovered` / `counts.coverage()` | `frontier_counts` GROUP BY status aggregate | DB query | FLOWING |
| `sampler.rs::staleness_timer` | `n` (rows re-enqueued) | `reclaim_stale_by_ttl` UPDATE rows_affected | DB write | FLOWING |
| `observe.rs::metrics_handler` | `handle.render()` | PrometheusHandle rendering registered metrics | recorder | FLOWING |
| `observe.rs::ready_handler` | `loop_alive` + DB ping result | `AtomicBool` set by loop + `SELECT 1` | both live sources | FLOWING |
| `loop_.rs::run_daemon_loop` | `batch` | `claim_batch` DB query | DB query + relay fetch | FLOWING |

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| Binary builds | `SQLX_OFFLINE=true cargo build --bin crawler` | exit 0 | PASS |
| --help lists --config | `./target/debug/crawler --help` | "Usage: crawler --config <CONFIG>" | PASS |
| daemon_config tests | `cargo test --test daemon_config -- --test-threads=1` | 9/9 passed | PASS |
| staleness tests | `cargo test --test staleness -- --test-threads=1` | 4/4 passed | PASS |
| observe tests | `cargo test --test observe -- --test-threads=2` | 5/5 passed | PASS |
| daemon_loop tests | `cargo test --test daemon_loop -- --test-threads=1` | 4/4 passed | PASS |
| graph_writer tests | `cargo test --test graph_writer -- --test-threads=1` | 4/4 passed | PASS |
| frontier tests (regression) | `cargo test --test frontier -- --test-threads=1` | 12/12 passed (1 transient container flake on first run, clean on re-run) | PASS |
| Full build all-targets | `SQLX_OFFLINE=true cargo build --all-targets` | exit 0 | PASS |

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|------------|-------------|--------|---------|
| FRESH-02 | 04-01, 04-04, 04-05 | Staleness scanner re-enqueues pubkeys whose knowledge exceeds TTL | SATISFIED | `reclaim_stale_by_ttl` proven by staleness tests; `staleness_timer` in sampler wires it on interval; CR-01 fix ensures not_found/failed get accurate timestamps |
| FRESH-03 | 04-04 | Each refresh records whether follow list actually changed | SATISFIED | `churn_recorded_on_change` test in graph_writer.rs; `change_count`/`last_changed_at` bump on change, `fetch_count` bumps always |
| OBS-01 | 04-03 | Prometheus /metrics exposes crawl coverage, staleness distribution, relay health, frontier depth, fetch rate, validation-failure counts | SATISFIED | All 6 metric constant groups defined in observe.rs; sampler emits them; WR-01 (fetch histogram recorded), WR-02 (counter names fixed), grafana dashboard references all |
| OBS-02 | 04-03 | Structured logging via tracing with configurable levels | SATISFIED | `init_tracing` with EnvFilter + human/JSON format; `json_format_selected` test green |
| OBS-03 | 04-03 | HTTP health endpoint (liveness/readiness) | SATISFIED | axum router with /health/live (200 unconditional) and /health/ready (200 iff loop_alive + DB reachable); WR-03 fix (loop_alive resets to false on shutdown) |
| OBS-04 | 04-04 | Periodic crawl-progress summaries logged during initial crawl | SATISFIED | `progress_summary` in sampler.rs logs frontier/coverage/fetched/total on interval; `progress_summary_counts` test green |
| OBS-05 | 04-03 | Grafana dashboard JSON committed covering all OBS-01 series | SATISFIED | `ops/grafana-dashboard.json` exists; `dashboard_json_valid` test confirms JSON parses and all series names present; WR-02 fix applied |
| OPS-01 | 04-02, 04-05 | Single Rust daemon binary configured via config file | SATISFIED | `[[bin]] name = "crawler"` in Cargo.toml; `--config` clap arg; full tunable set in Config struct; fail-fast validate; database_url never logged |
| OPS-02 | 04-01, 04-04, 04-05 | Graceful shutdown drains in-flight work, leaves DB consistent | SATISFIED | Cancel at claim boundary; `drain_all` helper (WR-04 fix); `loop_alive` reset to false (WR-03 fix); SHUTDOWN_TIMEOUT=30s bounded drain; `graceful_drain_no_orphan_leases` test proves zero orphan leases |

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| Multiple test files | Various | Pre-existing clippy warnings (is_multiple_of, is_err, doc indentation, constant assertion) | Info | Pre-date Phase 4; documented in `deferred-items.md`; none in Phase 4 files |

No TBD/FIXME/XXX debt markers found in any Phase 4 modified files.
No stub implementations found (all return values are substantive, wired to real DB/metric sources).
No per-pubkey metric labels found in sampler.rs (Pitfall 7 compliance confirmed).

### Code Review Fixes Verified

All 6 Critical/Warning findings from 04-REVIEW.md are confirmed resolved in the codebase:

| Finding | Resolution | Verified |
|---------|-----------|---------|
| CR-01: Timestamp::now() frozen at spawn | Per-batch `let now = Timestamp::now()` inside spawned closure (loop_.rs:156) | CONFIRMED |
| CR-02: concurrency/batch_size/reqs_per_second zero not caught | Three `anyhow::ensure!` guards in `validate()` (config.rs:211-213) | CONFIRMED — 3 new tests green |
| WR-01: fetch_duration histogram never recorded | `metrics::histogram!(METRIC_FETCH_DURATION).record(t0.elapsed())` in loop_.rs:168-170 | CONFIRMED |
| WR-02: Grafana counter queries missing `_total` suffix | Dashboard exprs updated with `_total`; sampler counters renamed without manual `_total` | CONFIRMED |
| WR-03: loop_alive never reset on shutdown | `loop_alive.store(false, Ordering::Relaxed)` at loop_.rs:221 | CONFIRMED — `graceful_drain_no_orphan_leases` asserts this |
| WR-04: drain abort on first join_worker error | `drain_all()` helper awaits all, collects first error (loop_.rs:237-252) | CONFIRMED |

### Human Verification Required

### 1. Live Relay Crawl Run

**Test:** Build with `SQLX_OFFLINE=true cargo build --bin crawler`. Copy `config.example.toml` to a local `config.toml`, set a real `database_url`, real `anchor_pubkey`, and leave `metrics_addr = "127.0.0.1:9100"`. Start with `cargo run --bin crawler -- --config config.toml`.

**Expected:** Structured tracing startup logs appear; the database_url/password is NEVER printed in any log line; `curl -s localhost:9100/health/live` returns 200; `curl -s localhost:9100/health/ready` returns 200 once crawl is running; `curl -s localhost:9100/metrics` returns non-empty Prometheus text with `frontier_depth`, `crawl_coverage_ratio`, and ingest counters after at least one batch processes. Periodic `crawl progress` log lines appear at the configured `progress_interval`. Send SIGTERM (Ctrl-C): daemon logs a drain, exits cleanly, and `SELECT count(*) FROM pubkeys WHERE status='in_progress'` is 0.

**Why human:** Requires a live Postgres, real relay connectivity, real SIGTERM delivery, and real-time observation of log output for database_url absence — none automatable in offline/unit test context.

### 2. Grafana Dashboard Rendering (OBS-05)

**Test:** Import `ops/grafana-dashboard.json` into a Grafana instance configured with a Prometheus data source scraping the running daemon's `/metrics` endpoint. Wait one scrape interval.

**Expected:** All panels populate with data: frontier_depth gauge, crawl_coverage_ratio gauge, staleness_age_seconds histogram, relay_consecutive_failures gauge, fetch_duration_seconds p95 panel, relay_active_count gauge, and the ingest/relay rate counter panels (relay_rate_limited_total, ingest_invalid_signature_total, etc.).

**Why human:** Requires a running Grafana + Prometheus stack, live relay data, and visual panel inspection — not automatable.

---

### Gaps Summary

No gaps. All 9 must-have truths are VERIFIED against actual code. All 6 code review findings are confirmed resolved. All 9 requirements (FRESH-02, FRESH-03, OBS-01–05, OPS-01–02) are SATISFIED by implementation evidence. 

Status is `human_needed` only because 2 items require a live relay + Grafana run — the automated portion is complete and clean.

---

_Verified: 2026-06-15T05:20:00Z_
_Verifier: Claude (gsd-verifier)_
