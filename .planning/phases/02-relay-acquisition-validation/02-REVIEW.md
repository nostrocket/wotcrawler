---
phase: 02-relay-acquisition-validation
reviewed: 2026-06-13T08:05:00Z
depth: standard
files_reviewed: 6
files_reviewed_list:
  - src/relay/fetch.rs
  - src/relay/mod.rs
  - src/relay/rate_limit.rs
  - tests/pagination.rs
  - tests/production_wiring.rs
  - tests/mock_relay/mod.rs
findings:
  critical: 1
  warning: 2
  info: 3
  total: 6
status: issues_found
---

# Phase 02 (gap-closure 02-10/02-11): Code Review Report

**Reviewed:** 2026-06-13T08:05:00Z
**Depth:** standard
**Files Reviewed:** 6 (changes introduced by commits 1ac92a9..eca222a)
**Status:** issues_found

## Summary

This review covers the gap-closure changes introduced by plans 02-10 and 02-11, which
addressed two blockers identified in the prior review cycle (preserved in git history):

- **02-10 (CR-03 residual / RELAY-03):** boundary-second stall detection via `prev_until`
  tracking in `paginate_chunk` — surfaces an unresolvable pinned-`until` stall as a requeue
  `Err` rather than silently completing with a truncated event list.
- **02-11 (WR-03 residual / RELAY-04):** per-relay rate-limiter keying — threads the
  caller's individual `relay_url` through `fetch_complete` / `fetch_complete_with_timeout`,
  replacing the joined pool-string key, and demotes `pool_label` to diagnostics.

**Disposition of prior blockers:**

- WR-03 (pool-string key collapse): **Closed.** `relay_url` is now threaded from
  `acquire_validated_lists_client` through `fetch_complete` into
  `fetch_complete_with_timeout` and used as the `paginate_chunk_gated` key and
  `FetchTimeout` label. `pool_label` is diagnostics-only. The `two_pooled_relays_get_independent_limiter_keys`
  test proves two relays mint two separate GCRA limiters.

- CR-03 residual (boundary-second stall): **Partially closed, with a correctness gap.**
  The `prev_until` tracker correctly detects the stall in the specific scenario where Window 2
  contributes at least one new event before Window 3 re-serves the same prefix. However,
  one structural scenario is not detected — see CR-01 below.

The remaining findings are one new correctness BLOCKER (a gap in the stall detection
coverage), two warnings (one a liveness concern on the `fetch_window_with_deadline` elapsed
check — a residual from prior WR-01 — and one on the fetch-before-rate-limit ordering),
and three info items.

## Narrative Findings (AI reviewer)

## Critical Issues

### CR-01: Stall detection misses the case where all pool events share the boundary second (no newer event in Window 1)

**File:** `src/relay/fetch.rs:100-168` (`paginate_chunk`, `prev_until` stall check)
**Lines:** 151-159 (stall check), 167 (`prev_until` update)

**Issue:** The `prev_until` stall detection requires that `prev_until == Some(current_until)`,
which can only be true on the **second or later** visit to a given `until=T`. `prev_until` is
updated to `Some(current_until)` only when `new_ids > 0` AND `page_back` returns `Some(next)`.
This means the stall fires on iteration N+1 (when `until` is already pinned at T from
iteration N).

**The gap:** When ALL events in the pool have `created_at == T` (no event at T+1 or later to
appear in Window 1), the first fetch (`until = now`) returns a capped window of events all at
T as their `oldest`, so `page_back` returns `Some(T)`. Now `until = T` and `prev_until =
Some(now)`. Window 2 (`until = T`) re-serves the same cap-sized prefix. `new_ids = 0`.
The stall check: `prev_until (Some(now)) == Some(current_until T)` → **FALSE**. The loop
breaks with `Ok`, silently dropping any boundary-second siblings beyond `cap`.

Concrete example (cap = 2, pool = `[A(T), B(T), C(T)]`, no newer event):

- Window 1 (`until = now`): returns `[A(T), B(T)]`. `oldest = T`, capped. `until = T`,
  `prev_until = Some(now)`.
- Window 2 (`until = T`): returns `[A(T), B(T)]` again. `new_ids = 0`. Check:
  `Some(now) == Some(T)` → **FALSE** → `break`. `Ok([A, B])` returned. **C(T) lost.**

