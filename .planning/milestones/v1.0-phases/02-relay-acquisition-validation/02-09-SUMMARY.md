---
phase: 02-relay-acquisition-validation
plan: 09
subsystem: relay
tags: [nostr-sdk, governor, rate-limiting, nip11, notifications, wiring]

# Dependency graph
requires:
  - phase: 02-relay-acquisition-validation (02-05)
    provides: fetch_complete / fetch_complete_with_timeout pagination loop (RELAY-03)
  - phase: 02-relay-acquisition-validation (02-07)
    provides: LimitCache::get_or_fetch clamped NIP-11 max_limit (RELAY-02)
  - phase: 02-relay-acquisition-validation (02-08)
    provides: RateLimiterRegistry::acquire shared GCRA limiter + record_notice/backoff (RELAY-04)
provides:
  - Production acquire path gates every fetch_events behind RateLimiterRegistry::acquire (paginate_chunk_gated)
  - acquire_validated_lists_client sources per-window cap from LimitCache, not a caller argument
  - spawn_notice_consumer drains client.notifications() and routes NOTICE/CLOSED into record_notice
  - handle_relay_message: socket-free testable per-message notice router
affects: [crawler-loop, ops-config, phase-03, phase-04-observability]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Gated pagination seam: paginate_chunk_gated wraps the injected fetch fn so registry.acquire runs before each window REQ, keeping count-vs-cap logic untouched"
    - "Notice consumer split: pure handle_relay_message (testable offline) + spawn_notice_consumer (live notification stream), never hand-rolling relay-message parsing (uses typed RelayMessage)"

key-files:
  created:
    - tests/production_wiring.rs
  modified:
    - src/relay/fetch.rs
    - src/relay/mod.rs
    - tests/mock_relay/mod.rs

key-decisions:
  - "handle_relay_message does NOT sleep the backoff in the shared consumer task — sleeping would stall every relay's notices; it only escalates the per-relay failure count that the fetch gate consults"
  - "acquire_validated_lists_client signature changed: dropped the caller max_limit arg, added relay_url + &RateLimiterRegistry + &LimitCache; cap is now sourced from the cache (authoritative per RELAY-02/T-02-13)"
  - "paginate_chunk_gated constructs the inner fetch future before awaiting acquire(); production/mock fetch futures are lazy so the REQ work still happens after the gate"

patterns-established:
  - "Production-wiring test pattern: drive the real production seam (paginate_chunk_gated) against the injected scripted relay with an injected registry/cache and assert observable throttling + cache-sourced filter limit, proving the mechanism is REACHED, not only unit-tested"

requirements-completed: [RELAY-02, RELAY-04]

# Metrics
duration: 22min
completed: 2026-06-13
---

# Phase 02 Plan 09: Production Wiring of Rate Limiter, NIP-11 Cap, and Notice Backoff (WR-03) Summary

**Wired the tested-but-disconnected RELAY-02/RELAY-04 mechanisms into the production acquire path: every window REQ now passes RateLimiterRegistry::acquire, max_limit is sourced from LimitCache, and a spawned notifications consumer routes rate-limited/blocked notices into record_notice/backoff.**

## Performance

- **Duration:** 22 min
- **Started:** 2026-06-13T15:00:00Z
- **Completed:** 2026-06-13T15:22:00Z
- **Tasks:** 2 (TDD)
- **Files modified:** 4 (1 created, 3 modified)

## Accomplishments
- Closed WR-03 (BLOCKER): the 02-VERIFICATION data-flow DISCONNECTED rows for `acquire()`, `LimitCache::get_or_fetch()`, and `record_notice()` are now wired — all three have production callers reachable from `acquire_validated_lists_client` / `fetch_complete`.
- Task 1: added `paginate_chunk_gated` (awaits `registry.acquire(relay_url)` before each window REQ); threaded `&RateLimiterRegistry` through `fetch_complete`/`fetch_complete_with_timeout`; `acquire_validated_lists_client` now sources the per-window cap from `limit_cache.get_or_fetch(relay_url).max_limit` and resets backoff on success (T-02-10 / T-02-13).
- Task 2: added `handle_relay_message` (pure, socket-free notice router) and `spawn_notice_consumer` (drains `client.notifications()`, matches typed `RelayMessage::Notice`/`Closed`, feeds the shared registry) so a rate-limited notice escalates the same per-relay counter the fetch gate consults (T-02-09).

