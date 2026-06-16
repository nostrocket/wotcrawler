---
phase: 03-graph-writer-bfs-frontier
reviewed: 2026-06-13T22:30:00Z
depth: standard
files_reviewed: 8
files_reviewed_list:
  - migrations/0002_frontier.sql
  - src/crawl/apply.rs
  - src/crawl/frontier.rs
  - src/crawl/mod.rs
  - src/lib.rs
  - tests/frontier.rs
  - tests/graph_writer.rs
  - tests/migrations.rs
findings:
  critical: 1
  warning: 3
  info: 2
  total: 6
status: resolved
resolution:
  fixed: [CR-01, WR-01, WR-02, WR-03]
  skipped: [IN-01, IN-02]
  skipped_reason: "Info-tier, out of fix scope (CR/WR only); IN-02 is non-actionable per the review (subsumed by the CR-01 fix)."
---

# Phase 03: Code Review Report

**Reviewed:** 2026-06-13T22:30:00Z
**Depth:** standard
**Files Reviewed:** 8
**Status:** issues_found

## Summary

Reviewed the DB-resident BFS frontier implementation: migration 0002, the claim/reclaim/requeue primitives, the `process_batch` / `apply_validated` composition seam, the bounded `run_crawl` worker loop, and all three test files. The SQL itself is correct (parameterized, no injection surface), the SKIP LOCKED claim path is sound, and the idempotency / newest-wins contract is preserved end-to-end.

Three issues require attention before this code is production-ready: one BLOCKER (worker errors silently swallowed), one WARNING that corrupts operational semantics over time (stale `claimed_at` on terminal `failed` rows), and one WARNING that risks permanently failing pubkeys that were never actually tried `max_attempts` times (crash counter leak in the startup reclaim).

---

## Critical Issues

### CR-01: Worker errors silently swallowed by `workers.retain`

**File:** `src/crawl/mod.rs:198`

**Issue:** `workers.retain(|h| !h.is_finished())` removes finished `JoinHandle`s without inspecting their result. A worker that returns `Err(StoreError::Sqlx(...))` — for example a DB connection drop mid-`apply_follow_list`, an `upsert_pubkey` failure, or any other store error — has its error silently dropped. The affected pubkeys stay `in_progress` indefinitely (no terminal status, no `last_fetched_at` stamp), invisible to the next claim scan (which only looks at `status = 'discovered'`). They will never be retried until the process is restarted and `reclaim_stale_on_startup` runs. Under sustained connection instability this accumulates silently.

The `workers.drain(..)` path (triggered when the frontier temporarily goes empty) does surface errors via `join_worker`, but that path is only reached when `claim_batch` returns empty — which may not happen until many more batches have been processed.

**Fix:**

```rust
// Replace the silent retain with a draining collect that joins finished handles:
let mut still_running = Vec::with_capacity(workers.len());
for handle in workers.drain(..) {
    if handle.is_finished() {
        join_worker(handle).await?;   // surface any error immediately
    } else {
        still_running.push(handle);
    }
}
workers = still_running;
```

Or, more idiomatically, keep a secondary `finished` pass after the retain:

```rust
let (done, running): (Vec<_>, Vec<_>) = workers.drain(..).partition(|h| h.is_finished());
workers = running;
for handle in done {
    join_worker(handle).await?;
}
```

---

## Warnings

### WR-01: Terminal `failed` rows retain a stale `claimed_at` lease timestamp

**File:** `src/crawl/frontier.rs:147-149`

**Issue:** In the `CASE … THEN 'failed'` branch of `requeue_or_fail`, `claimed_at` is preserved unchanged (`THEN claimed_at`). A `failed` row is no longer leased to any worker; the non-NULL `claimed_at` value is semantically wrong (the comment on the column reads "NULL when not leased"). The startup reclaim only resets rows `WHERE status = 'in_progress'`, so `failed` rows keep their stale timestamp forever.

This is an operational / correctness issue for any monitoring, debugging, or Phase 4 staleness logic that uses `claimed_at IS NOT NULL` to identify in-flight rows — it will include all `failed` rows as false positives.

**Fix:**

```sql
-- In the terminal (failed) branch, clear claimed_at:
claimed_at = CASE
    WHEN fetch_attempts + 1 >= $2::int2 THEN NULL   -- ← was: claimed_at
    ELSE NULL
END
```

Since both branches produce `NULL`, the entire `claimed_at` arm simplifies to:

```sql
claimed_at = NULL,
```

which is correct: both requeue (`discovered`) and terminal (`failed`) releases the lease.

---

### WR-02: `reclaim_stale_on_startup` does not reset `fetch_attempts`, allowing crash-loops to permanently fail pubkeys

