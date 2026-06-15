---
phase: 04-daemon-staleness-loop-observability
plan: 04
subsystem: daemon-runtime
tags: [daemon, loop, cancellation, sampler, metrics, staleness, churn, observability]
requires:
  - "crawl::run_crawl primitives (seed_anchor, reclaim_stale_on_startup, claim_batch, process_batch, Semaphore, join_worker)"
  - "crawl::frontier::reclaim_stale_by_ttl + reclaim_in_progress_older_than (04-01)"
  - "daemon::config::Config intervals/ttl/reclaim_age (04-02)"
  - "daemon::observe metric-name constants + AppState.loop_alive (04-03)"
  - "relay::rate_limit::RateLimiterRegistry::{failure_count, active_relay_count}"
  - "tests/common ScriptedGraph + fresh_db/status_of (04-01)"
provides:
  - "daemon::loop_::run_daemon_loop — cancellation-aware continuous crawl (OPS-02/FRESH-02)"
  - "daemon::sampler::{frontier_counts, sample_gauges, progress_summary, staleness_timer, in_run_reclaim_timer}"
  - "daemon::sampler::FrontierCounts {discovered,in_progress,fetched,not_found,failed,total} + coverage()"
  - "FRESH-03 churn-on-change proof (tests/graph_writer.rs::churn_recorded_on_change)"
  - "OPS-02 graceful-drain proof (zero orphaned in_progress leases)"
  - "OBS-04 progress-summary count proof"
affects:
  - "04-05 (main bootstrap wires run_daemon_loop + the four sampler tasks under a JoinSet + signal->token)"
tech-stack:
  added: []
  patterns:
    - "cancel at the claim boundary only; never wrap process_batch in select! (Pitfall 4)"
    - "tokio::time::interval + select!-on-CancellationToken background timer shape"
    - "single GROUP BY status aggregate feeding every gauge per tick (Pitfall 6)"
    - "EXTRACT(EPOCH ...)::double precision cast in SQL to keep an f64 bind (no bigdecimal dep)"
    - "aggregate-only gauges, no per-pubkey labels (Pitfall 7)"
key-files:
  created:
    - "src/daemon/loop_.rs"
    - "src/daemon/sampler.rs"
  modified:
    - "src/daemon/mod.rs"
    - "tests/daemon_loop.rs"
    - "tests/graph_writer.rs"
    - ".sqlx/ (3 new query files)"
decisions:
  - "loop_alive set true AFTER the startup seed+reclaim writes so /health/ready only flips once the loop can make progress"
  - "staleness-age histogram samples a capped (LIMIT 1000) age aggregate per tick, not the whole table — distribution shape over exact enumeration (Pitfall 6)"
  - "relay-health gauge emits the MAX consecutive-failure count across the curated set (aggregate), not per-relay labels — per-relay counters already exist as relay_rate_limited"
  - "sample_gauges takes the curated relays Vec so it can fold failure_count across exactly the curated set"
metrics:
  duration_min: 21
  tasks: 3
  files: 5
  completed: "2026-06-15"
---

# Phase 4 Plan 04: Daemon Loop + Sampler + FRESH-03 Churn Summary

The library's proven crawl mechanics become a long-running, signal-drainable, observable daemon loop: a NEW `run_daemon_loop` that reuses every Phase 3 primitive verbatim and changes only the loop control (idle-poll instead of terminate, cancel-at-claim-boundary drain), a `sampler` module of four coarse-interval background tasks (gauges, progress summary, staleness scan, in-run reclaim) that all stop on cancellation, and the FRESH-03 churn-on-change assertion — all green over a real Postgres, with `run_crawl` and its 12 frontier tests untouched.

## What Was Built

**Task 1 — `run_daemon_loop` (commit 8d5db43):** `src/daemon/loop_.rs` with `pub async fn run_daemon_loop<F, Fut>` carrying the SAME generic bounds as `run_crawl`. It reuses `seed_anchor` + `reclaim_stale_on_startup` (startup orphan reclaim KEPT), the `Arc<Semaphore>` owned-permit-before-spawn backpressure (CRAWL-04), `process_batch`, the opportunistic finished-worker reap, and `join_worker` verbatim. The ONLY new control logic: `token.is_cancelled()` checked before each claim (stop claiming on shutdown), and on an empty batch a `tokio::select!` on `token.cancelled()` vs `tokio::time::sleep(idle_poll_interval)` so the loop idle-polls instead of terminating (FRESH-02). The spawned `process_batch` future is never wrapped in `select!` (Pitfall 4 / T-04-09). After the loop breaks, every remaining worker is drained via `join_worker` so each leased row reaches a terminal status — zero orphaned `in_progress` (OPS-02 / T-04-08). `loop_alive` is set `true` after the startup writes. `run_crawl` is untouched.

