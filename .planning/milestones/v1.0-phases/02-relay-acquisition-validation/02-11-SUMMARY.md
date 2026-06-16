---
phase: 02-relay-acquisition-validation
plan: 11
subsystem: api
tags: [nostr, rate-limiting, governor, gcra, relay-pool, fetch]

# Dependency graph
requires:
  - phase: 02-relay-acquisition-validation
    provides: "02-09 paginate_chunk_gated + RateLimiterRegistry::acquire production gating; 02-10 boundary-stall requeue; 02-08 shared Arc<DirectLimiter> per-relay quota"
provides:
  - "fetch_complete / fetch_complete_with_timeout take an explicit per-relay relay_url used as the GCRA limiter key and FetchTimeout label"
  - "pool_label demoted to diagnostics-only (folded into the FetchTimeout message, never the acquire() key)"
  - "RateLimiterRegistry::active_relay_count / has_limiter introspection for per-relay-keying assertions"
  - "Regression test proving two pooled relays mint two independent limiter keys, not one joined-pool-string key"
affects: [03-persistence, 04-observability, 05-relay-health]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Per-relay GCRA limiter keyed strictly on the caller's individual relay_url, threaded explicitly through the fetch path (no internal re-derivation from pool state)"
    - "Pool-level diagnostics enrich the requeue label but never the limiter identity"

key-files:
  created: []
  modified:
    - src/relay/fetch.rs
    - src/relay/mod.rs
    - src/relay/rate_limit.rs
    - tests/production_wiring.rs

key-decisions:
  - "Folded pool_label into the FetchTimeout message text rather than adding a tracing dependency — tracing is not yet wired into the crate and Rule 3 excludes speculative dependency additions; the plan explicitly offered this alternative."

patterns-established:
  - "Limiter key = individual relay_url threaded from the caller; pool diagnostics are message-only context"

requirements-completed: [RELAY-04]

# Metrics
duration: 6min
completed: 2026-06-13
---

# Phase 02 Plan 11: Per-Relay Limiter Keying (WR-03 Residual) Summary

**Threaded the caller's individual relay_url through fetch_complete / fetch_complete_with_timeout so each relay drives its own GCRA limiter, demoting the joined pool_label string to diagnostics-only — closing BLOCKER 2 (WR-03 residual / RELAY-04).**

## Performance

- **Duration:** 6 min
- **Started:** 2026-06-13T07:49:00Z
- **Completed:** 2026-06-13T07:55:50Z
- **Tasks:** 2
- **Files modified:** 4

## Accomplishments
- Added `relay_url: &str` parameter to `fetch_complete` and `fetch_complete_with_timeout`, threaded from `acquire_validated_lists_client`'s existing per-relay url, and used it as BOTH the `paginate_chunk_gated` limiter key and the `FetchTimeout` label.
- Demoted `pool_label` from limiter/timeout key to diagnostics-only — its joined connected-pool string is now folded into the `FetchTimeout` message for operator context but never passed to `registry.acquire()`.
- Added `RateLimiterRegistry::active_relay_count()` and `has_limiter()` introspection so tests can assert per-relay keying without reaching private fields.
- Added `two_pooled_relays_get_independent_limiter_keys` regression test proving two pooled relays yield two independent limiter keys (and that the joined-pool-string key is absent).

## Task Commits

Each task was committed atomically:

1. **Task 1: RED test + registry introspection** - `d6b5afa` (test)
2. **Task 2: Thread per-relay relay_url, demote pool_label** - `9caf770` (feat / GREEN)

**Plan metadata:** (final docs commit)

_Note: Task 1's test and the introspection it asserts against are compile-coupled, so they committed together; the test was load-bearing at the `paginate_chunk_gated` seam from the start and became end-to-end load-bearing after Task 2 threaded the url through the production path._

## Files Created/Modified
- `src/relay/fetch.rs` - Added `relay_url` param to `fetch_complete`/`fetch_complete_with_timeout`; used it as the limiter key and FetchTimeout label; demoted `pool_label` to a diagnostics string folded into the timeout message.
- `src/relay/mod.rs` - `acquire_validated_lists_client` now threads its individual `relay_url` into the `fetch_complete` call.
- `src/relay/rate_limit.rs` - Added `active_relay_count` and `has_limiter` introspection methods.
- `tests/production_wiring.rs` - Added the two-relay independent-keys regression test.

## Decisions Made
- **pool_label diagnostics via FetchTimeout message, not tracing:** `tracing` is listed as a planned dependency in CLAUDE.md but is not yet a Cargo dependency. Adding it solely to host one debug log would be a speculative dependency install (excluded from Rule 3). The plan explicitly offered folding the pool list into the FetchTimeout message as an alternative, which keeps `pool_label` used (no dead-code warning) and preserves both the per-relay key and operator pool context.

## Deviations from Plan

None - plan executed exactly as written. (The `tracing` vs FetchTimeout-message choice was a plan-sanctioned alternative, not a deviation.)

## Issues Encountered
- Initial Task 2 draft used `tracing::debug!` for pool diagnostics; the build failed because `tracing` is not a crate dependency. Resolved by folding the pool context into the `FetchTimeout` label string (a plan-listed alternative) instead of adding a dependency. No scope change.
- Pre-existing clippy warnings in unrelated test files (`tests/nip11_limits.rs`, `tests/pagination.rs`, `tests/concurrency.rs`) are out of scope for this plan and logged to `deferred-items.md`.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Per-relay rate limiting is now real end-to-end in production: the limiter key, NIP-11 cache key, and backoff reset key are the SAME individual relay url. Observable truth #9 (per-relay limiter keyed per-relay, not pool-wide) is restored.
- BLOCKER 2 / WR-03 residual closed; RELAY-04 politeness contract honored across pool-membership changes (GCRA state no longer reset on churn).
- No blockers introduced.

## Known Stubs
None.

## Self-Check: PASSED

All modified files and task commits verified present (src/relay/fetch.rs, src/relay/mod.rs, src/relay/rate_limit.rs, tests/production_wiring.rs, 02-11-SUMMARY.md; commits d6b5afa, 9caf770).

---
*Phase: 02-relay-acquisition-validation*
*Completed: 2026-06-13*
