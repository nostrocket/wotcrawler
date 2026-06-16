---
phase: 04-daemon-staleness-loop-observability
plan: 01
subsystem: daemon-foundation
tags: [daemon, staleness, frontier, migration, deps, scaffold]
requires:
  - "crawl::frontier (Phase 3 reclaim/claim primitives)"
  - "crawl::run_crawl + join_worker (Phase 3 bounded worker loop)"
  - "migrations 0001/0002 (pubkeys.status, last_fetched_at, churn columns)"
provides:
  - "Cargo deps: clap, tracing, tracing-subscriber, metrics-exporter-prometheus, axum, tokio-util, humantime-serde"
  - "[[bin]] crawler -> src/main.rs (placeholder entry)"
  - "src/daemon module root (keystone for 04-02..04-05)"
  - "migration 0003: pubkeys_last_fetched_idx (index-only)"
  - "frontier::reclaim_stale_by_ttl (FRESH-02)"
  - "frontier::reclaim_in_progress_older_than (OPS-02)"
  - "crawl::join_worker is pub(crate) for daemon-loop reuse"
  - "shared tests/common ScriptedGraph + fresh_db/pk/status_of/follows_event"
  - "Wave 0 test scaffolds: daemon_config, daemon_loop, observe (named #[ignore])"
affects:
  - "04-02 (config), 04-03 (observe), 04-04 (loop), 04-05 (main bootstrap)"
tech-stack:
  added:
    - "clap 4.6.1 (derive)"
    - "tracing 0.1.44"
    - "tracing-subscriber 0.3.23 (env-filter, json, fmt)"
    - "metrics-exporter-prometheus 0.18.3 (default-features = false)"
    - "axum 0.8.9"
    - "tokio-util 0.7.18 (rt)"
    - "humantime-serde 1.1.1"
    - "tower 0.5 (dev-dep)"
  patterns:
    - "single-statement-on-pool sweep UPDATE (no long txn) keyed on indexed last_fetched_at"
    - "make_interval(secs => $1::double precision) for index-friendly age comparison"
key-files:
  created:
    - "src/main.rs"
    - "src/daemon/mod.rs"
    - "migrations/0003_staleness.sql"
    - "tests/staleness.rs"
    - "tests/daemon_config.rs"
    - "tests/daemon_loop.rs"
    - "tests/observe.rs"
  modified:
    - "Cargo.toml"
    - "src/lib.rs"
    - "src/crawl/mod.rs"
    - "src/crawl/frontier.rs"
    - "tests/common/mod.rs"
    - "tests/frontier.rs"
decisions:
  - "make_interval secs is Postgres double precision, not int — bind i64 seconds as f64 at the boundary, keep i64 caller-facing API"
  - "cargo sqlx prepare must run with -- --all-targets so integration-test query metadata is retained (lib-only prepare prunes it)"
metrics:
  duration_min: 24
  tasks: 3
  files: 14
  completed: "2026-06-15"
---

# Phase 4 Plan 01: Daemon Foundation Summary

Phase 4 wiring keystone: new daemon dependency surface + `crawler` binary target, index-only migration 0003, two new frontier staleness/reclaim sweeps (FRESH-02 / OPS-02) proven green over a real DB, `join_worker` opened for loop reuse, `daemon` module registered, and the Wave 0 test scaffolds (config/loop/observe) plus a shared `ScriptedGraph` harness in place for later plans to fill.

## What Was Built

**Task 1 — deps + bin + module (commit c674718):** Added `clap`, `tracing`, `tracing-subscriber`, `metrics-exporter-prometheus` (`default-features = false`, built-in HTTP listener feature OFF — the axum router owns `/metrics`), `axum`, `tokio-util`, `humantime-serde`, and dev-dep `tower`; bumped `tokio` features to add `signal` + `time`; added `[[bin]] crawler -> src/main.rs` with a placeholder `main()`; created `src/daemon/mod.rs` (module root with the OPS/OBS/FRESH-02 requirement map); registered `pub mod daemon` in `src/lib.rs`; made `crawl::join_worker` `pub(crate)` so the daemon loop can reuse it.

