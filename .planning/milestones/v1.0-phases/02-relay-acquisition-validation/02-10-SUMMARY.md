---
phase: 02-relay-acquisition-validation
plan: 10
subsystem: api
tags: [nostr, relay, pagination, completeness, rust]

# Dependency graph
requires:
  - phase: 02-relay-acquisition-validation
    provides: paginate_chunk inclusive page-back (CR-03) and MAX_PAGES_PER_CHUNK budget (CR-04) from plan 02-05
provides:
  - "prev_until boundary-second stall detection in paginate_chunk: a deterministic relay re-serving the same cap-sized prefix for a pinned until=T surfaces a requeue Err instead of a silent truncated Ok (CR-03 residual / RELAY-03 / T-02-15)"
  - "Deterministic-relay test harness prefix_for_until_fetch_fn modeling same-prefix-per-until cap-boundary behavior (not a hand-fed cut sibling)"
affects: [relay-acquisition, fetch completeness, follow-graph integrity]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Boundary-stall detection: distinguish genuine exhaustion from a pinned-until cap stall using a prev_until cross-iteration tracker (returned>=cap AND prev_until==Some(until) AND new_ids==0 => requeue Err)"
    - "Deterministic-relay mock: a fixed sorted-newest-first event pool clamped per-filter-until returns the same cap-sized prefix for a repeated until, modeling real relay behavior the pop-front ScriptedRelay cannot"

key-files:
  created: []
  modified:
    - src/relay/fetch.rs
    - tests/pagination.rs
    - tests/mock_relay/mod.rs

key-decisions:
  - "Reused RelayError::FetchTimeout for the boundary stall (existing requeue semantics already match) rather than adding a dedicated BoundaryStall variant — no other error variant touched."
  - "prev_until is set AFTER the page_back decision (end of iteration), so the stall comparison reflects THIS fetch's until vs the PRIOR fetch's until; the first page-back into a boundary second is genuine exhaustion (Ok), only a REPEATED pinned until=T is the stall (Err)."

patterns-established:
  - "prev_until stall tracker: track the previous iteration's until to tell a fresh page-back from a re-request of the same pinned boundary second"
  - "Deterministic-prefix mock helper for completeness tests that must model real relay behavior, not scripted hand-feeding"

requirements-completed: [RELAY-03]

# Metrics
duration: 5min
completed: 2026-06-13
---

# Phase 02 Plan 10: CR-03 Boundary-Second Stall Closure Summary

**paginate_chunk now distinguishes a genuinely exhausted boundary second from a deterministic relay stalled at a pinned until=T (more events remaining) and surfaces the latter as a requeue Err, closing the silent-truncation hole in RELAY-03 completeness.**

## Performance

- **Duration:** 5 min
- **Started:** 2026-06-13T07:44:17Z
- **Completed:** 2026-06-13T07:49:11Z
- **Tasks:** 2
- **Files modified:** 3

## Accomplishments
- Added a `prev_until: Option<Timestamp>` cross-iteration tracker to `paginate_chunk` and a stall-detection branch: when a window is still capped (`returned >= cap`), `until` did not advance (`prev_until == Some(current_until)`), and the re-request yielded nothing new (`new_ids == 0`), the loop returns `Err(RelayError::FetchTimeout(...))` so the caller requeues — it never silently completes a truncated follow list.
- Preserved genuine-exhaustion termination: a short window, or the FIRST page-back into a boundary second (until just advanced), still breaks with `Ok` — no false-positive stall errors.
- Added `prefix_for_until_fetch_fn` to the mock harness: a deterministic newest-first relay over a fixed event pool that returns the SAME cap-sized prefix for a repeated `until=T` and NEVER volunteers the cut sibling — modeling the exact real-relay behavior the verification flagged as untested.
- Added `deterministic_boundary_stall_surfaces_error`: a TDD RED→GREEN test that FAILED against the old silent-completion behavior (returned 3-event Ok omitting C(T)) and PASSES after the fix (Err surfaced).

## Task Commits

Each task was committed atomically (TDD RED → GREEN):

1. **Task 1: deterministic-relay stall test (RED)** - `1ac92a9` (test)
2. **Task 2: prev_until stall detection in paginate_chunk (GREEN)** - `9f7633d` (feat)

_No REFACTOR commit needed — implementation was clean on first pass._

## Files Created/Modified
- `src/relay/fetch.rs` - Added `prev_until` tracker and the boundary-stall branch in `paginate_chunk`; updated the zero-new-id guard's doc comment to describe stall vs exhaustion. `page_back`, the MAX_PAGES_PER_CHUNK budget, and `fetch_complete`/`fetch_complete_with_timeout`/`pool_label` (02-11's concern) untouched.
- `tests/pagination.rs` - Added `deterministic_boundary_stall_surfaces_error` and the `std::cell::RefCell`/`std::rc::Rc` imports plus `prefix_for_until_fetch_fn` import.
- `tests/mock_relay/mod.rs` - Added `UntilLog` type alias and `prefix_for_until_fetch_fn` deterministic-prefix helper.

## Decisions Made
- Reused `RelayError::FetchTimeout` for the boundary stall rather than adding a dedicated `BoundaryStall` variant — the plan permitted either, and FetchTimeout's existing requeue semantics already match. No other error variant was modified.
- Placed `prev_until = Some(current_until)` after the `page_back` decision (end of iteration). The terminating arms (`new_ids == 0 break`, `page_back None => break`) exit the loop, so the placement is correct: the stall comparison always reflects THIS fetch's until vs the PRIOR fetch's.

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered
None. The RED test failed exactly as predicted (silent truncated Ok omitting C(T)); the GREEN fix made it pass with all 7 pre-existing pagination tests staying green.

## Known Stubs
None - both changes are complete production logic and a complete test.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- BLOCKER 1 (CR-03 residual / RELAY-03, T-02-15) closed: capped-window completeness with the stall surfaced as an error (observable truth #10) is restored.
- BLOCKER 2 (WR-03 per-relay limiter key) remains open for plan 02-11; `fetch_complete`/`pool_label` were deliberately left untouched here as 02-11's concern.
- Full test suite green: `cargo test --tests` passes all suites (pagination 8/8); `cargo build` has no warnings.

## Self-Check: PASSED

All modified files present on disk; both task commits (`1ac92a9`, `9f7633d`) found in git history.

---
*Phase: 02-relay-acquisition-validation*
*Completed: 2026-06-13*