The test `deterministic_boundary_stall_surfaces_error` avoids this gap by constructing the
pool with a newer event `N(T+1)`, ensuring Window 1 returns `[N(T+1), A(T)]` (with `A(T)` as
new), so Window 2 contributes `B(T)` as a new event (`new_ids = 1`), allowing `prev_until`
to reach `Some(T)` before Window 3 re-stalls. The test covers this specific scenario but
leaves the no-newer-event scenario untested and undetected.

**Scope note:** For kind-3 follow lists (replaceable events, typically one per pubkey), the
no-newer-event scenario is less common than for unreplaceable event kinds, but it can occur
when a relay serves old archived events with identical `created_at` values (clock-synchronized
clients, migration artifacts). The stall also goes undetected when `until = now` returns
exactly cap events that all share the oldest second and happen to have no complement in the
pool at any later second — a race-window scenario for a crawl that re-fetches a relay seconds
after the original write.

**Fix:** Detect the stall on the FIRST visit to a boundary second that returns `new_ids == 0`
while still capped, not only on the second visit. One approach: when `new_ids == 0` and
`returned >= cap`, check whether `until` is about to remain unchanged (i.e. `page_back` would
return `Some(oldest) == current_until`). If so, the relay is already pinned at this boundary
second on the very first re-request and a stall is certain:

```rust
if new_ids == 0 {
    // Stall on ANY pinned-until capped zero-new-id window, regardless of
    // whether this is the first or Nth visit to this until value.
    // When returned >= cap AND page_back would return the same `until`
    // (i.e. oldest == current_until), the relay is pinned and more events
    // may remain. Signal a requeue immediately.
    let next = page_back(returned, cap, oldest);
    if returned >= cap && next == Some(current_until) {
        return Err(RelayError::FetchTimeout(format!(
            "boundary-second stall: relay re-served the same cap-sized \
             prefix for pinned until={} with more events remaining",
            current_until.as_secs()
        )));
    }
    break;
}
```

This removes the `prev_until` dependency for the stall and catches it on the first pinned
iteration. The companion test should include a pool with no newer events so the gap is
covered by a regression test.

---

## Warnings

### WR-01: A complete-but-slow window is misclassified as a timeout (residual from prior review)

**File:** `src/relay/fetch.rs:225-241` (`fetch_window_with_deadline`), `src/relay/fetch.rs:339-345`

**Issue:** `fetch_window_with_deadline` passes the full `timeout` to `client.fetch_events`
(line 341) and then checks `started.elapsed() >= timeout` (line 237). If a relay legitimately
delivers a complete short window (< cap) but takes the full timeout duration to do so,
`fetch_events` returns `Ok` at `elapsed ≈ timeout`; the `>=` check then converts that
complete window into `Err(FetchTimeout)` and requeues it.

The next retry encounters the same slow relay and trips the identical check — a permanent
requeue loop for any consistently slow-but-honest relay. The `>=` boundary also makes a
window finishing at *exactly* the deadline a guaranteed false timeout.

This is a pre-existing concern (prior WR-01), unchanged by 02-10/02-11. Highlighted again
because the per-relay `relay_url` threading (02-11) now makes the requeue label accurate,
which is beneficial, but the false-timeout condition for slow honest relays persists.

**Fix:** Pass a strictly shorter budget to `fetch_events` than the outer deadline (e.g.
`timeout.mul_f32(0.9)` or `timeout - grace`), leaving measurable slack for the elapsed
check. Alternatively, treat a window as a timeout only when `returned == cap` AND
`elapsed >= timeout` — a complete short window can never be a silent partial truncation.

---

### WR-02: `paginate_chunk_gated` calls `fetch(filter)` before awaiting the rate limiter, so mock-relay side effects precede the GCRA gate

**File:** `src/relay/fetch.rs:198-213` (`paginate_chunk_gated`)
**Lines:** 206-210

**Issue:** The wrapping closure inside `paginate_chunk_gated` calls `fetch(filter)` on
line 206 to create the future, then awaits `registry.acquire(relay_url)` on line 208, then
awaits `fut` on line 209:

```rust
let fut = fetch(filter);      // side effects happen HERE (window pop, until recorded)
async move {
    registry.acquire(relay_url).await?;  // rate limit applied AFTER
    fut.await
}
```

For the production path (`fetch_window_with_deadline` returning a lazy async future),
`fetch(filter)` merely constructs the future — no I/O occurs until `fut.await`. The rate
limiter fires before any network request. This is correct for production.

