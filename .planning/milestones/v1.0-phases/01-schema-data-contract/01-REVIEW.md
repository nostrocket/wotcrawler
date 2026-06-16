---
phase: 01-schema-data-contract
reviewed: 2026-06-12T07:13:53Z
depth: standard
files_reviewed: 12
files_reviewed_list:
  - migrations/0001_graph_schema.sql
  - src/error.rs
  - src/lib.rs
  - src/store/follows.rs
  - src/store/mod.rs
  - src/store/pubkeys.rs
  - tests/bootstrap.rs
  - tests/common/mod.rs
  - tests/concurrency.rs
  - tests/contract.rs
  - tests/edge_diff.rs
  - tests/migrations.rs
findings:
  critical: 2
  warning: 6
  info: 3
  total: 11
status: issues_found
---

# Phase 1: Code Review Report

**Reviewed:** 2026-06-12T07:13:53Z
**Depth:** standard
**Files Reviewed:** 12
**Status:** issues_found

## Summary

Reviewed the Phase 1 schema migration, the sqlx store layer (`connect`/`run_migrations`, `upsert_pubkey`/`set_fetch_status`, `apply_follow_list`), and all five integration test files. The schema and contract views are well-structured, all SQL is parameterized (no injection surface found), and the test suite covers migration idempotency, contract shape, and the documented happy paths.

