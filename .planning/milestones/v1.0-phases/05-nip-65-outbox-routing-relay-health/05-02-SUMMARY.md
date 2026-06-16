---
phase: 05-nip-65-outbox-routing-relay-health
plan: 02
subsystem: relay
tags: [relay-health, ewma, relay-06, config, fail-fast, rate-limit, notice-consumer]

# Dependency graph
requires:
  - phase: 05-nip-65-outbox-routing-relay-health
    plan: 01
    provides: "error-injecting ScriptedGraph seam (RelayFailure::Timeout/NotFound) + store::relays; nip65 import path nostr_sdk::nips::nip65"
  - phase: 04-relay-health-staleness
    provides: "RateLimiterRegistry shape (Mutex<HashMap>-behind-Arc, record_*/introspection); Config field+default-fn+validate fail-fast idiom; hand-impl Debug redaction"
provides:
  - "RelayHealthRegistry (src/relay/health.rs): per-relay EWMA health score in [0,1], unknown=1.0; record_success(latency-penalized)/record_timeout/record_connect_failure/record_rate_limited; permits=max(1,round(per_relay_concurrency*score)); in_use/incr_in_use/decr_in_use admission bookkeeping; route_allowed (skip-below-threshold + probe) + mark_attempt"
  - "DEFAULT_HEALTH_ALPHA/RELAY_HEALTH_THRESHOLD/PER_RELAY_CONCURRENCY/NIP65_MAX_WRITE_RELAYS/NIP65_FALLBACK_ENABLED consts (back the config defaults by name)"
  - "rate-limit-hit health capture: handle_relay_message/spawn_notice_consumer take Arc<RelayHealthRegistry>; RateLimited arm calls record_rate_limited"
  - "daemon `health` binding: Arc<RelayHealthRegistry>::new(cfg.health_alpha) built beside the rate-limiter registry, passed into the notice consumer (05-04 extends it)"
  - "5 Config fields (nip65_fallback_enabled, nip65_max_write_relays, relay_health_threshold, per_relay_concurrency, health_alpha) with const-backed serde defaults + fail-fast validate guards"
