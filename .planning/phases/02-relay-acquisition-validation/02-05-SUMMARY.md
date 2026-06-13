---
phase: 02-relay-acquisition-validation
plan: 05
subsystem: api
tags: [nostr, relay, pagination, fetch-timeout, adversarial-relay, rust]

# Dependency graph
requires:
  - phase: 02-relay-acquisition-validation
    provides: "02-03 paginate_chunk / page_back / fetch_complete_with_timeout, RelayError::FetchTimeout variant, mock_relay ScriptedRelay harness"
  - phase: 02-relay-acquisition-validation
    provides: "02-02 verify::accept ingest gate (the authoritative post-verification dedup boundary)"
provides:
  - "Inclusive boundary page-back (page_back returns oldest, not oldest-1) closing the CR-03 boundary-second hole"
  - "Cross-window HashSet<EventId> dedup + zero-new-id progress stop in paginate_chunk"
  - "MAX_PAGES_PER_CHUNK budget bounding an until-ignoring adversarial relay (CR-04)"
  - "fetch_window_with_deadline: elapsed-time check constructing RelayError::FetchTimeout on the SDK's partial-Ok timeout (CR-02)"
  - "Removal of pre-verify dedup_by_id from the fetch path (CR-01 fetch half / id-squat protection)"
affects: [phase-03-orchestration-persistence, phase-04-observability]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Inclusive boundary page-back + id-dedup + zero-new-id stop (completeness without boundary holes)"
    - "Per-window elapsed-deadline wrapper (fetch_window_with_deadline) decoupled from the SDK's silent partial-Ok timeout"
    - "Dedup strictly AFTER verification (verify::accept), never before, to defeat id-squat suppression"

key-files:
  created:
    - tests/fetch_timeout.rs
  modified:
    - src/relay/fetch.rs
    - tests/pagination.rs

key-decisions:
  - "MAX_PAGES_PER_CHUNK = 10_000: worst-case out = 10_000 * cap(<=500) ~ 5M events before erroring, far above any honest follow-list history, so it never truncates legitimate pagination yet bounds a hostile relay."
  - "Reused RelayError::FetchTimeout(String) for the page-budget overflow rather than adding a new variant — the requeue semantics are identical (a chunk that cannot complete) and the plan forbade an unjustified new variant."
  - "FetchTimeout relay label derived from the connected pool's relay urls (pool_label), never embedding secrets (T-02-01)."

patterns-established:
  - "Page-back is INCLUSIVE; cross-window dedup + zero-new-id progress is what stops the loop, not exclusive arithmetic."
  - "Timeout is detected by wall-clock elapsed >= deadline, because nostr-relay-pool 0.44.1 returns a partial Ok (no error) on timeout."
  - "fetch.rs never collapses two events sharing a claimed id before verification."

requirements-completed: [RELAY-03]

# Metrics
duration: 8min
completed: 2026-06-13
---

# Phase 2 Plan 05: Fetch Completeness & Adversarial-Relay Safety Summary

**Inclusive boundary page-back + cross-window id-dedup + MAX_PAGES_PER_CHUNK budget + elapsed-time FetchTimeout detection, with pre-verify dedup removed so an id-squat cannot suppress the genuine event.**

## Performance

- **Duration:** 8 min
- **Started:** 2026-06-13T06:36:33Z
- **Completed:** 2026-06-13T06:44:17Z
- **Tasks:** 2 (TDD)
- **Files modified:** 3 (1 created, 2 modified)

## Accomplishments
- **CR-03 (T-02-11) closed:** `page_back` now returns `oldest` (inclusive) on a capped window, so an event sharing the oldest second that the relay's cap cut off is re-requested rather than permanently lost at the boundary second.
- **CR-04 (T-02-16) closed:** `paginate_chunk` carries a cross-window `HashSet<EventId>`, stops when a window yields zero new ids, and hard-errors at `MAX_PAGES_PER_CHUNK` — an until-ignoring relay can no longer drive an unbounded loop or unbounded `out` growth.
- **CR-02 (T-02-12) closed:** new `fetch_window_with_deadline` records `Instant::now()` and returns `RelayError::FetchTimeout(relay_url)` when `elapsed >= timeout`, converting the SDK's silent partial-`Ok` timeout into a requeue signal instead of a falsely-complete window.
- **CR-01 fetch half (T-02-17) closed:** pre-verify `dedup_by_id` removed from the fetch path; cross-source dedup now happens only after `verify::accept`, so a forged id-squat copy cannot suppress the genuine event before its signature is checked.

## Task Commits

Each task was committed atomically (TDD: test → feat):

