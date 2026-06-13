---
phase: 02-relay-acquisition-validation
reviewed: 2026-06-13T09:00:00Z
depth: standard
files_reviewed: 2
files_reviewed_list:
  - src/relay/fetch.rs
  - tests/pagination.rs
findings:
  critical: 0
  warning: 3
  info: 2
  total: 5
status: issues_found
---

# Phase 02 (gap-closure 02-12): Code Review Report

**Reviewed:** 2026-06-13T09:00:00Z
**Depth:** standard
**Files Reviewed:** 2
**Status:** issues_found

## Summary

This is a gap-closure review of plan 02-12 (RELAY-03 no-newer-event boundary
stall). The prior review (preserved in git history) raised CR-01: the
`prev_until` 2-visit guard missed the no-newer-event case and silently dropped
boundary-second siblings. Plan 02-12 closes it by OR-combining a first-visit
detector at `paginate_chunk` lines 161-171:

```rust
if returned >= cap
    && (prev_until == Some(current_until)
        || page_back(returned, cap, oldest) == Some(current_until))
```

**Disposition of prior CR-01:** **Closed.** I traced every test in
`tests/pagination.rs` against the implementation. The new
`page_back(...) == Some(current_until)` disjunct fires on the FIRST capped
zero-new-id re-request of a pinned boundary second, exactly the no-newer-event
path. `no_newer_event_boundary_stall_surfaces_error` (line 235) now asserts
`Err`, and the implemented detector matches the fix the prior review proposed.
Short windows (`returned < cap`) and genuinely-advancing windows
(`oldest < current_until`, so page_back returns a different timestamp) still
break with `Ok`, so legitimate exhaustion is preserved in those cases.

The change is correct in its fail-safe direction (it never converts a stall
into silent truncation — worst case is a recoverable requeue). No BLOCKERs.
However the change carries a real false-positive cost against honest relays,
the retained `prev_until` disjunct is now dead code, and a symmetric
zero-new-id blind spot remains unaddressed.

## Narrative Findings (AI reviewer)

## Warnings

### WR-01: Honest relays with exactly-cap siblings at the boundary second are now misclassified as a stall

**File:** `src/relay/fetch.rs:161-171`

**Issue:** The detector fires whenever `new_ids == 0 && returned >= cap &&
page_back(returned, cap, oldest) == Some(current_until)`. That condition is ALSO
true for a perfectly honest, fully-exhausted relay whose oldest second contains
exactly `cap` events with no cut sibling.

Trace (cap=2), pool = `[A(T), B(T)]` — only two events at second T, nothing
older, nothing cut:

- Window 1 (`until=now`): `[A(T), B(T)]` → capped, `oldest=T`, page_back →
  `Some(T)`, so `until=T`.
- Window 2 (`until=T`): relay re-serves `[A(T), B(T)]` (which is genuinely
  everything ≤ T) → `new_ids=0`, `returned=2 >= cap`, `oldest=T == current_until`
  → `page_back == Some(T)` → **Err**.

The relay was complete and correct, yet the chunk is requeued as a stall. The
inline comment (lines 134-160) acknowledges the ambiguity is "unresolvable" and
chooses requeue over silent truncation — a defensible safety bias the prior
review endorsed — but the *common* real case is honest relays whose newest
follow-list events cluster in the same second (kind-3 replacement churn), so
this generates avoidable re-fetch load against well-behaved relays, in tension
with the "each list fetched roughly once" / relay-goodwill constraint in
CLAUDE.md. There is no test covering the honest exactly-cap-and-complete relay,
so this accepted false-positive is undocumented in the test suite and could be
"fixed" back into truncation by a future maintainer.

**Fix:** This is inherent to count-vs-cap detection and cannot be resolved
without a relay-volunteered completeness signal, but the blast radius must be
bounded so an honest exactly-cap relay is not requeued forever. Tag this error
distinctly from a network timeout (see WR-03) so the caller can cap retries on
the ambiguous case, and add a regression test that documents the accepted
false-positive:
```rust
// Honest relay, exactly cap at the oldest second, genuinely complete:
// paginate_chunk currently returns Err (accepted false-positive). This test
// pins that behavior so it is not silently regressed into truncation.
```

### WR-02: The `prev_until` disjunct is now dead — it can never be the deciding branch

**File:** `src/relay/fetch.rs:163`, `:97-100`, `:178-180`

**Issue:** The condition is
`prev_until == Some(current_until) || page_back(...) == Some(current_until)`.
For the `prev_until` half to be the deciding branch, it must be true while the
`page_back` half is false. But `prev_until == Some(current_until)` only becomes
true after the loop pinned `until` via line 174-175 (`until = next`), where
`next` came from `page_back(...)` returning `Some(current_until)` on the prior
iteration. On the pinned re-visit, reaching this branch still requires
`returned >= cap`, and a relay re-serving the same pinned prefix yields the same
`oldest`, so `page_back(...) == Some(current_until)` is true too. I could not
construct an input where the `prev_until` disjunct fires but the `page_back`
disjunct does not — and the comment at lines 148-152 itself states the page_back
check "is the stronger first-visit detector," making the 2-visit guard a strict
subset.