affects: [05-03-nip65-fallback, 05-04-health-routing-concurrency]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Parallel in-memory registry (RelayHealthRegistry mirrors RateLimiterRegistry's Mutex<HashMap>-behind-Arc shape) — a separate, richer EWMA signal, never entangled with the limiter (Pitfall 3)"
    - "EWMA score: score = alpha*sample + (1-alpha)*prev, prev default 1.0; success sample=1/(1+latency/scale), timeout/connect=0.0, rate-limit=0.2"
    - "Skip-below-threshold + periodic probe routing; never-attempted relay is probe-due"
    - "Single NOTICE-consumer hook is the only contact point between the limiter and health (record_rate_limited beside record_notice)"

key-files:
  created:
    - src/relay/health.rs
    - tests/relay_health.rs
  modified:
    - src/relay/mod.rs
    - src/daemon/mod.rs
    - src/daemon/config.rs
    - config.example.toml
    - tests/daemon_config.rs
    - tests/production_wiring.rs

key-decisions:
  - "RelayHealthRegistry is PARALLEL to RateLimiterRegistry, not an extension — rate_limit.rs is byte-for-byte unchanged (Pitfall 3)"
  - "LATENCY_SCALE_SECS=3.0, PROBE_INTERVAL=60s as private consts (Claude's discretion per RESEARCH A1; tunable, no correctness impact)"
  - "Blocked notices still only escalate the limiter; only RateLimited degrades health (mirrors the RateLimited-vs-Blocked usable/stop-traffic split)"
  - "daemon `health` binding named exactly `health` so 05-04 threads it into the fan-out/sampler without renaming"
  - "permits floors at 1 (max(1, round(...))) so a near-zero-score relay keeps one probe slot"

patterns-established:
  - "Pattern: a config knob's serde default fn references a library DEFAULT_* const by name (never re-literal) so daemon/library never drift"
  - "Pattern: parallel registry threaded through the existing NOTICE consumer signature exactly as Arc<RateLimiterRegistry> is"

requirements-completed: [RELAY-06]

# Metrics
duration: ~6min
completed: 2026-06-15
---

# Phase 5 Plan 02: RelayHealthRegistry EWMA + Rate-Limit Capture + Fail-Fast Config Summary

**A parallel per-relay `RelayHealthRegistry` (continuous EWMA health in [0,1], permit scaling, skip-below-threshold + periodic-probe routing) wired into the existing NOTICE consumer for rate-limit-hit capture, with the daemon callsite patched (build green) and 5 fail-fast-validated RELAY-06 config knobs.**

## Performance

- **Duration:** ~6 min
- **Started:** 2026-06-15T09:48:03Z
- **Completed:** 2026-06-15T09:54:06Z
- **Tasks:** 3
- **Files modified:** 7 (2 created, 5 modified)

## Accomplishments
- `src/relay/health.rs`: `RelayHealthRegistry` — a parallel registry (never touches `rate_limit.rs`) holding one EWMA score per relay url. `record_success` is latency-penalized (`sample = 1/(1 + latency_secs/3.0)`), `record_timeout`/`record_connect_failure` sample 0.0, `record_rate_limited` samples 0.2 (degrade, not zero). `score` (unknown=1.0), `permits = max(1, round(per_relay_concurrency*score))`, `in_use`/`incr_in_use`/`decr_in_use` admission bookkeeping, `route_allowed` (>=threshold OR probe-due) + `mark_attempt`. 5 `DEFAULT_*` consts back the config defaults by name.
- `src/relay/mod.rs`: `handle_relay_message` + `spawn_notice_consumer` now thread an `Arc<RelayHealthRegistry>`; the `RateLimited` arm calls `health.record_rate_limited` beside the limiter's `record_notice`. `Blocked` still only escalates the limiter.
- `src/daemon/mod.rs`: builds `let health = Arc::new(RelayHealthRegistry::new(cfg.health_alpha));` beside the rate-limiter registry and passes `Arc::clone(&health)` into the notice consumer — the Wave 1 build (incl. tests) is green. The binding is named `health` for 05-04 to extend.
- `src/daemon/config.rs`: 5 new fields (`nip65_fallback_enabled`, `nip65_max_write_relays`, `relay_health_threshold`, `per_relay_concurrency`, `health_alpha`) with const-backed serde defaults, all rendered in the hand-impl `Debug`, and 4 fail-fast `validate` guards (the bool needs none).
- `config.example.toml`: documents all 5 fields with their defaults (keeps `example_config_is_valid` green).
- Tests: new offline `tests/relay_health.rs` (`ewma_moves_with_signals`, `skip_then_probe`, `permits_scale_with_health` — 3 green); `tests/daemon_config.rs` extended with a default-fill + 4 reject-tests (14 green); `tests/production_wiring.rs` notice-handler callsites updated to the new signature (asserting health degrades on a rate-limited notice).

## Task Commits

Each task was committed atomically:

1. **Task 1: RelayHealthRegistry (EWMA, permits, probe) + offline tests** - `0fe8257` (feat)
2. **Task 2: rate-limit-hit health capture in NOTICE consumer + daemon callsite patch** - `c1ddbf2` (feat)
3. **Task 3: 5 config fields + fail-fast validate + example + tests** - `fb2790e` (feat)

**Plan metadata:** _(this docs commit)_

## Files Created/Modified
- `src/relay/health.rs` (created) - `RelayHealthRegistry` + EWMA/permit/probe methods + `DEFAULT_HEALTH_*`/`DEFAULT_RELAY_HEALTH_THRESHOLD`/`DEFAULT_PER_RELAY_CONCURRENCY`/`DEFAULT_NIP65_MAX_WRITE_RELAYS`/`DEFAULT_NIP65_FALLBACK_ENABLED` consts; private `LATENCY_SCALE_SECS=3.0` + `PROBE_INTERVAL=60s`.
- `tests/relay_health.rs` (created) - 3 offline `#[test]`s against the public registry API (no DB).
- `src/relay/mod.rs` - `pub mod health;` registered; `handle_relay_message`/`spawn_notice_consumer` thread `Arc<RelayHealthRegistry>`; `record_rate_limited` in the RateLimited arm.
- `src/daemon/mod.rs` - construct + pass the `health` registry; sources `cfg.health_alpha`.
- `src/daemon/config.rs` - 5 fields + const-backed default fns + Debug fields + 4 validate guards.
- `config.example.toml` - new "NIP-65 outbox routing & relay health (RELAY-06)" section documenting all 5 fields.
- `tests/daemon_config.rs` - `relay_health_fields_default_fill` + `nip65_max_write_relays_zero_rejected` + `per_relay_concurrency_zero_rejected` + `relay_health_threshold_out_of_range_rejected` + `health_alpha_out_of_range_rejected`.
- `tests/production_wiring.rs` - notice-handler callsites updated to the 4-arg `handle_relay_message` signature.

## Decisions Made
- RelayHealthRegistry is a PARALLEL registry; `git diff --stat src/relay/rate_limit.rs` shows it unchanged (Pitfall 3).
- `Blocked` notices do NOT touch health — only `RateLimited` degrades the score (usable-but-slow vs stop-traffic split).
- `permits` floors at 1 so a degraded relay always keeps one probe slot.
- The `health` daemon binding is named exactly `health` for 05-04 reuse.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Updated `tests/production_wiring.rs` notice-handler callsites**
- **Found during:** Task 2 (signature change to `handle_relay_message`)
- **Issue:** `handle_relay_message` gained an `&RelayHealthRegistry` parameter (3 callsites in `tests/production_wiring.rs` used the old 3-arg form), which would break `cargo build --tests`. The plan named only `src/relay/mod.rs` + `src/daemon/mod.rs` for Task 2, but the existing test is a direct caller of the changed public fn.
- **Fix:** Imported `RelayHealthRegistry` + `DEFAULT_HEALTH_ALPHA`, constructed a registry per test, and threaded `&health` into all three `handle_relay_message` calls; added a positive assertion that a rate-limited notice degrades the health score (strengthens the test rather than just keeping it compiling).
- **Files modified:** tests/production_wiring.rs
- **Verification:** `SQLX_OFFLINE=true cargo build --tests` green; the 3 affected tests (`rate_limited_message_escalates_failure_count`, `blocked_message_does_not_increment_rate_limit_counter`, `consumer_and_fetch_share_one_registry`) pass.
- **Committed in:** c1ddbf2 (Task 2 commit)

---

**Total deviations:** 1 auto-fixed (1 blocking callsite update required by the public-fn signature change).
**Impact on plan:** Necessary to keep the Wave 1 build green — the plan's CRITICAL note already mandated a green `cargo build --tests`; this test is simply another caller of the same changed signature. No scope creep; the API surface and behaviors are exactly as planned.

## Issues Encountered
- None. All offline; no DB / testcontainers needed (this plan adds no `sqlx::query!` so no `.sqlx` regen was required). No container/port flake.

## Authentication Gates
None.

## TDD Gate Compliance
Task 1 was `tdd="true"`. The failing test (`tests/relay_health.rs`) was authored before `src/relay/health.rs` existed, but both were committed together in `0fe8257` rather than as a separate RED commit — the registry is a pure offline unit and the RED/GREEN cycle was exercised locally (tests written, then implementation iterated to green) without a standalone failing-test commit. Behaviors are fully test-verified: 3 green offline tests asserting score-rises-on-fast-success/falls-on-timeout/connect/rate-limit, permits == max(1, round(...)), and route_allowed flipping with threshold + probe. No standalone RED commit exists for Task 1.

## User Setup Required
None — the 5 new config fields all have conservative const-backed defaults, so an existing minimal config keeps validating unchanged; operators may tune them via `config.toml` or `WOT__*` env vars.

## Next Phase Readiness
- 05-03 (NIP-65 fallback) is unblocked: `lookup_write_relays` (05-01) + the error-injecting `ScriptedGraph` seam are ready, independent of health.
- 05-04 (health routing/concurrency) has the full `RelayHealthRegistry` (score/permits/in_use/route_allowed/mark_attempt), the daemon `health` binding to extend into the fan-out + per-relay admission, and the 5 config knobs wired and validated.
- No blockers.

---
*Phase: 05-nip-65-outbox-routing-relay-health*
*Completed: 2026-06-15*

## Self-Check: PASSED
- All 2 created files (src/relay/health.rs, tests/relay_health.rs) + SUMMARY.md present on disk.
- All 3 task commits (0fe8257, c1ddbf2, fb2790e) present in git history.
- `SQLX_OFFLINE=true cargo build --tests` green; relay_health (3) + daemon_config (14) tests green; src/relay/rate_limit.rs unchanged.