**Task 2 — migration 0003 + frontier sweeps (commit d73259c):** `migrations/0003_staleness.sql` is index-only and idempotent (`CREATE INDEX IF NOT EXISTS pubkeys_last_fetched_idx ON pubkeys (last_fetched_at)`) — NO columns added (churn columns pre-exist from 0001 per RESEARCH A3). Added `frontier::reclaim_stale_by_ttl` (FRESH-02: re-enqueue `fetched`/`not_found`/`failed` rows past the TTL into `discovered`, resetting `claimed_at`/`fetch_attempts`) and `frontier::reclaim_in_progress_older_than` (OPS-02: age-gated in-run reclaim that never resets freshly-claimed live leases). `.sqlx` regenerated for both new queries (`prepare --check` clean).

**Task 3 — tests + scaffolds + shared harness (commit 979152f):** Promoted `ScriptedGraph` + `follows_event`/`pk`/`fresh_db`/`status_of` from `tests/frontier.rs` into `tests/common/mod.rs` (frontier.rs now uses the promoted versions; its 12 tests still green). `tests/staleness.rs` ships 4 real DB tests covering both sweeps. Wave 0 scaffolds `tests/daemon_config.rs` (4), `tests/daemon_loop.rs` (2), `tests/observe.rs` (6) created as named `#[ignore]` stubs.

## Verification Results

- `SQLX_OFFLINE=true cargo build --all-targets` — exit 0.
- `cargo test --test staleness -- --test-threads=2` — 4 passed.
- `cargo test --test frontier -- --test-threads=2` — 12 passed (no regression from ScriptedGraph promotion).
- `cargo test --test daemon_config --test daemon_loop --test observe -- --list` — lists 4 + 2 + 6 named stubs.
- `cargo clippy --all-targets` — no new warnings on touched files (5 pre-existing warnings in untouched test files logged to `deferred-items.md`).
- `cargo sqlx prepare --check -- --all-targets` — clean (zero drift), 20 `.sqlx` files (18 prior + 2 new).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] `make_interval` secs parameter type mismatch**
- **Found during:** Task 2 (`cargo sqlx prepare`).
- **Issue:** The plan specified `make_interval(secs => $1)` with a "plain `i64`" bind, but Postgres `make_interval`'s `secs` parameter is `double precision`, so sqlx inferred the bind as `f64` and the `i64` bind failed to compile (E0308).
- **Fix:** Kept the caller-facing API as `i64` seconds (per plan intent), and at the SQL/bind boundary used `make_interval(secs => $1::double precision)` with `ttl_secs as f64` / `age_secs as f64`. Behavior is identical; the index on `last_fetched_at` still supports the comparison.
- **Files modified:** `src/crawl/frontier.rs`.
- **Commit:** d73259c.

**2. [Rule 3 - Blocking] `cargo sqlx prepare` must target all targets**
- **Found during:** Task 2.
- **Issue:** A lib-only `cargo sqlx prepare` pruned 7 `.sqlx` query files belonging to existing integration-test binaries (they have their own `sqlx::query!` calls), which would break offline test builds.
- **Fix:** Ran `cargo sqlx prepare -- --all-targets` so test-binary query metadata is retained; result is 18 prior + 2 new = 20 files, zero deletions.
- **Files modified:** `.sqlx/` (2 new query files).
- **Commit:** d73259c.

## Threat Mitigations Applied

- **T-04-01** (DoS — full-table-scan staleness sweep): migration 0003 adds `pubkeys_last_fetched_idx`; the sweep keys on the indexed `last_fetched_at`.
- **T-04-02** (Tampering — in-run reclaim resetting a live lease): `reclaim_in_progress_older_than` keys on `claimed_at < now() - age`; the `in_progress_only_reclaims_old` test proves a freshly-claimed lease is left untouched.

## Known Stubs

The Wave 0 test scaffolds (`tests/daemon_config.rs`, `tests/daemon_loop.rs`, `tests/observe.rs`) are intentional named `#[ignore]` stubs (`unimplemented!()` bodies) to be filled by plans 04-02/04-03/04-04/04-05, and `src/main.rs` / `src/daemon/mod.rs` are intentional placeholders (real bootstrap in 04-05). These are the planned Wave 0 deliverables, not unresolved stubs — each is annotated with the filling plan.

## Self-Check: PASSED

All created files exist (src/main.rs, src/daemon/mod.rs, migrations/0003_staleness.sql, tests/staleness.rs, tests/daemon_config.rs, tests/daemon_loop.rs, tests/observe.rs) and all three task commits (c674718, d73259c, 979152f) are present in git history.
