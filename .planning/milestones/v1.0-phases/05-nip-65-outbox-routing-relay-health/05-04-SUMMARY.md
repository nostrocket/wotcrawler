---
phase: 05-nip-65-outbox-routing-relay-health
plan: 04
subsystem: daemon
tags: [RELAY-06, relay-health, routing, per-relay-concurrency, semaphore, deadlock-safe, nip-65-fallback, metrics, grafana]
requires:
  - "05-02: RelayHealthRegistry (score/permits/in_use/incr_in_use/decr_in_use/route_allowed/mark_attempt, record_success/timeout/connect_failure/rate_limited); the daemon `health` binding (Arc<RelayHealthRegistry>::new(cfg.health_alpha)); the 5 RELAY-06 config knobs"
  - "05-03: process_batch fallback params (fallback_enabled, nip65_max_write_relays, &RelayHealthRegistry, injected fallback_fetch + relay_list_fetch closures) + the no-op closures at run_daemon_loop to replace"
  - "05-01: store::relays write-relay lookup; URL-aware ScriptedGraph + relay_list_event fixture; nip65 import path"
provides:
  - "relay::fetch::record_fetch_health (per-relay Ok/Err -> health signal classification in the relay layer)"
  - "relay::health::admit_per_relay (the single deadlock-safe per-relay admission gate: fixed Semaphore + health-scaled in-use gate + drop-guard decrement)"
  - "daemon health-driven fan-out: skip-below-threshold+probe routing, per-relay Semaphore map, health capture at the fetch arms, documented global->per-relay->GCRA->fetch order"
  - "live-Client fallback_fetch (kind-3 from write relays) + relay_list_fetch (plain curated kind:10002) closures threaded into process_batch via run_daemon_loop (shared health, fallback config)"
  - "observe::{METRIC_RELAY_HEALTH, METRIC_NIP65_RECOVERED, METRIC_RELAY_CONCURRENCY} consts"
  - "sampler labeled per-relay health + concurrency-in-use gauges over the bounded curated set only"
  - "Grafana panels: relay_health (labeled), nip65_recovered_total rate, relay_concurrency_in_use (labeled)"
  - "tests/nip65_fallback::no_deadlock_single_permit (green): multi-relay fan-out at per_relay_concurrency=1 completes under a bounded timeout"
affects: []
tech-stack:
  added: []
  patterns:
    - "Single shared per-relay admission helper (admit_per_relay) used by BOTH the live daemon fan-out and the deadlock test, so the acquisition order is identical everywhere it is exercised (testability + no drift)"
    - "tokio::Semaphore fixed-size + health-scaled in-use gate (Pitfall 5: Semaphore cannot shrink) with a Drop guard that decrements in_use on every exit path so a failed/panicking fetch never leaks a slot"
    - "RelayError-variant -> health-signal classification factored into the relay layer (record_fetch_health) where the variants live, called at the fan-out outcome site"
    - "Live-Client fallback/relay-list closures own the only Client references; crawl/apply.rs stays Client-free (extends the 05-03 injected-closure seam)"
key-files:
  created: []
  modified:
    - "src/relay/fetch.rs (record_fetch_health classification helper)"
    - "src/relay/health.rs (admit_per_relay deadlock-safe admission gate)"
    - "src/daemon/mod.rs (health-driven fan-out, per-relay Semaphore map, live fallback_fetch + relay_list_fetch closures, sampler+loop threading of the shared health binding)"
    - "src/daemon/loop_.rs (run_daemon_loop gains fallback_enabled/nip65_max_write_relays/shared health + both closures, passed to process_batch)"
    - "src/daemon/observe.rs (3 metric-name consts)"
    - "src/daemon/sampler.rs (Arc<RelayHealthRegistry> param + labeled curated-only health & concurrency gauges)"
    - "ops/grafana-dashboard.json (3 panels)"
    - "tests/nip65_fallback.rs (no_deadlock_single_permit body)"
    - "tests/daemon_loop.rs (run_daemon_loop call sites + new fallback-disabled args)"
    - "tests/observe.rs (dashboard_json_valid asserts the 3 new series)"
decisions:
  - "Health-classification mapping lives in relay::fetch::record_fetch_health (relay layer owns RelayError variants); the fan-out (daemon) measures latency around the per-relay fetch and calls it at the Ok/Err arm BEFORE the ? requeue propagation, so a per-relay error still requeues the batch (D-09) but is observed first."
  - "Extracted admit_per_relay into relay::health so the deadlock test exercises the SAME admission + acquisition order the live fan-out uses (the fan-out itself is a live-Client closure and not directly unit-testable). The in-use gate spins via tokio::task::yield_now (no lock across await), permits floors at 1 so a degraded relay always admits a probe."
  - "Per-relay Semaphore map is built once over the BOUNDED curated set (cfg.relays) sized Semaphore::new(per_relay_concurrency); health scales effective concurrency via the in-use gate, never by resizing (Pitfall 5)."
  - "relay_list_fetch is a PLAIN curated kind:10002 fetch (Kind::RelayList), NOT routed through the kind-3 fallback (Pitfall 4: no recursion)."
  - "Labeled gauges (relay_health, relay_concurrency_in_use) emitted ONLY over the curated set; transient write relays stay in-memory for routing (Pitfall 7 cardinality)."
