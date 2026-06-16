# Phase 3: Graph Writer & BFS Frontier - Pattern Map

**Mapped:** 2026-06-13
**Files analyzed:** 8 (3 new src modules, 1 new migration, 2 new tests, 2 modified)
**Analogs found:** 8 / 8

This is a **wiring-and-verification** phase. Every hard primitive already exists
and is tested in Phases 1-2 (`apply_follow_list`, `upsert_pubkey`,
`set_fetch_status`, `acquire_validated_lists_client`, `ingest_events`). The new
code is small: one additive migration, a `crawl` module (claim / sweep / seed +
bounded worker loop + per-batch composition seam), and two test suites. The
overriding constraint: **consume and verify** these analogs, do **not** rebuild
them. The planner must frame tasks accordingly.

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match Quality |
|-------------------|------|-----------|----------------|---------------|
| `migrations/0002_frontier.sql` | migration | DDL / batch | `migrations/0001_graph_schema.sql` | exact (additive idempotent migration) |
| `src/crawl/frontier.rs` (NEW) | store/queue | CRUD (claim/lease/sweep/seed) | `src/store/pubkeys.rs` | role-match (sqlx write helpers on `pubkeys`) |
| `src/crawl/apply.rs` (NEW) | service | transform / request-response | `src/relay/mod.rs` `acquire_validated_lists` + `src/store/follows.rs` | role-match (composition seam) |
| `src/crawl/mod.rs` (NEW) | service / orchestrator | event-driven loop | `src/relay/mod.rs` `spawn_notice_consumer` (tokio spawn) + `src/store/mod.rs` (module entry) | partial (bounded worker loop is new; spawn/Arc idiom matches) |
| `src/lib.rs` (MODIFIED) | config | — | `src/lib.rs` (existing module re-exports) | exact |
| `src/store/pubkeys.rs` (MODIFIED, optional) | store | CRUD | `src/store/pubkeys.rs` `set_fetch_status` | exact (add `requeue_or_fail` / `fetch_attempts` helpers) |
| `tests/graph_writer.rs` (NEW) | test | integration (Postgres) | `tests/edge_diff.rs` + `tests/acquire_pipeline.rs` | exact (Postgres-fixture diff test + real `ValidatedFollowList`) |
| `tests/frontier.rs` (NEW) | test | integration (Postgres + mock relay) | `tests/concurrency.rs` (two pools) + `tests/migrations.rs` (idempotency) + `tests/mock_relay/mod.rs` | role-match |
| `.sqlx/*.json` (REGENERATED) | build artifact | — | existing `.sqlx/query-*.json` | exact (`cargo sqlx prepare`) |

## Pattern Assignments

### `migrations/0002_frontier.sql` (migration, additive idempotent DDL)

**Analog:** `migrations/0001_graph_schema.sql`

**Header / idempotency convention** (0001 lines 1-15) — every migration opens
with a doc block stating it is idempotent and naming the scope. Mirror this.

**Status CHECK domain** to widen (0001 lines 20-21):
```sql
status             TEXT  NOT NULL DEFAULT 'discovered'
                   CHECK (status IN ('discovered','fetched','not_found','failed')),
```
A CHECK constraint cannot be edited in place (RESEARCH Pitfall 1). The Phase 1
CHECK is **inline + unnamed** (auto-named `pubkeys_status_check`). The 0002
migration must `DROP CONSTRAINT IF EXISTS pubkeys_status_check` then
`ADD CONSTRAINT pubkeys_status_check CHECK (... 'in_progress' ...)` with an
explicit name, and use `ADD COLUMN IF NOT EXISTS` for the new columns. **Verify
the actual auto-generated name via `\d pubkeys` against a migrated DB** before
finalizing (RESEARCH Assumption A1).

**Partial index pattern to follow** (0001 lines 42-45):
```sql
CREATE INDEX IF NOT EXISTS pubkeys_status_idx ON pubkeys (status)
    WHERE status IN ('discovered','not_found','failed');
```
This already covers the `WHERE status='discovered'` claim scan. Do NOT add an
`in_progress` index unless startup-sweep latency is shown to matter (RESEARCH A3).