However, for the mock relay (`ScriptedRelay::fetch_fn`), the closure pops the next
scripted window and records the `filter.until` *synchronously* when called, because
`fetch_fn` returns `std::future::ready(...)`. These side effects — window consumption and
`until` recording — therefore occur **before** the rate limiter fires.

Consequences:
1. Test assertions about the ordering of `until` recording relative to rate-limiting are
   unreliable (the mock records `until` before any GCRA delay).
2. If `registry.acquire()` were to fail in a future refactor (currently infallible), the
   window would already be popped and the error irrecoverable — the mock relay's state would
   be desynchronized.
3. The test `gated_pagination_throttles_each_window` measures elapsed time correctly (the
   real GCRA delay is still applied before `fut.await`), but the `relay.untils()` log does
   not reflect the post-GCRA ordering.

**Fix:** Restructure the closure so `fetch(filter)` is called inside the `async move` block,
after `registry.acquire()`:

```rust
paginate_chunk(authors, kind, cap, move |filter| {
    async move {
        registry.acquire(relay_url).await?;
        fetch(filter).await   // fetch called AFTER the gate
    }
})
.await
```

This requires `fetch` to be `Send` or the async block to be on the same thread, which is
compatible with the existing `FnMut(Filter) -> Fut` bound since the closure is called
sequentially. For the production path the observable behavior is identical; for the mock
path the side effects are correctly sequenced after the rate limiter.

---

## Info

### IN-01: `FetchTimeout` variant conflates genuine timeouts with boundary-second stalls

**File:** `src/relay/fetch.rs:107-109` (budget error), `src/relay/fetch.rs:153-157` (stall
error), `src/relay/fetch.rs:238` (deadline error); `src/error.rs:58-62`

**Issue:** `RelayError::FetchTimeout(String)` is reused for three distinct failure modes:
page-budget exceeded (budget error), boundary-second stall (stall error), and elapsed-time
timeout (deadline error). A caller matching on `FetchTimeout` cannot distinguish these
without parsing the message string. Future callers may want to handle them differently:
a timeout might warrant a backoff-before-retry, a stall might warrant an immediate retry
with a single-author chunk to drain the boundary second, and a budget overrun might warrant
operator alerting. The plan sanctioned this reuse ("existing requeue semantics already
match"), but the conflation reduces future extensibility.

**Fix:** Consider a dedicated `BoundaryStall(String)` variant (as the plan noted as an
option) and a `PageBudgetExceeded(String)` variant alongside `FetchTimeout`. No urgency —
the current semantics are correct for the requeue path.

---

### IN-02: `prefix_for_until_fetch_fn` sort relies on stable ordering but documents it as "stable-ish"

**File:** `tests/mock_relay/mod.rs:137`

**Issue:** The sort `sorted.sort_by(|a, b| b.created_at.cmp(&a.created_at))` is stable
(Rust's `slice::sort_by` is guaranteed stable). The companion test comment in
`tests/pagination.rs:190` says "created_at ties at T are broken by the sort's stable-ish
order." The word "stable-ish" understates the guarantee — the ordering is deterministic
and guaranteed by the language. A future maintainer reading "stable-ish" might add a
secondary sort key (e.g. by `event.id`) that changes which events fall in the cap-2 prefix
for `until=T`, potentially breaking the stall test's invariant (the test requires C(T)
to remain outside the cap-2 prefix).

**Fix:** Replace "stable-ish" with "stable (Rust sort_by is guaranteed stable)" and note
explicitly which two events appear in the prefix (A, B) and which is excluded (C), so the
constraint is clear to maintainers.

---

### IN-03: `gated_pagination_throttles_each_window` comment claims "two capped windows" but the setup has one

**File:** `tests/production_wiring.rs:42-43`

**Issue:** The test comment says "Two CAPPED windows then a short one, so paginate pages back
and issues >= 3 REQs." In reality the scripted relay has exactly two windows: `w1` (capped,
== cap) and `w2` (short, < cap). This produces exactly two REQs — not three. The comment
overstates the scenario by one window and one REQ, which may mislead future maintainers
about the test's coverage.

**Fix:** Update the comment to read "One capped window then a short one, so paginate pages
back and issues exactly 2 REQs."

---

_Reviewed: 2026-06-13T08:05:00Z_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