metrics:
  duration: 30min
  completed: 2026-06-15
  tasks: 5
  files: 10
---

# Phase 05 Plan 04: Health-Driven Routing, Per-Relay Concurrency & Live NIP-65 Fallback Wiring Summary

The relay health score now visibly drives routing and per-relay concurrency (RELAY-06): the daemon fan-out skips relays below `relay_health_threshold` (with periodic probe re-admission), gates each relay through a fixed per-relay `Semaphore` + a health-scaled in-use admission gate, captures the per-relay fetch outcome (success+latency / timeout / connect-failure) into the shared `RelayHealthRegistry` at the Ok/Err arms, and observes a fixed deadlock-safe acquisition order (global crawl permit -> per-relay permit -> GCRA token -> fetch). The live-Client `fallback_fetch` + on-demand `relay_list_fetch` closures (from 05-03) are now built against the connected `Client` and threaded — together with the shared `health` binding and the fallback config — into `process_batch`. Per-relay health + concurrency-in-use gauges (curated set only) + the `nip65_recovered` counter are exported and rendered in three new Grafana panels.

## What Was Built

**Task 1 — health capture seam (`src/relay/fetch.rs`):** `record_fetch_health(health, relay_url, latency, &outcome)` maps a per-relay `Result<Vec<Event>, RelayError>` to the health signal in the relay layer that owns the variants — `Ok` -> `record_success(latency)`, `FetchTimeout` -> `record_timeout`, every other `Err` (incl. `Client`) -> `record_connect_failure` (RESEARCH A4). No change to pagination/timeout/GCRA.

**Task 2 — health-driven fan-out + admission gate (`src/relay/health.rs`, `src/daemon/mod.rs`):**
- `admit_per_relay(health, sem, relay, per_relay_concurrency, fetch)`: acquires the fixed per-relay `Semaphore` permit, spins on the health-scaled in-use gate (`in_use < permits(relay)`, `permits` floors at 1) without holding a lock across the await, increments in-use, runs `fetch`, and decrements via a `Drop` guard on every exit path. The doc comment states the fixed order.
- The fan-out: skips a relay when `!route_allowed(relay_url, threshold)`, calls `mark_attempt`, builds a per-relay `Semaphore::new(per_relay_concurrency)` map over the curated set once, admits via `admit_per_relay`, times the round-trip, and calls `record_fetch_health` BEFORE the `?` requeue. Reuses the single 05-02 `health` binding (`cfg.health_alpha`); no second registry.

**Task 3 — live fallback closures (`src/daemon/mod.rs`, `src/daemon/loop_.rs`):** `fallback_fetch(author, write_relays)` fetches kind-3 from the given write relays through the GCRA-gated path; `relay_list_fetch(author)` does a plain curated `Kind::RelayList` fetch (no recursion). `run_daemon_loop` now takes `fallback_enabled`/`nip65_max_write_relays`/`Arc<RelayHealthRegistry>` + both closures and passes them to `process_batch`. The closures own the live `Client`; `crawl/apply.rs` stays Client-free.

**Task 4 — metrics + sampler (`src/daemon/observe.rs`, `src/daemon/sampler.rs`):** Added `METRIC_RELAY_HEALTH`, `METRIC_NIP65_RECOVERED`, `METRIC_RELAY_CONCURRENCY`. `sample_gauges` takes `Arc<RelayHealthRegistry>` and emits `relay_health` + `relay_concurrency_in_use` labeled by `relay` ONLY over the curated `relays` slice (Pitfall 7).

**Task 5 — Grafana + deadlock test (`ops/grafana-dashboard.json`, `tests/nip65_fallback.rs`, `tests/observe.rs`):** 3 panels (`relay_health`, `sum(rate(nip65_recovered_total[5m]))`, `relay_concurrency_in_use`); `no_deadlock_single_permit` un-ignored and implemented — 8 concurrent batches across 3 single-permit relays under the global->per-relay->GCRA order via `admit_per_relay`, asserting completion within a 10s timeout (a hang fails). `dashboard_json_valid` strengthened to assert the 3 new series.

## Verification