**Contract-view + COMMENT ON convention** (0001 lines 47-94) — `pubkey_freshness`
(lines 55-56) currently exposes `status`. Per D-12, do NOT expose transient
`in_progress`; redefine the view with `CREATE OR REPLACE VIEW` to collapse it
(e.g. `CASE WHEN status='in_progress' THEN 'discovered' ELSE status END`). New
internal columns get `COMMENT ON COLUMN ... IS 'INTERNAL: ...'` exactly like the
existing bookkeeping comments (0001 lines 84-94). Update the
`pubkey_freshness.status` contract comment (0001 lines 79-80) to keep the
documented domain `{discovered, fetched, not_found, failed}` truthful.

**New columns (D-12):**
```sql
ALTER TABLE pubkeys ADD COLUMN IF NOT EXISTS claimed_at     TIMESTAMPTZ;
ALTER TABLE pubkeys ADD COLUMN IF NOT EXISTS fetch_attempts SMALLINT NOT NULL DEFAULT 0;
```

---

### `src/crawl/frontier.rs` (store/queue: claim / lease / sweep / seed)

**Analog:** `src/store/pubkeys.rs`

**Module doc + import block** (pubkeys.rs lines 1-11):
```rust
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::error::StoreError;
```
Reuse `StoreError` (no new error type needed — claim/sweep are sqlx writes).

**Single-statement write idiom** to copy for `seed_anchor` and
`reclaim_stale_on_startup` (`set_fetch_status`, pubkeys.rs lines 54-70):
```rust
pub async fn set_fetch_status(pool: &PgPool, id: i64, status: &str, ts: DateTime<Utc>) -> Result<(), StoreError> {
    sqlx::query!(
        "UPDATE pubkeys SET status = $2, last_fetched_at = $3 WHERE id = $1",
        id, status, ts
    )
    .execute(pool)
    .await?;
    Ok(())
}
```
- `seed_anchor` reuses `upsert_pubkey` (pubkeys.rs lines 30-45) verbatim — the
  upsert lands the anchor as `discovered` (D-03). No new code, just a call.
- `reclaim_stale_on_startup` is a one-shot `UPDATE pubkeys SET status='discovered',
  claimed_at=NULL WHERE status='in_progress'` returning `rows_affected()` (D-06).

**Batch claim with `FOR UPDATE SKIP LOCKED`** (NEW pattern — no in-repo analog
for the CTE form; closest existing query macros are the `query_scalar!`/`query!`
calls throughout `follows.rs`/`pubkeys.rs`). Use the `query!` macro with a CTE:
```sql
WITH claimed AS (
    SELECT id FROM pubkeys
    WHERE status = 'discovered'
    ORDER BY id
    LIMIT $1
    FOR UPDATE SKIP LOCKED
)
UPDATE pubkeys p SET status = 'in_progress', claimed_at = now()
FROM claimed WHERE p.id = claimed.id
RETURNING p.id, p.pubkey;
```
Run it as its **own short transaction** — the row lock releases at commit, NOT
during the multi-second fetch (D-04 / RESEARCH Pitfall 6). The `pubkey` column
comes back as `Vec<u8>` (bytea) — convert to `&[u8]` / nostr `PublicKey` at the
fetch boundary, mirroring how `follows.rs` reads `applied_event_id` as
`Vec<u8>` (follows.rs lines 48-53).

**Transaction idiom** to copy if wrapping the claim explicitly
(follows.rs lines 94-139): `let mut tx = pool.begin().await?; ... tx.commit().await?;`
with `.execute(&mut *tx)`.

**Retry/terminal helper (`requeue_or_fail`)** — route every terminal transition
through `set_fetch_status` so `last_fetched_at` is always stamped (RESEARCH
Pitfall 5 / FRESH-01). Bump `fetch_attempts`, return to `discovered` if under the
cap, else `failed` (D-09 / RESEARCH Pitfall 7).

---

### `src/crawl/apply.rs` (service: acquire -> union -> ingest -> upsert -> apply seam)

**Analog:** `src/relay/mod.rs` (`acquire_validated_lists`, lines 149-177) +
`src/store/follows.rs` (`apply_follow_list`) + `src/ingest/mod.rs`
(`ingest_events`, `ValidatedFollowList`)