**Task 2 — sampler (commit a130f1f):** `src/daemon/sampler.rs` with five public async fns. `frontier_counts` runs one `SELECT status, COUNT(*) ... GROUP BY status` into `FrontierCounts {discovered, in_progress, fetched, not_found, failed, total}` with a `coverage()` helper guarding `total == 0`. `sample_gauges` is a coarse-interval `tokio::time::interval` loop (select!-on-cancel) emitting the `frontier_depth` + `crawl_coverage_ratio` gauges, a `staleness_age_seconds` histogram from a capped age aggregate, and `relay_active_count` + `relay_consecutive_failures` (max across the curated set) gauges — NO per-pubkey labels (Pitfall 7). `progress_summary` logs the same counts via `tracing::info!` (OBS-04). `staleness_timer` calls `reclaim_stale_by_ttl` + increments `staleness_reenqueued_total` (FRESH-02). `in_run_reclaim_timer` calls `reclaim_in_progress_older_than` + increments `in_run_reclaimed_total` (OPS-02). All four stop on `token.cancelled()`. `.sqlx` regenerated for the two new aggregate queries.

**Task 3 — tests + FRESH-03 (commit 3f2acee):** `tests/daemon_loop.rs` filled with three real `#[tokio::test]` integration tests using the promoted `ScriptedGraph` + an injected `CancellationToken`: `graceful_drain_no_orphan_leases` (cancel after progress → loop returns Ok → zero `in_progress`, the OPS-02 guarantee), `idle_then_resume_after_reenqueue` (anchor drains → loop idles → `upsert_pubkey` enqueues a new row mid-idle → loop wakes and fetches it → cancel + drain, the FRESH-02 distinction), and `progress_summary_counts` (hand-seeded status mix → `frontier_counts` reports exact per-status counts + coverage = 0.5, the OBS-04 proof). `tests/graph_writer.rs` gained `churn_recorded_on_change`: two distinct events with a changed followee set bump `change_count` (1 → 2) and advance `last_changed_at` while `fetch_count` bumps every apply (FRESH-03), the changed contrast to the existing `same_event_zero_touch`.

## Verification Results

- `SQLX_OFFLINE=true cargo test --test daemon_loop --test graph_writer -- --test-threads=2` — 3 + 4 passed.
- `SQLX_OFFLINE=true cargo test --test frontier -- --test-threads=2` — 12 passed (run_crawl untouched, no regression).
- `SQLX_OFFLINE=true cargo build --all-targets` — exit 0.
- `cargo clippy --all-targets` — no warnings on `src/daemon/loop_.rs`, `src/daemon/sampler.rs`, or `tests/daemon_loop.rs` (pre-existing warnings in unrelated test files are out of scope).
- `cargo sqlx prepare --check -- --all-targets` — clean (zero drift); 3 new `.sqlx` files (status aggregate, staleness-age, churn-assertion query).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] `EXTRACT(EPOCH ...)` NUMERIC bind needed an explicit `double precision` cast**
- **Found during:** Task 2 (`cargo sqlx prepare`).
- **Issue:** The staleness-age aggregate used `EXTRACT(EPOCH FROM (now() - last_fetched_at))`, which Postgres returns as `NUMERIC`; sqlx would map that to `bigdecimal`/`BigDecimal` (not an enabled feature), breaking compile.
- **Fix:** Cast in SQL — `EXTRACT(EPOCH FROM ...)::double precision AS age_secs` — so sqlx infers a plain `Option<f64>` bind. No bigdecimal dependency added; the histogram consumes the `f64` directly.
- **Files modified:** `src/daemon/sampler.rs`.
- **Commit:** a130f1f.

**2. [Rule 3 - Blocking] `sample_gauges` needed the curated relay list to fold relay-health failure counts**
- **Found during:** Task 2.
- **Issue:** `RateLimiterRegistry::failure_count(relay_url)` is per-relay; the aggregate "max consecutive failures" gauge needs to fold over exactly the curated relay set, which the registry does not enumerate on its own.
- **Fix:** Added a `relays: Vec<String>` parameter to `sample_gauges` so it folds `failure_count` across the curated set (the bounded curated relays, never per-pubkey — Pitfall 7 honored). 04-05's bootstrap passes the configured relay list.
- **Files modified:** `src/daemon/sampler.rs`.
- **Commit:** a130f1f.

### Note (no deviation): tokio-util `sync` feature

An initial attempt to add a `sync` feature to `tokio-util` for `CancellationToken` failed (no such feature gate exists). `tokio_util::sync::CancellationToken` is available with the already-enabled `rt` feature, so `Cargo.toml` was reverted to its prior state — no dependency change in this plan.

## Threat Mitigations Applied

- **T-04-08** (shutdown orphans `in_progress` leases): cancel at the claim boundary + drain in-flight workers via `join_worker`; `graceful_drain_no_orphan_leases` asserts zero `in_progress` after cancel.
- **T-04-09** (select! aborts an in-flight `apply_follow_list` txn): `process_batch` is never wrapped in `select!`; cancellation lands only at the claim boundary and the in-flight batch finishes during the drain.
- **T-04-10** (sampler aggregate starves the crawl): one cheap `GROUP BY status` per tick on a coarse configurable interval; the staleness-age aggregate is `LIMIT`-capped so the histogram never walks the whole table.

## Self-Check: PASSED

- `src/daemon/loop_.rs` — FOUND.
- `src/daemon/sampler.rs` — FOUND.
- Commit 8d5db43 (Task 1) — FOUND.
- Commit a130f1f (Task 2) — FOUND.
- Commit 3f2acee (Task 3) — FOUND.
