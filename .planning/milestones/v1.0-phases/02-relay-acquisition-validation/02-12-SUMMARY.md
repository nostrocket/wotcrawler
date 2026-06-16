---
phase: 02-relay-acquisition-validation
plan: 12
subsystem: relay-acquisition
tags: [relay, pagination, completeness, gap-closure, RELAY-03, TDD]
requires:
  - "src/relay/fetch.rs paginate_chunk / page_back (02-03, 02-10)"
  - "tests/mock_relay prefix_for_until_fetch_fn deterministic-relay harness (02-10)"
  - "RelayError::FetchTimeout requeue signal (02-03)"
provides:
  - "First-visit boundary-second stall detection in paginate_chunk (page_back re-pin check)"
  - "no_newer_event_boundary_stall_surfaces_error regression test"
affects:
  - "downstream spam-scoring layer (follow-list completeness invariant at cap boundary)"
tech-stack:
  added: []
  patterns:
    - "Stall detection via page_back(returned, cap, oldest) == Some(current_until) — page-back would re-pin same until"
    - "OR-combine new first-visit detector with retained prev_until 2-visit guard (superset)"
key-files:
  created: []
  modified:
    - "src/relay/fetch.rs"
    - "tests/pagination.rs"
decisions:
  - "Reconciled zero_new_id_window_stops_even_when_capped: its capped re-served prefix at a pinned boundary IS the stall; renamed to capped_reserved_prefix_at_pinned_boundary_surfaces_error and changed expectation from Ok([A,B]) to Err"
  - "Did not add a new RelayError variant — reused FetchTimeout(String) as the requeue signal per plan"
metrics:
  duration: "3m"
  completed: "2026-06-13T11:08:21Z"
  tasks: 2
  files: 2
---

# Phase 2 Plan 12: No-Newer-Event Boundary Stall Closure Summary

Closed BLOCKER gap CR-01-new (RELAY-03): `paginate_chunk` now detects a deterministic newest-first relay's boundary-second stall on the FIRST capped zero-new-id re-request — including the no-newer-event case where every pool event shares the boundary second — by checking that `page_back(returned, cap, oldest)` would re-pin the same `until`, surfacing a requeue `Err` instead of a silently truncated `Ok`.

## What Was Built

### Task 1 (RED) — commit `c39a619`
Added `no_newer_event_boundary_stall_surfaces_error` to `tests/pagination.rs`, a companion to `deterministic_boundary_stall_surfaces_error`. The sole structural difference: the pool is `[A(T), B(T), C(T)]` with NO event newer than the boundary second `T` (no `N(T+1)`). It drives `prefix_for_until_fetch_fn` with `cap=2` and asserts the result is `Err`, not a silent truncated `Ok([A, B])`, plus that `until=T` was re-requested at least once. Confirmed FAILING before the fix (returned `Ok([A(T), B(T)])`, dropping `C(T)`).

### Task 2 (GREEN) — commit `7a98314`
Strengthened the zero-new-id stall branch in `paginate_chunk` (`src/relay/fetch.rs`). The stall condition became:

`returned >= cap && (prev_until == Some(current_until) || page_back(returned, cap, oldest) == Some(current_until))`

The new `page_back(...) == Some(current_until)` disjunct fires on the first capped zero-new-id re-request of a pinned boundary second, independent of `prev_until`. This catches the no-newer-event path where `until` becomes `T` on the first page-back and the relay re-serves the same cap-sized prefix while siblings at `T` remain. The existing `prev_until` 2-visit guard is retained (OR-combined) as the superset case. Genuine exhaustion is preserved: a short window (`returned < cap`) makes `page_back` return `None`, and a window whose oldest is at an older second makes `page_back` return `Some(older) != current_until` — both break `Ok`. The loop doc comment was updated to describe the first-visit detection and reference CR-01-new.

## Deviations from Plan

### Test reconciliation (explicit per Task 2 action)
`zero_new_id_window_stops_even_when_capped` was renamed to `capped_reserved_prefix_at_pinned_boundary_surfaces_error` and its expectation changed from `Ok` (len==2) to `Err(RelayError::FetchTimeout(_))`. Per its literal data — window2 is `== cap`, all ids already seen, oldest (4000) equals the pinned `until` — its old `Ok` expectation was itself an instance of the silent-truncation bug CR-01-new. Chose option (a) from the plan action (confirm it is the stall and expect Err) rather than shortening window2, matching the invariant "a capped re-served-prefix at a pinned boundary is a stall, never silent completion." Documented in the test doc comment.

### Auto-fixed Issues

**1. [Rule 1 - Lint] Used `result.is_err()` instead of `matches!(result, Err(_))` in the new test**
- **Found during:** Task 2 (clippy check)
- **Issue:** clippy flagged `matches!(result, Err(_))` in the new `no_newer_event_boundary_stall_surfaces_error` as a redundant pattern match.
- **Fix:** Replaced with `result.is_err()` in the test I authored.
- **Files modified:** tests/pagination.rs
- **Commit:** 7a98314

The one remaining clippy warning at `tests/pagination.rs:217` (`deterministic_boundary_stall_surfaces_error`) is pre-existing and unmodified by this plan — out of scope.

## Authentication Gates
None.

## Verification Results
- `cargo test --test pagination`: 9 passed, 0 failed (includes the new test, the retained `deterministic_boundary_stall_surfaces_error`, and the reconciled `capped_reserved_prefix_at_pinned_boundary_surfaces_error`).
- `grep -n "page_back(returned, cap, oldest)" src/relay/fetch.rs`: matches at line 164 (the new first-visit detector) plus the doc comment and the existing advance match.
- `cargo test --tests`: all suites green (pagination, reconnect_policy, relay_list, replaceable, verify_gate, and others) — no regressions.
- `cargo build`: succeeds, no dead-code or unused warnings.

## Known Stubs
None.

## Self-Check: PASSED
- FOUND: .planning/phases/02-relay-acquisition-validation/02-12-SUMMARY.md
- FOUND: src/relay/fetch.rs
- FOUND: tests/pagination.rs
- FOUND: commit c39a619 (RED)
- FOUND: commit 7a98314 (GREEN)