**The wired `apply_validated` seam** (the bridge `ValidatedFollowList` ->
existing writer). The fields map directly per the `ValidatedFollowList` doc
(ingest/mod.rs lines 28-56):
```rust
async fn apply_validated(pool: &PgPool, vfl: &ValidatedFollowList) -> Result<bool, StoreError> {
    let follower_id = upsert_pubkey(pool, &vfl.follower_pubkey.to_bytes()).await?;
    let mut followee_ids = Vec::with_capacity(vfl.followee_pubkeys.len());
    for fp in &vfl.followee_pubkeys {
        // upsert_pubkey IS the discovery/enqueue mechanism (D-03): a new followee
        // lands as 'discovered' — this is what makes CRAWL-02 structural.
        followee_ids.push(upsert_pubkey(pool, &fp.to_bytes()).await?);
    }
    apply_follow_list(pool, follower_id, vfl.event_id.as_bytes(), vfl.created_at, &followee_ids).await
}
```
`apply_follow_list` (follows.rs lines 40-142) already does: idempotency
short-circuit on `applied_event_id` (lines 47-71), self-follow drop (lines
73-78), Rust-side diff (lines 80-91), DELETE+INSERT+freshness UPDATE in one txn
(lines 93-139). **Do not touch it.**

**Fan-out + single-ingest composition** (RESEARCH Open Question 1 — RESOLVE in
planning). The `acquire_validated_lists` seam is generic over an injected fetch
source (relay/mod.rs lines 149-177): it takes a `fetch: F` closure and runs
`ingest::ingest_events` ONCE over its output (lines 163-176). This is the lever
for D-08's "run `ingest_events` once over the cross-relay union": collect the
raw `Vec<Event>` union across all curated relays inside the injected `fetch`
closure, then a single `acquire_validated_lists` call resolves newest-wins over
the whole union. The production driver `acquire_validated_lists_client`
(relay/mod.rs lines 203-236) shows the per-relay wiring (NIP-11 cap source, rate
limiter, backoff reset) to fan out across.

**Per-author batch result resolution** (D-07/D-09/D-10):
```rust
for claimed in &batch {
    match validated_for.get(&claimed.id) {
        Some(vfl) => { apply_validated(pool, vfl).await?; }                 // -> 'fetched' inside apply_follow_list
        None if relays_answered => set_fetch_status(pool, claimed.id, "not_found", now).await?, // D-10
        None /* transient */ => requeue_or_fail(pool, claimed.id, max_attempts, now).await?,     // D-09
    }
}
```

---

### `src/crawl/mod.rs` (service/orchestrator: bounded worker loop)

**Analog:** `src/store/mod.rs` (module entry + doc, lines 1-46) for the module
shape; `src/relay/mod.rs` `spawn_notice_consumer` (lines 280-311) for the
`tokio::spawn` + `Arc` sharing idiom.

**Module doc + `pub mod` declarations** (store/mod.rs lines 1-13) — open with a
doc block naming the phase requirements (CRAWL-01..04), then
`pub mod frontier; pub mod apply;`.

**Constant-with-rationale idiom for defaults** (store/mod.rs lines 20-24) — batch
size N, concurrency cap, max retry attempts are Phase 3 parameters with sane
defaults (config-sourcing is Phase 4). Document each like `MAX_CONNECTIONS`:
```rust
/// Maximum connections in the crawler's writer pool.
/// Sizing is explicit Claude discretion (RESEARCH A4); 8 is a sane default ...
const MAX_CONNECTIONS: u32 = 8;
```