The core defects are concentrated in `apply_follow_list`, the single most important function in the phase. Its read-compute-write structure is not actually atomic — the idempotency check and the current-edge read run on the pool *outside* the transaction — and it never performs the newest-wins comparison that `applied_created_at` exists to support (per the migration's own D-07 comment). For a crawler whose stated purpose is taming *adversarial* kind-3 data fetched concurrently from many relays, both are correctness failures: stale events can roll the graph back, and concurrent applies for the same follower can commit an edge set that matches no event. Both are fixable inside the existing transaction with a row lock and one comparison.

Secondary issues: inconsistent input validation at the store boundary (pubkeys validated, event ids not), silent no-op semantics in `set_fetch_status`, bookkeeping divergence between the two paths that stamp `last_fetched_at`, and a concurrency test that swallows all writer errors and can pass vacuously.

## Critical Issues

### CR-01: Newest-wins (INGEST-03 / D-07) is never enforced — older events overwrite newer state

**File:** `src/store/follows.rs:40-141` (cf. `migrations/0001_graph_schema.sql:27-28, 85-88`)
**Issue:** The migration stores `applied_created_at` explicitly "used for newest-wins resolution (INGEST-03)" (D-07), and `apply_follow_list` receives `created_at` — but the function never compares the incoming `created_at` against the stored `applied_created_at`. Any event whose id differs from the currently applied one is applied unconditionally, *including strictly older events*. Kind-3 is a replaceable event kind and relays routinely serve stale copies; an adversarial (or merely lagging) relay returning an old kind-3 will delete current edges, resurrect long-removed follows, and rewind `applied_created_at` — silent data loss in the core graph. Nothing in the function's doc comment delegates this check to the caller, and the caller *cannot* perform it race-free anyway: only inside this function's transaction is the compare-against-current atomic with the write.
**Fix:** Inside the transaction (see CR-02), read `applied_event_id, applied_created_at` with the row lock and bail out when the incoming event is not newer:

```rust
// inside tx, after locking the pubkeys row:
if let Some(applied) = row.applied_created_at {
    if created_at <= applied {
        // optionally bump last_fetched_at/fetch_count as a fetch attempt
        tx.commit().await?;
        return Ok(false); // stale event: never regress the applied list
    }
}
```

(NIP-01 tie-break for equal `created_at` — lowest event id wins — can be added if exact replaceable-event semantics are wanted.)

### CR-02: Idempotency check and edge-diff read run outside the transaction — concurrent applies corrupt the edge set

**File:** `src/store/follows.rs:48-94`
**Issue:** The module doc claims everything happens "all in ONE transaction", but only the writes do. The `applied_event_id` short-circuit check (line 48) and the `SELECT followee_id FROM follows` current-set read (line 81) execute on the pool *before* `pool.begin()` (line 94). Two concurrent `apply_follow_list` calls for the same follower — the normal case when the same pubkey's kind-3 arrives from multiple relays at once — both read the same `current_set`, compute independent diffs, and interleave their DELETE/INSERT batches. Example: both observe `current = {}`; A applies list₁, B applies list₂; `ON CONFLICT DO NOTHING` (line 109) silently absorbs the collisions and the committed edge set becomes `list₁ ∪ list₂`, while `applied_event_id` names only whichever UPDATE committed last. The database then asserts "this follower's list is event B" while containing edges from neither-event's list. Crash-atomicity holds, but concurrent correctness — the actual risk for a multi-relay crawler — does not. The same gap lets two concurrent same-event applies both miss the short circuit and double-bump counters.
**Fix:** Move both reads inside the transaction and serialize per-follower writers with a row lock on `pubkeys`:

```rust
let mut tx = pool.begin().await?;
let row = sqlx::query!(
    "SELECT applied_event_id, applied_created_at FROM pubkeys WHERE id = $1 FOR UPDATE",
    follower_id
)
.fetch_one(&mut *tx)
.await?;
// idempotency short-circuit, newest-wins check (CR-01), then:
let current_rows = sqlx::query_scalar!(
    "SELECT followee_id FROM follows WHERE follower_id = $1",
    follower_id
)
.fetch_all(&mut *tx)
.await?;
// diff + DELETE/INSERT/UPDATE as today, then tx.commit()
```

The `FOR UPDATE` lock makes the read-diff-write sequence serializable per follower without blocking other followers or any readers (MVCC).

## Warnings

### WR-01: `event_id` length is not validated at the store boundary

**File:** `src/store/follows.rs:40-46` (cf. `src/store/pubkeys.rs:31-33`)
**Issue:** `upsert_pubkey` enforces the 32-byte invariant (V5 input validation per `src/error.rs:16-19`), but `apply_follow_list` accepts `event_id: &[u8]` of any length — including empty — and stores it as the idempotency key. Nostr event ids are always 32 bytes; a malformed/empty id silently corrupts the `applied_event_id` comparison semantics (e.g., two different "bad" ingests with `&[]` would be treated as the same event). The boundary validation is inconsistent for two values with identical invariants.
**Fix:** Validate `event_id.len() == 32` at the top of `apply_follow_list` and return a typed error (either reuse `StoreError::InvalidPubkey`'s pattern with a new `InvalidEventId(usize)` variant, or generalize the variant).

### WR-02: `set_fetch_status` silently succeeds for nonexistent pubkey ids

**File:** `src/store/pubkeys.rs:54-70`
**Issue:** The `UPDATE ... WHERE id = $1` returns `Ok(())` even when zero rows match. A caller passing a stale or wrong id (e.g., after a bug in the frontier bookkeeping) gets success while the status transition was never recorded — the pubkey stays `discovered` forever and the staleness machinery never learns the fetch happened. The `rows_affected()` result is discarded.
**Fix:**

```rust
let result = sqlx::query!(/* ... */).execute(pool).await?;
if result.rows_affected() == 0 {
    return Err(StoreError::UnknownPubkeyId(id)); // new typed variant
}
```

### WR-03: Stringly-typed `status` parameter permits contract-violating states and yields opaque errors

**File:** `src/store/pubkeys.rs:54-61`
**Issue:** `status: &str` accepts anything; out-of-domain values only fail at the database as a generic `StoreError::Sqlx(CheckViolation)` with no typed signal. Worse, in-domain misuse is *accepted*: `set_fetch_status(pool, id, "discovered", ts)` writes `status = 'discovered'` **with `last_fetched_at` stamped**, directly contradicting the public contract (`pubkey_freshness.last_fetched_at` is documented "NULL until first fetched", and SCHEMA.md defines `discovered` as "not yet fetched"). A downstream consumer filtering on `status = 'discovered' AND last_fetched_at IS NULL` assumptions would mis-weight that row.
**Fix:** Replace `&str` with a Rust enum restricted to the three legal *transition targets*:

```rust
pub enum FetchStatus { Fetched, NotFound, Failed }
```

and map to the TEXT value internally (keeps the D-09 TEXT-in-DB decision while making illegal states unrepresentable at the API).

### WR-04: `fetch_count` bookkeeping diverges between the two write paths (FRESH-03 churn data skewed)

**File:** `src/store/pubkeys.rs:54-70` (cf. `src/store/follows.rs:59-69, 120-137`)
**Issue:** `apply_follow_list` bumps `fetch_count` on every apply/confirm, and the contract comment defines `last_fetched_at` as "the most recent fetch *attempt*". `set_fetch_status` — the path used for `failed`/`not_found` attempts — stamps `last_fetched_at` but never increments `fetch_count`. So for failure-prone pubkeys, `fetch_count` undercounts attempts while `last_fetched_at` advances, and the FRESH-03 churn ratio (`change_count / fetch_count`) is computed over inconsistent denominators depending on which path recorded the attempt. The migration comment (`fetch_count`: "number of times this pubkey has been fetched") does not resolve the ambiguity.
**Fix:** Either bump `fetch_count = fetch_count + 1` in `set_fetch_status` (treating it as attempt count, matching `last_fetched_at` semantics), or rename/document `fetch_count` as "successful applies only" in the migration comments — but make the two paths agree deliberately.

### WR-05: Schema lacks byte-length CHECKs on `pubkey` / `applied_event_id` despite being the cross-project contract

**File:** `migrations/0001_graph_schema.sql:19, 27`
**Issue:** `pubkeys.pubkey` is documented (migration comment, SCHEMA.md, contract COMMENT ON) as a 32-byte x-only key, but the only enforcement is Rust-side in `upsert_pubkey`. Per project constraints, the *database* is the project boundary — operator psql sessions, future maintenance scripts, or any second writer can insert wrong-length `bytea` and the contract views will happily serve it to the spam layer. Same for `applied_event_id`. Defense-in-depth belongs in the schema when the schema is the API.
**Fix:** Add to the migration (or a follow-up additive migration, since 0001 may already be applied):

```sql
ALTER TABLE pubkeys ADD CONSTRAINT pubkey_len_32 CHECK (octet_length(pubkey) = 32);
ALTER TABLE pubkeys ADD CONSTRAINT event_id_len_32
    CHECK (applied_event_id IS NULL OR octet_length(applied_event_id) = 32);
```

### WR-06: Concurrency test swallows all writer errors — can pass vacuously (GRAPH-03 proof is unsound)

**File:** `tests/concurrency.rs:53, 68-77`
**Issue:** The writer loop discards every result: `let _ = apply_follow_list(...)`. If the writer fails on its first iteration (schema regression, FK error, pool exhaustion), the loop keeps spinning no-ops and the 100 reader queries run against an *idle* database — the test then "proves" readers don't block a writer that never wrote. The test also never asserts the writer made progress (e.g., that `fetch_count` advanced or edges exist), so the GRAPH-03 success criterion it discharges can be satisfied by a fully broken writer.
**Fix:** Propagate writer errors and assert progress:

```rust
// in the spawned task: break out of the loop on Err and return it
let res = apply_follow_list(&pool, follower, &event, Utc::now(), half).await;
if let Err(e) = res { return Err(e); }
// after the reader loop, before abort:
let fetches: i64 = sqlx::query_scalar!("SELECT fetch_count FROM pubkeys WHERE id = $1", follower)
    .fetch_one(&writer_pool).await?;
assert!(fetches > 0, "writer made no progress; concurrency test is vacuous");
```

(`writer.abort()` then still cleans up; alternatively check `writer.is_finished()` to detect early writer death.)

## Info

### IN-01: Partial-index comment misstates what the staleness scanner needs

**File:** `migrations/0001_graph_schema.sql:42-45`
**Issue:** The comment says the partial index on `status IN ('discovered','not_found','failed')` serves "the Phase 4 staleness scanner ... re-fetch candidates", but the staleness scanner's primary candidates are *stale `fetched` rows* (`status = 'fetched' AND last_fetched_at < now() - ttl` — exactly SCHEMA.md's own example query), which this index deliberately excludes. The index serves the never-fetched/retry queue only; the actual staleness scan will need an index involving `last_fetched_at` in a later migration.
**Fix:** Reword the comment to "never-fetched / retry queue" and note that the `fetched`-staleness index arrives with Phase 4, so Phase 4 doesn't assume it already exists.

### IN-02: Unknown `follower_id` surfaces as opaque `sqlx::Error::RowNotFound`

**File:** `src/store/follows.rs:48-53`
**Issue:** Calling `apply_follow_list` with a nonexistent follower id fails via `fetch_one` as a generic `StoreError::Sqlx(RowNotFound)`, indistinguishable at the type level from infrastructure failures. Callers that want to treat "unknown id" as a logic bug vs. "connection dropped" as retryable cannot branch cleanly.
**Fix:** Use `fetch_optional` and map `None` to a typed `StoreError::UnknownPubkeyId(follower_id)` (shared with WR-02's fix).

### IN-03: Stale planning reference in crate-root rustdoc

**File:** `src/lib.rs:3`
**Issue:** The doc comment "Re-exports the error type and the (Plan 03) store module" leaks internal planning-artifact numbering into published rustdoc; it will be meaningless (and eventually wrong) outside the GSD workflow.
**Fix:** Drop "(Plan 03)": "Re-exports the error type and the store module."

---

_Reviewed: 2026-06-12T07:13:53Z_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