## Task Commits

TDD: one RED test commit covering both tasks (shared test file), then GREEN per task.

1. **RED (Tasks 1+2): failing production-wiring tests** - `5bd4ea3` (test)
2. **Task 1: gate every production window REQ behind acquire()** - `4e89434` (feat)
3. **Tasks 1+2: source max_limit from LimitCache + spawn notice consumer** - `875e8a5` (feat)

_Task 1's cap-sourcing edit and Task 2's consumer both live in `src/relay/mod.rs`; they were committed together in `875e8a5` because a single file cannot be cleanly split across two commits without interactive hunk staging. The fetch.rs gate (Task 1) is isolated in `4e89434`._

## Files Created/Modified
- `src/relay/fetch.rs` - Added `paginate_chunk_gated`; `fetch_complete`/`fetch_complete_with_timeout` now take `&RateLimiterRegistry` and gate every window REQ.
- `src/relay/mod.rs` - `acquire_validated_lists_client` sources cap from `LimitCache`, threads the registry, resets backoff on success; added `handle_relay_message` + `spawn_notice_consumer`.
- `tests/production_wiring.rs` - 5 tests proving the gate throttles each window, the cap is the cached `max_limit`, and the notice handler escalates `failure_count` (rate-limited) / does not (blocked).
- `tests/mock_relay/mod.rs` - `ScriptedRelay` now records per-REQ filter `limit` (`limit_capturing_fetch_fn` / `limits_seen`) so a test can assert the cache-sourced cap.

## Decisions Made
- The shared notice consumer escalates the failure count but does NOT sleep the backoff inline — sleeping a single shared task would stall every relay's notice processing. The per-relay failure count is the state the fetch path's backoff schedule consults.
- `acquire_validated_lists_client` signature changed (dropped caller `max_limit`, added `relay_url`, `&RateLimiterRegistry`, `&LimitCache`). The cache value is authoritative for the cap (RELAY-02/T-02-13); no production callers existed outside this plan, so the change is contained.
- `paginate_chunk_gated` delegates to `paginate_chunk` (page-back logic untouched) and only wraps the fetch fn with the acquire gate, keeping the count-vs-cap completeness contract intact.

## Deviations from Plan

None - plan executed exactly as written. Both tasks' mechanisms were implemented and verified; the only structural note is that the two tasks' `src/relay/mod.rs` edits share one commit (see Task Commits note above), which is a commit-granularity detail, not a scope or behavior deviation.

## Issues Encountered
- Initial `paginate_chunk_gated` nested an `async` block that borrowed the `FnMut` `fetch` across awaits, which the borrow checker rejects ("captured variable cannot escape FnMut closure body"). Resolved by constructing the (lazy) inner fetch future before the `async move` block and awaiting `acquire()` first — the gate still runs before the REQ work because the production/mock fetch futures do nothing until awaited.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness
- WR-03 closed: the rate limiter, NIP-11 cap, and notice backoff now actually govern real REQs in production.
- The crawler driver (a later phase) must call `spawn_notice_consumer(client.clone(), Arc::clone(&registry))` once at acquisition start and pass the same `Arc<RateLimiterRegistry>` + `LimitCache` into `acquire_validated_lists_client` per relay so the consumer and fetch gate share state.
- `relay_url` is now a required per-relay key on the production entry point; config sourcing of the curated set (OPS-01) remains a later phase.

## TDD Gate Compliance
RED (`5bd4ea3`, test) precedes GREEN (`4e89434` + `875e8a5`, feat). Verified the tests failed to compile before implementation (missing `paginate_chunk_gated` / `handle_relay_message`) and pass after.

## Self-Check: PASSED

All claimed files exist on disk (tests/production_wiring.rs, src/relay/fetch.rs, src/relay/mod.rs, tests/mock_relay/mod.rs, 02-09-SUMMARY.md) and all three task commits (5bd4ea3, 4e89434, 875e8a5) are present in git history.

---
*Phase: 02-relay-acquisition-validation*
*Completed: 2026-06-13*