- `SQLX_OFFLINE=true cargo build --tests` — green (no new `query!`; no `.sqlx` regen needed).
- `cargo test --test nip65_fallback --test relay_health -- --test-threads=2` — 5 + 3 passed (incl. the now-green `no_deadlock_single_permit`).
- `cargo test --test daemon_loop` / `--test frontier` / `--test graph_writer` — all pass on re-run / individually (16 daemon_loop+frontier+graph_writer tests). See Issues: the testcontainers port-exposure flake hit different tests each run; every test passes individually and on a clean re-run — no logic regression.
- `cargo test --test daemon_config` — 14 passed (incl. the 5 RELAY-06 config tests).
- `cargo test --test observe dashboard_json_valid` — passed; `ops/grafana-dashboard.json` parses (9 panels).
- `cargo clippy --all-targets` — clean of any new warnings (the one I introduced was fixed; remaining warnings are pre-existing in unrelated test files — out of scope).
- Grep gates: single `RelayHealthRegistry::new` in `src/daemon/mod.rs`; `route_allowed`/`mark_attempt`/`admit_per_relay`/`record_fetch_health` present; `fallback_fetch`+`relay_list_fetch` present; `crawl/apply.rs` has zero `Client` references; labeled gauges inside the curated `for r in &relays` loop only; `nip65_recovered` fired un-suffixed.

## Task Commits

1. **Task 1** — `de913b1` (feat): record_fetch_health classification helper.
2. **Task 2** — `5179562` (feat): admit_per_relay + health-driven fan-out + deadlock-safe order.
3. **Task 3** — `903e99b` (feat): live fallback_fetch + relay_list_fetch threaded into process_batch.
4. **Task 4** — `17f17c6` (feat): metric consts + labeled curated-only gauges.
5. **Task 5** — `b22c628` (test): Grafana panels + no_deadlock_single_permit; `6ed863b` (style): clippy cleanup.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing critical helper] Extracted `admit_per_relay` into `relay::health`**
- **Found during:** Task 2 / Task 5
- **Issue:** The deadlock-safety test (`no_deadlock_single_permit`) must prove the global->per-relay->GCRA order, but the live fan-out is a closure capturing the live `Client` and cannot be unit-tested. Inlining the admission loop in the fan-out would leave the test asserting a re-implemented order (drift risk).
- **Fix:** Factored the per-relay admission (Semaphore permit + in-use gate + Drop-guard decrement) into a single public `admit_per_relay` helper that BOTH the fan-out and the test call, so the order is identical everywhere it runs.
- **Files modified:** src/relay/health.rs, src/daemon/mod.rs, tests/nip65_fallback.rs
- **Commit:** 5179562, b22c628

**2. [Rule 3 - Blocking compile] Updated `tests/daemon_loop.rs` `run_daemon_loop` call sites**
- **Found during:** Task 3
- **Issue:** Extending `run_daemon_loop`'s signature with the 5 fallback params broke its 3 direct callers in `tests/daemon_loop.rs`.
- **Fix:** Appended `fallback_enabled=false`, a default max-write-relays, a fresh `RelayHealthRegistry`, and no-op closures at all 3 sites — behavior identical (these tests exercise loop control, not the fallback, which is covered in `tests/nip65_fallback.rs`).
- **Files modified:** tests/daemon_loop.rs
- **Commit:** 903e99b

**3. [Rule 2 - Test strengthening] `dashboard_json_valid` now asserts the 3 new series**
- **Found during:** Task 5
- **Issue:** The OBS-05 dashboard test only verified the pre-existing series; the 3 new panels would not be guarded against accidental removal.
- **Fix:** Added `METRIC_RELAY_HEALTH`/`METRIC_NIP65_RECOVERED`/`METRIC_RELAY_CONCURRENCY` to the assertion list.
- **Files modified:** tests/observe.rs
- **Commit:** b22c628

## Issues Encountered

- **Testcontainers port-exposure flake** (`container '<id>' does not expose port 5432/tcp`): intermittently fails 1-2 DB-backed tests when several Postgres containers start concurrently. The failing test differs every run, and every affected test (`daemon_loop`, `frontier`) passes individually and on a clean re-run. Confirmed environmental (Docker port-mapping race), not a logic regression — my `daemon_loop.rs` change only appends behavior-preserving fallback-disabled args. Matches the documented re-run-once guidance.

## TDD Gate Compliance

Task 5 was `tdd="true"`. The `behavior` (deadlock-free fan-out at `per_relay_concurrency=1`) is verified by `no_deadlock_single_permit`. The implementation it exercises (`admit_per_relay`) was authored in Task 2 (the routing/admission task), so this was a verification-style test rather than a standalone RED-before-GREEN commit: the test and the helper it asserts were committed in separate commits (5179562 helper, b22c628 test), and the test passes against the real admission gate. No standalone failing-test (RED) commit exists for Task 5.

## Known Stubs

None. The fallback is now fully live: `fallback_enabled` is sourced from config (default `true`), the real Client closures are threaded, and the shared health registry drives routing. The no-op closures remain ONLY in `tests/daemon_loop.rs` (loop-control tests that deliberately don't exercise the fallback — the recovery path is covered end-to-end in `tests/nip65_fallback.rs`).

## Self-Check: PASSED
- All modified files present on disk; `ops/grafana-dashboard.json` parses (9 panels with the 3 new exprs).
- All 6 commits (de913b1, 5179562, 903e99b, 17f17c6, b22c628, 6ed863b) present in git history.
- `no_deadlock_single_permit` green; single `RelayHealthRegistry::new` in daemon/mod.rs; apply.rs Client-free.