1. **Task 1 (RED): failing tests for inclusive boundary / dedup / zero-new-id / budget** - `a0a9d24` (test)
2. **Task 1 (GREEN): inclusive page-back + page budget + new-id progress guard** - `c924930` (feat)
3. **Task 2 (RED): failing test for elapsed-timeout FetchTimeout requeue** - `5313c2a` (test)
4. **Task 2 (GREEN): FetchTimeout on elapsed timeout + drop pre-verify dedup** - `5f85c7c` (feat)

**Plan metadata:** (see final docs commit)

## Files Created/Modified
- `src/relay/fetch.rs` - Inclusive `page_back`; reworked `paginate_chunk` (cross-window seen-set, new-id progress, `MAX_PAGES_PER_CHUNK` budget); new `fetch_window_with_deadline` elapsed-check wrapper + `pool_label`; removed `dedup_by_id`; `fetch_complete_with_timeout` now returns the raw un-deduped union.
- `tests/fetch_timeout.rs` - New: `timed_out_window_requeues` (elapsed >= timeout -> FetchTimeout) and `fast_window_returns_events`.
- `tests/pagination.rs` - Updated `page_back_pages_on_capped_window_only` and `capped_first_window_triggers_second_page` to inclusive expectations; added `inclusive_boundary_keeps_boundary_event`, `cross_window_dedup_keeps_each_event_once`, `zero_new_id_window_stops_even_when_capped`, `budget_guard_errors_on_adversarial_relay`.

## Decisions Made
- `MAX_PAGES_PER_CHUNK = 10_000` — generously sized so the product with the relay cap (~5M events) far exceeds any honest follow-list history, never truncating legitimate pagination while still bounding a hostile relay to a finite, recoverable failure.
- Reused `RelayError::FetchTimeout(String)` for the page-budget overflow (carrying a "page budget exceeded" message) instead of adding a new error variant — requeue semantics are identical and the plan explicitly forbade an unjustified new variant.
- `FetchTimeout` relay label derived from the connected pool's relay urls via `pool_label`; contains no secrets (T-02-01).

## Deviations from Plan

None - plan executed exactly as written. (Two pre-existing tests in `tests/pagination.rs` that asserted the old exclusive `oldest - 1` behavior were updated to the new inclusive expectation; the plan's Task 1 action explicitly calls for updating the existing `page_back_pages_on_capped_window_only` test, and the second update is the same behavioral change.)

## Issues Encountered
- `cargo clippy --fix` churned through several closure-style lint iterations (`redundant_closure` → `let_and_return` → unnecessary `mut`) on the pagination test closures; converged to passing `fetch` directly to `paginate_chunk` with `let fetch = relay.fetch_fn();` (no `mut`). The pagination test now produces 0 clippy warnings.
- One pre-existing `redundant_closure` warning remains in `tests/concurrency.rs:45` (introduced in 01-03, untouched here). Logged to `deferred-items.md` per the scope boundary; not fixed.

## User Setup Required
None - no external service configuration required.

## Verification
- `cargo test --test pagination --test fetch_timeout` exits 0 (7 + 2 tests pass); full suite green.
- `grep -rn "FetchTimeout(" src/` returns 2 construction sites in `src/relay/fetch.rs` (page budget + elapsed timeout).
- `grep -n "saturating_sub(1)" src/relay/fetch.rs` returns nothing (exclusive boundary gone).
- `grep -n "dedup_by_id" src/relay/fetch.rs` returns nothing (pre-verify dedup removed).
- `cargo clippy --all-targets` produces no new warnings for `fetch.rs` (only the pre-existing, out-of-scope `concurrency.rs` warning remains).

## Threat Surface
No new security-relevant surface introduced beyond the plan's `<threat_model>`. All four registered threats (T-02-11, T-02-16, T-02-12, T-02-17) are mitigated as planned.

## Next Phase Readiness
- RELAY-03 completeness and adversarial-relay safety restored; Phase 02 acquisition half now emits complete windows and requeues (never silently drops) timed-out or hostile windows.
- Ready for Phase 03 orchestration/persistence (connect_curated -> acquire_validated_lists_client -> upsert_pubkey -> apply_follow_list).

## Self-Check: PASSED

All claimed files exist (`src/relay/fetch.rs`, `tests/fetch_timeout.rs`, `tests/pagination.rs`, `02-05-SUMMARY.md`) and all task commits are in git history (`a0a9d24`, `c924930`, `5313c2a`, `5f85c7c`).

---
*Phase: 02-relay-acquisition-validation*
*Completed: 2026-06-13*