**File:** `src/crawl/frontier.rs:99-108`

**Issue:** The reclaim SQL is:

```sql
UPDATE pubkeys SET status = 'discovered', claimed_at = NULL
WHERE status = 'in_progress'
```

It does not reset `fetch_attempts`. A pubkey that accumulated `max_attempts - 1` transient failures before being claimed (→ `in_progress`) and then orphaned by a crash will, after reclaim, be one relay error away from permanent `failed` status — even though it was never actually attempted `max_attempts` times successfully. In a pattern of repeated crashes (e.g. OOM during large-batch processing), every pubkey that was in-flight at crash time will have its counter bumped toward the cap without any actual relay failure, eventually permanently marking valid pubkeys as `failed`.

The doc comment on `reclaim_stale_on_startup` says "re-fetching it is harmless because `apply_follow_list` is idempotent" — this is true for the edge-diff correctness, but the counter leak is not addressed.

**Fix:**

```sql
UPDATE pubkeys
SET status = 'discovered',
    claimed_at = NULL,
    fetch_attempts = 0     -- ← reset the crash-orphaned counter
WHERE status = 'in_progress'
```

Alternatively, if the design intentionally counts each crash as an attempt (to prevent crashing on a single bad pubkey forever), that policy should be documented explicitly and the `max_attempts` cap should be sized accordingly. As written, the counter is a "relay failure" count but crash-orphaned rows silently accumulate it.

---

### WR-03: `set_fetch_status` accepts an unchecked `&str` status — no compile-time domain safety

**File:** `src/store/pubkeys.rs:54-70`

**Issue:** The `status: &str` parameter is validated only by the DB CHECK constraint at runtime. The current two callers in `apply.rs` pass `"not_found"`, which is correct. But the function is `pub` and its signature provides no compile-time guarantee that a future caller cannot pass `"in_progress"` (which would bypass the lease mechanism), a typo like `"not-found"` (which would return `Ok(())` while updating zero rows if the CHECK fires silently, or raise an error), or any other out-of-domain string.

Because sqlx's `query!` macro checks the _query structure_ at compile time against the offline `.sqlx/` metadata but does NOT validate runtime-bound string values, a bad `status` string either causes a DB CHECK error surfaced as `StoreError::Sqlx` (a confusing runtime error) or, if the DB somehow accepted it, would corrupt the state machine.

**Fix:** Introduce a typed enum at the store boundary:

```rust
#[derive(Debug, Clone, Copy)]
pub enum FetchStatus {
    Discovered,
    Fetched,
    NotFound,
    Failed,
}

impl FetchStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Discovered => "discovered",
            Self::Fetched    => "fetched",
            Self::NotFound   => "not_found",
            Self::Failed     => "failed",
        }
    }
}

pub async fn set_fetch_status(
    pool: &PgPool,
    id: i64,
    status: FetchStatus,
    ts: DateTime<Utc>,
) -> Result<(), StoreError> { … }
```

At minimum, rename the parameter to `status_str` and add a debug-mode assertion:
```rust
debug_assert!(
    ["discovered", "fetched", "not_found", "failed"].contains(&status),
    "set_fetch_status called with invalid domain value: {status}"
);
```

---

## Info

### IN-01: `now_ts` is a redundant alias of `now` in `process_batch`

**File:** `src/crawl/apply.rs:135`

**Issue:** `let now_ts = now;` creates an unnecessary copy/alias of the `now` parameter (which is `Copy`), then `now` is used directly on line 146 anyway. The alias adds no clarity.

**Fix:** Delete the alias and pass `now` directly to `acquire_validated_lists`:

```rust
let result = acquire_validated_lists(
    &requested,
    want_kind,
    now,   // ← directly, no alias needed
    future_clamp_secs,
    follow_cap,
    union_fetch,
)
.await;
```

---

### IN-02: `apply_validated` performs N individual `upsert_pubkey` calls outside any transaction

**File:** `src/crawl/apply.rs:61-67`

**Issue:** For a batch author with 1000 followees, `apply_validated` issues 1001 individual DB round-trips (`upsert_pubkey` × (1 follower + 1000 followees)) before opening the `apply_follow_list` transaction. If the DB connection is lost after some followees are upserted, those orphaned `discovered` rows are harmless (they'll be crawled normally), but the follower row remains `in_progress`. Because worker errors are silently swallowed (CR-01), this `in_progress` row is stranded until restart.

This is not a standalone bug (it becomes a non-issue when CR-01 is fixed), but it illustrates why CR-01 has cascading effects. No change required here beyond fixing CR-01.

---

_Reviewed: 2026-06-13T22:30:00Z_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