**Bounded worker pool (CRAWL-04)** — NEW code; no exact in-repo analog. Use
`tokio::sync::Semaphore` over a claim->spawn loop, OR a fixed `JoinSet` of
workers (Claude's discretion, RESEARCH Pattern 4). The `tokio::spawn` +
`Arc::clone` pattern from `spawn_notice_consumer` (relay/mod.rs lines 280-311)
is the closest idiom for spawning bounded tasks that share state. The "queue" is
`pubkeys.status` in the DB, so the loop terminates when `claim_batch` returns
empty (frontier drained).

---

### `src/lib.rs` (MODIFIED)

**Analog:** `src/lib.rs` itself (lines 1-13). Add `pub mod crawl;` to the
existing `pub mod` block (lines 8-11), following the doc-comment-per-module
style. No re-export needed unless a crawl error type is introduced.

---

### `tests/graph_writer.rs` (NEW — GRAPH-02 through the wired seam)

**Analog:** `tests/edge_diff.rs` (Postgres-fixture diff assertions) +
`tests/acquire_pipeline.rs` (building real signed events / `ValidatedFollowList`)

**Test harness boilerplate** (edge_diff.rs lines 7-11, 31-36):
```rust
mod common;
use web_of_trust::store::{self, follows::apply_follow_list, pubkeys::upsert_pubkey};
// ...
let (_pg, url) = common::start_postgres().await?;
let pool = store::connect(&url).await?;
store::run_migrations(&pool).await?;
```
`store::run_migrations` now also applies 0002 (it runs the whole `migrations/`
dir, store/mod.rs lines 43-46) — no test change needed for the new migration.

**Edge-delta + zero-touch assertion idiom** (edge_diff.rs lines 18-27, 102-143)
— `edge_count` helper, then assert exact row deltas; re-apply with the SAME
event id and assert `changed == false` and zero edge rows touched, `fetch_count`
bumps but `change_count` does not.

**Building real `ValidatedFollowList` inputs** — unlike `edge_diff.rs` which uses
synthetic id arrays, GRAPH-02 verification needs REAL validated events through
the `apply_validated` seam. Use `common::signed_event` / `common::keys`
(common/mod.rs lines 50-72) to build kind-3 events, run them through
`ingest::ingest_events` (or the `acquire_validated_lists` seam, as
`acquire_pipeline.rs` lines 108-116 demonstrates) to get a `ValidatedFollowList`,
then drive it through `apply_validated`.

---

### `tests/frontier.rs` (NEW — claim/lease, reachability, crash-resume, concurrency)

**Analog:** `tests/concurrency.rs` (two-pool concurrent access) +
`tests/migrations.rs` (idempotency assertion via `information_schema`) +
`tests/mock_relay/mod.rs` (deterministic offline fetch)

**Two-pool concurrency idiom for SKIP-LOCKED no-double-claim** (concurrency.rs
lines 21-81) — separate `store::connect(&url)` pools and `tokio::spawn`ed tasks
proxy two workers; assert two concurrent `claim_batch` calls never return the
same id. The timeout-guarded read (concurrency.rs lines 68-77) is the model for
asserting non-blocking behavior.

**Migration-idempotency extension** (migrations.rs lines 16-47) — extend the
existing pattern: migrate, count `_sqlx_migrations WHERE success`, re-run, assert
no-op. Add an assertion that re-running 0002 is a no-op (RESEARCH Pitfall 1).

**Deterministic offline relay** (mock_relay/mod.rs lines 45-106) — reuse
`ScriptedRelay` + `fetch_fn` for crash-resume/BFS tests rather than live relays
(RESEARCH Environment: live relays are a Phase 4 concern). `ScriptedRelay::new`
takes pre-built windows; `event_at(seed, ts)` (lines 32-38) builds signed kind-3
events.

**Crash-resume test shape** (CRAWL-03) — leave rows `in_progress` (simulate
crash, no clean shutdown), invoke `reclaim_stale_on_startup`, run a second pass;
assert orphans return to `discovered` then `fetched`, and any pre-existing
`fetched` row's `fetch_count` is unchanged (never re-fetched). Mirror the direct
`sqlx::query_scalar!` introspection style from edge_diff.rs lines 116-134.

**Spam-island reachability test** (CRAWL-02) — insert an isolated pubkey nobody
reachable follows; assert `claim_batch` never returns it (it is `discovered` only
if upserted, and structural reachability means it never is — RESEARCH Pitfall 4).

---

### `.sqlx/*.json` (REGENERATED build artifact)

**Analog:** existing `.sqlx/query-*.json` (e.g. the `upsert_pubkey` entry, a
`query` string + `describe` columns/parameters block). After writing 0002 and all
new `query!`/`query_scalar!` calls, run `cargo sqlx prepare` against a DB migrated
to 0002 and commit the new `.sqlx/*.json` so `SQLX_OFFLINE=true cargo build`
stays green in CI (RESEARCH Pitfall 2; Phase 1 convention). Existing entries for
unchanged queries remain valid.

## Shared Patterns

### sqlx query style (raw SQL via macros, never string-formatted)
**Source:** `src/store/mod.rs` lines 9-10; every query in `follows.rs`/`pubkeys.rs`
**Apply to:** all of `src/crawl/frontier.rs`, `src/crawl/apply.rs`
All queries use `sqlx::query!` / `query_scalar!` with `$1`-style bind parameters
— SQL is NEVER string-formatted (T-03-01). bytea pubkeys round-trip as `Vec<u8>`
/ `&[u8]`. Offline metadata in `.sqlx/` makes the macros compile without a live
DB.

### Transaction pattern (multi-statement atomic writes)
**Source:** `src/store/follows.rs` lines 94-139
**Apply to:** the batch-claim short transaction in `frontier.rs`
```rust
let mut tx = pool.begin().await?;
// ... .execute(&mut *tx).await? ...
tx.commit().await?;
```
The claim must be its OWN short transaction; the multi-second fetch happens
AFTER commit, with no lock held (D-04 / RESEARCH Pitfall 6).

### Error handling (typed per-boundary errors, transparent sqlx wrap)
**Source:** `src/error.rs` lines 13-28 (`StoreError`)
**Apply to:** all new store/crawl functions
Reuse `StoreError` for DB writes (it already `#[from]`s `sqlx::Error` and
`MigrateError`); reuse `RelayError` at the fetch boundary. The count-and-skip vs.
genuine-error split (error.rs lines 64-103) means the retry policy keys off
genuine `RelayError`s from the fetch path, NOT ingest count-and-skip rejections
(CONTEXT code_context). Do not introduce a new error enum unless the crawl
driver genuinely needs to unify `StoreError` + `RelayError` — if so, follow the
one-enum-per-boundary `thiserror` shape (error.rs lines 14, 37, 76).

### FRESH-01 terminal-status stamping
**Source:** `src/store/pubkeys.rs` lines 54-70 (`set_fetch_status`)
**Apply to:** every terminal transition in `frontier.rs`/`apply.rs`
Route every `not_found`/`failed` transition through `set_fetch_status` (it stamps
`last_fetched_at`); a raw `UPDATE status=...` that bypasses it would drop the
timestamp and break FRESH-01 + the Phase 4 staleness scan (RESEARCH Pitfall 5).
`fetched` is stamped inside `apply_follow_list` (follows.rs lines 120-137).

### Integration-test harness (testcontainers Postgres)
**Source:** `tests/common/mod.rs` lines 29-34 (`start_postgres`); `tests/edge_diff.rs` lines 31-36
**Apply to:** `tests/graph_writer.rs`, `tests/frontier.rs`
`mod common;` + `start_postgres()` -> `store::connect` -> `store::run_migrations`.
Deterministic `pk(seed)` / `common::keys(seed)` fixtures. Offline event fixtures
(`signed_event`, `forged_event`, etc.) and the `ScriptedRelay` mock for any relay
interaction — never live relays in Phase 3 tests.

## No Analog Found

| File / Pattern | Role | Data Flow | Reason |
|----------------|------|-----------|--------|
| `FOR UPDATE SKIP LOCKED` batch-claim CTE | store/queue | CRUD | No existing queue-claim query in the repo. Pattern is well-defined in RESEARCH Pattern 3; query-macro *style* analog is `pubkeys.rs`/`follows.rs`, but the CTE+SKIP LOCKED form is new. Regenerate `.sqlx` after writing it. |
| Bounded worker pool (Semaphore / JoinSet loop) | orchestrator | event-driven | No existing concurrency-bounded loop. Closest idiom is `spawn_notice_consumer`'s `tokio::spawn`+`Arc` (relay/mod.rs 280-311), but the claim->fetch->apply worker loop with a permit cap is new (RESEARCH Pattern 4). |

## Metadata

**Analog search scope:** `src/store/`, `src/relay/`, `src/ingest/`, `migrations/`,
`tests/`, `.sqlx/`
**Files scanned:** ~20 (all of `src/`, key tests, both migrations, sample `.sqlx`)
**Pattern extraction date:** 2026-06-13