This is dead code dressed as defense-in-depth. It is not harmful (an OR can only
widen detection), but it keeps `prev_until` (lines 97-100, 180) as load-bearing
state that is now meaningful only in comments, inviting future maintainers to
reason about an invariant that no longer matters. Confirming the deadness: in
`tests/pagination.rs`, removing the `prev_until` disjunct would not fail any
test — `no_newer_event_boundary_stall_surfaces_error` and
`deterministic_boundary_stall_surfaces_error` both pass on the page_back
disjunct alone, and `capped_reserved_prefix_at_pinned_boundary_surfaces_error`
(line 100) likewise fires via page_back on its window 2.

**Fix:** Either remove `prev_until` and the disjunct entirely (relying on the
page_back detector the tests already exercise), or, if retained deliberately,
add a test that fails when ONLY the `prev_until` disjunct is removed AND a test
that fails when ONLY the `page_back` disjunct is removed, proving both branches
carry weight. As written, the `prev_until` branch is unverified by construction.

### WR-03: Boundary-stall and budget errors reuse `FetchTimeout`, conflating three failure modes the caller must distinguish

**File:** `src/relay/fetch.rs:166-170` (stall), `:107-109` (budget), `:251` (real timeout)

**Issue:** The boundary-second stall (line 166), the page-budget exhaustion
(line 107), and a genuine network timeout (`fetch_window_with_deadline`, line
251) all return `RelayError::FetchTimeout`. The caller's requeue policy cannot
tell them apart without parsing the message string. This directly undermines the
fix for WR-01: bounding retries on the ambiguous honest-exactly-cap case
requires distinguishing "boundary stall (may already be complete)" from "relay
timed out (retry later)" from "relay hostile, budget blown (alert operator)".
Treating all three identically means an honest exactly-cap relay (WR-01) is
requeued on the same unbounded path as a transient timeout, and operators
reading `FetchTimeout` logs cannot separate a slow relay from a truncating one —
working against the relay-health observability requirement in CLAUDE.md.

**Fix:** Introduce dedicated variants `BoundaryStall { until: u64 }` and
`PageBudgetExceeded(String)` alongside `FetchTimeout`, so the requeue layer,
retry caps (WR-01), and metrics can treat them differently.

## Info

### IN-01: Zero-new-id capped window with `oldest < current_until` still breaks `Ok` — a symmetric blind spot

**File:** `src/relay/fetch.rs:122`, `:161-172`

**Issue:** The detector only fires when `page_back(...) == Some(current_until)`,
i.e. `oldest == current_until`. A relay that re-serves a capped page whose
`oldest` is OLDER than `current_until` but contributes zero new ids (echoing an
already-seen older page) makes the condition false, so the loop `break`s with
`Ok` at line 172 — even though `returned >= cap` signals the relay may still be
truncating. This is the symmetric counterpart to the stall 02-12 fixed: the
"pinned at current_until" stall is now caught, but a "re-served older capped
page, zero new" window still completes silently. An adversary controls `oldest`,
so the zero-new-id `break` is not unconditionally safe. This is narrower than
WR-01 and likely out of scope for 02-12, but it means the genuine-exhaustion
escape hatch (comment lines 154-160) rests on the assumption that a zero-new-id
capped window that "advanced" is honest — which is not enforced.

**Fix:** Consider treating ANY `returned >= cap && new_ids == 0` window as a
requeue (the `MAX_PAGES_PER_CHUNK` budget already bounds the loop, so this
cannot spin). If that is too aggressive for honest relays, document why an
older-page echo is assumed impossible.

### IN-02: `MAX_PAGES_PER_CHUNK` worst-case `out` figure is cap-dependent, stated as a constant

**File:** `src/relay/fetch.rs:43-47`

**Issue:** The doc states worst-case `out` is `MAX_PAGES_PER_CHUNK * cap` ≈ 5M
events, which assumes `cap == 500` (the DEFAULT_MAX_LIMIT ceiling). The budget
guard runs at the top of the loop before the fetch and `pages` is incremented
after (line 120), so the loop performs exactly `MAX_PAGES_PER_CHUNK` fetches
before tripping — confirmed by `budget_guard_errors_on_adversarial_relay`
asserting `untils().len() == MAX_PAGES_PER_CHUNK` (tests/pagination.rs:170-174).
The magnitude is correct, but the 5M figure reads as a fixed constant when it
scales with the relay's actual `max_limit`.

**Fix:** Reword to "≈ `MAX_PAGES_PER_CHUNK * relay_max_limit` (≤ ~5M at the
500-cap ceiling)" to avoid implying a fixed 5M regardless of cap.

---

_Reviewed: 2026-06-13T09:00:00Z_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
