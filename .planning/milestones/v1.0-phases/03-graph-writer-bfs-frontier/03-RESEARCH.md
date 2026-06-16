# Phase 3: Graph Writer & BFS Frontier - Research

**Researched:** 2026-06-13
**Domain:** PostgreSQL-resident work queue (status-as-frontier) + transactional edge-diff verification, on the locked Rust / sqlx 0.9 / Postgres 17 / nostr-sdk 0.44 stack
**Confidence:** HIGH (stack and architecture fully locked by CONTEXT D-01..D-12; existing code read directly; queue pattern verified against Postgres docs + standard practice)

## Summary

Phase 3 is a **wiring-and-verification phase, not a build-from-scratch phase**. The two hardest pieces already exist and are tested: the transactional edge-diff writer `apply_follow_list` (Phase 1, idempotent on unchanged event id, self-follow-dropping, atomic) and the full relay→ingest acquisition path `acquire_validated_lists_client` → `ingest_events` → `ValidatedFollowList` (Phase 2). This phase (1) **formally verifies** GRAPH-02 against real validated events through the wired seam, (2) builds the **crawl driver** that composes acquire → `upsert_pubkey` (follower + each followee) → `apply_follow_list`, and (3) ships **one additive migration** that turns the existing `pubkeys.status` lifecycle into a crash-safe, DB-resident BFS frontier.

The frontier is not a new data structure. Per the locked decisions, the frontier **is** the set of `pubkeys.status = 'discovered'` rows (D-01); discovery happens for free because `upsert_pubkey` lands every newly-seen followee as `discovered` (D-03); reachability-gating (CRAWL-02) holds **structurally** — a pubkey only becomes a row because someone already in the graph (seeded by the anchor) followed it, so spam islands are never inserted and never crawled (D-02). The well-trodden Postgres job-queue pattern `SELECT ... FOR UPDATE SKIP LOCKED` claims work without contention; a claim flips `discovered → in_progress` in a short transaction (D-04), the multi-second relay fetch runs **outside** any row lock, and crash recovery is a **startup-only sweep** that resets orphaned `in_progress` rows back to `discovered` (D-06). Crash safety leans on the writer's existing idempotency (re-fetching in-flight work is harmless, D-05) rather than exactly-once claiming.

**Primary recommendation:** Add the additive migration (`in_progress` status + `claimed_at` + `fetch_attempts`), build a `crawl` module with a bounded worker pool (semaphore or fixed task set) that batch-claims N `discovered` rows via `FOR UPDATE SKIP LOCKED`, fans out to curated relays through the existing `acquire_validated_lists_client`, unions the raw events, runs `ingest_events` once, then for each `ValidatedFollowList` upserts follower+followees and calls `apply_follow_list`. Verify GRAPH-02 with real fixture events and verify crash-resume by killing mid-crawl and asserting no completed pubkey is re-fetched. Do **not** add a frontier table, a depth column, an in-run reclaim sweep, or any recursive-CTE BFS.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Edge-diff persistence (GRAPH-02) | Store layer (`store::follows::apply_follow_list`) | — | Already built/tested; Phase 3 only wires the followee-id resolution seam and verifies it |
| Pubkey identity / discovery (enqueue) | Store layer (`store::pubkeys::upsert_pubkey`) | — | Upsert IS the enqueue: a new followee lands as `discovered` (D-03); no separate enqueue path |
| Frontier state / queue | Database (`pubkeys.status` + partial index) | — | DB-resident so it survives crashes (CRAWL-03); status-driven, no separate table (D-01) |
| Claim / lease (concurrency-safe dequeue) | Database (`FOR UPDATE SKIP LOCKED` claim txn) | Crawl driver | DB enforces mutual exclusion; driver owns the short claim transaction (D-04) |
| Crash recovery / reclaim | Crawl driver (startup sweep) | Database | Startup-only `in_progress → discovered` reset (D-06); in-run sweep deferred to Phase 4 |
| Relay fetch / validation | Relay+ingest layer (`acquire_validated_lists_client`, `ingest_events`) | — | Phase 2 owns this; Phase 3 fans out + unions + calls it (D-08) |
| Bounded concurrency / backpressure | Crawl driver (worker pool / semaphore) | tokio runtime | App-level concurrency budget (CRAWL-04); Claude's discretion on mechanism |
| Retry / terminal-status policy | Crawl driver + store (`set_fetch_status`, `fetch_attempts`) | Database | Bounded in-crawl retry then terminal `failed`/`not_found` (D-09/D-10/D-11) |

## User Constraints (from CONTEXT.md)

### Locked Decisions
- **D-01 — Frontier is status-driven on existing `pubkeys` table, no separate frontier table.** The frontier *is* the set of `pubkeys.status = 'discovered'` rows, served by the existing `pubkeys_status_idx` partial index. One source of truth.
- **D-02 — Traversal is order-agnostic.** Reach every reachable pubkey exactly once; order does not matter. Claim any `discovered` row (e.g. by `id`). No discovery-sequence / depth column. CRAWL-02 reachability gate holds structurally regardless of order.
- **D-03 — Anchor pubkey is the only seed.** Pre-insert the configurable anchor as a `discovered` row; BFS expands purely by applying fetched follow lists (every followee gets `upsert_pubkey`'d, landing as `discovered` unless already known).
- **D-04 — Claiming uses a persisted lease.** A claim flips `discovered → in_progress` and stamps `claimed_at`, in its own short transaction using `SELECT ... FOR UPDATE SKIP LOCKED` to prevent two workers claiming the same row. The DB row lock is **not** held for the duration of the multi-second multi-relay fetch — only for the claim flip.
- **D-05 — In-flight work is re-fetched after a crash (at-least-once).** Safe because `apply_follow_list` is idempotent on an unchanged event id. CRAWL-03's guarantee is about not refetching *completed* (`fetched`) work, not exactly-once for in-flight.
- **D-06 — Stale `in_progress` rows are reclaimed to `discovered` by a startup-only sweep.** A clean shutdown leaves none; a crash leaves some. Fully satisfies CRAWL-03 within Phase 3 scope. Continuous in-run reclaim deferred to Phase 4.
- **D-07 — A worker batch-claims N authors at once** and fetches them as one author-chunked request set (reusing `acquire_validated_lists_client` author-chunking). N is a parameter (config knob in Phase 4). Minimizes relay round-trips. Partially-failing batch resolves status per author.
- **D-08 — For a claimed batch, fan out to all curated relays → collect the raw event union → run `ingest::ingest_events` once.** Newest-wins across relays, resilient to any single relay missing/lagging. Ingest gate already cross-relay dedups by event id and resolves the replaceable winner over the full union, so unioning before ingest introduces no double-processing. Per-relay rate limiting already applies inside the fetch path.
- **D-09 — Bounded in-crawl retry for transient errors, then a recorded terminal status.** On a transient fetch error, return the author to `discovered` and bump a per-pubkey `fetch_attempts` counter. After a configurable max attempts (default ~3), set status `failed`.
- **D-10 — `not_found`** (relays answered but no kind-3 exists) is terminal for this phase and is the target for Phase 5 NIP-65 fallback (RELAY-05). `failed` (errors exhausted retries) is the errored-out terminal state.
- **D-11 — Both `failed` and `not_found` are recorded** (status + `last_fetched_at` stamped) so the Phase 4 staleness loop can re-enqueue them. The existing `pubkeys_status_idx` partial index already targets `('discovered','not_found','failed')`.
- **D-12 — This phase ships one additive migration** extending the freshness model: add `'in_progress'` to the `pubkeys.status` CHECK constraint domain; add `claimed_at TIMESTAMPTZ` (lease timestamp, internal); add `fetch_attempts` (small int, internal retry counter). Internal bookkeeping columns — keep out of contract views (Phase 1 D-11). Update `COMMENT ON` to label INTERNAL. Lean toward NOT exposing transient `in_progress` in `pubkey_freshness`. Follows Phase 1 D-13: frontier support arrives as its own additive migration.

### Claude's Discretion
- Worker-pool / concurrency mechanism (fixed task pool pulling from the DB queue vs. a semaphore over a spawn loop), exact bounded-concurrency implementation for CRAWL-04, batch size N default, retry-cap default, claim batch SQL, the `in_progress` exposure choice in `pubkey_freshness`, and GRAPH-02 verification test construction (real validated events through the wired seam) — all planner/researcher decisions within the locked stack (sqlx 0.9, Postgres 16/17, `FOR UPDATE SKIP LOCKED`). The startup reclaim sweep's "stale" definition is effectively "any `in_progress` at startup" (D-06), so no age threshold is needed for the Phase 3 startup case.

### Deferred Ideas (OUT OF SCOPE)
- **Continuous (in-run) stale-lease reclaim sweep** — Phase 4 daemon loop (D-06). Phase 3 does startup-only reclaim.
- **Config-file sourcing of anchor / batch size N / concurrency cap / retry cap** — OPS-01, Phase 4. Phase 3 takes them as parameters with defaults.
- **NIP-65 fallback for `not_found` pubkeys** — Phase 5 (RELAY-05); Phase 3 only records the terminal `not_found` status.
- **Staleness/TTL re-enqueue of `failed`/`not_found`** — Phase 4 (FRESH-02); Phase 3 records the statuses the scanner will consume.

## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| GRAPH-02 | A replacing kind-3 is applied as a transactional edge diff (insert added, delete removed); an unchanged list (same event id) touches zero edge rows | `apply_follow_list` already implements this (idempotency short-circuit on `applied_event_id`, Rust-side set diff, DELETE+INSERT+freshness UPDATE in one `pool.begin()`/`commit()`). Phase 3 wires the followee-pubkey→id resolution seam and verifies it against real `ValidatedFollowList` events. See "Pattern 1" and "Validation Architecture". |
| CRAWL-01 | Crawl starts from a single configurable anchor pubkey and discovers pubkeys via BFS over follow edges | Pre-insert anchor as `discovered` (D-03); BFS = claim `discovered` rows, fetch, apply follow lists, which upserts new followees as `discovered`. See "Pattern 2". |
| CRAWL-02 | Only pubkeys followed by someone already in the graph are enqueued — spam islands never crawled | Holds structurally: a pubkey is only inserted (as `discovered`) because it appeared as a followee in a fetched list (D-02). No explicit gate needed; the gate is the data-flow. See "Pattern 2" + "Pitfall 4". |
| CRAWL-03 | Frontier is DB-resident; after crash/restart the crawler resumes without refetching completed work | Frontier lives in `pubkeys.status` (D-01). `fetched`/`not_found`/`failed` rows are never re-claimed (claim query selects only `discovered`). Startup sweep resets orphaned `in_progress → discovered` (D-06). See "Pattern 3" + "Validation Architecture". |
| CRAWL-04 | In-flight fetch concurrency is bounded end-to-end (backpressure; no unbounded queues/memory) | Bounded worker pool / semaphore over the claim→fetch→apply cycle; workers pull from the DB queue rather than an in-memory queue, so the "queue" is the DB and cannot grow in process memory. See "Pattern 4". |
| FRESH-01 | Every pubkey records when its follow-list knowledge was last acquired or confirmed | Already wired: `apply_follow_list` stamps `last_fetched_at`/`last_confirmed_at`; `set_fetch_status` stamps `last_fetched_at` on terminal `not_found`/`failed`. Phase 3 ensures every terminal path stamps it. See "Pitfall 5". |

## Standard Stack

No new external dependencies. Every capability is in the locked stack already in `Cargo.toml`.

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| sqlx | 0.9.0 | Postgres driver, compile-time-checked queries, connection pool, transactions (`pool.begin()`/`tx.commit()`) | Locked (CLAUDE.md). The claim transaction, batch claim, and edge-diff all use `query!`/`query_scalar!` macros against the live/offline schema. `.sqlx/` offline metadata convention already established (Phase 1). [VERIFIED: Cargo.toml] |
| tokio | 1.52 | Async runtime; bounded concurrency via `Semaphore`, fixed task pool via `JoinSet`/spawn | Locked. `tokio::sync::Semaphore` is the standard backpressure primitive for CRAWL-04. [VERIFIED: Cargo.toml] |
| nostr-sdk | 0.44 | Relay pool + event types behind the existing acquire path | Locked. Phase 3 calls `relay::acquire_validated_lists_client`; does not touch nostr-sdk directly. [VERIFIED: Cargo.toml] |
| chrono | 0.4 | `DateTime<Utc>` in store signatures and timestamps | Locked. `apply_follow_list`/`set_fetch_status` already take `DateTime<Utc>`. [VERIFIED: Cargo.toml] |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| tokio (`sync::Semaphore`) | 1.52 | Permit-based concurrency cap | Bound in-flight batch fetches (CRAWL-04). One permit per in-flight batch worker. |
| tokio (`task::JoinSet`) | 1.52 | Fixed worker pool / structured spawn | Alternative CRAWL-04 mechanism: spawn a fixed set of worker tasks that loop claim→fetch→apply. Claude's discretion between this and Semaphore-over-spawn-loop. |
| anyhow / thiserror | 1.0 / 2.0 | Error plumbing | Crawl-driver error type; reuse `StoreError`/`RelayError` at module boundaries. [VERIFIED: Cargo.toml] |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| `pubkeys.status` as frontier (D-01) | Dedicated `frontier`/`queue` table | Rejected by D-01: a second structure must be kept consistent with graph state. The status column already exists, is indexed (`pubkeys_status_idx`), and is the single source of truth. |
| `FOR UPDATE SKIP LOCKED` claim (D-04) | Advisory locks; `SELECT ... FOR UPDATE` (blocking) | SKIP LOCKED is the canonical contention-free queue-claim primitive (Postgres 9.5+); blocking `FOR UPDATE` serializes workers and kills throughput. Advisory locks add a parallel lock namespace to reason about. [VERIFIED: postgresql.org docs] |
| In-memory BFS visited-set + queue | DB-resident frontier (D-01/CRAWL-03) | CLAUDE.md notes hand-rolled in-memory BFS is "often leaner at these scales" — but CRAWL-03 *requires* crash survival, which an in-memory frontier cannot provide. The DB-resident frontier is mandated by the requirement, not chosen for performance. |
| Recursive-CTE BFS in Postgres | In-memory/driver-side claim loop | Explicitly on CLAUDE.md "What NOT to Use": Postgres recursive executor can't hold visited-state efficiently (seconds-to-minutes at scale). Phase 3's loop is driver-side; the DB only does indexed claims + adjacency writes. |

**Installation:** None — no new crates. Phase 3 adds one SQL migration and Rust modules using existing dependencies.

**Version verification:** All versions confirmed against the committed `Cargo.toml` [VERIFIED: Cargo.toml on disk]. No new packages introduced, so no registry lookups are required for this phase.

## Package Legitimacy Audit

> Not applicable — Phase 3 installs **no external packages**. All functionality uses crates already present in `Cargo.toml` (sqlx 0.9.0, tokio 1.52, nostr-sdk 0.44, chrono 0.4, anyhow, thiserror), each verified and in use since Phases 1–2.

**Packages removed due to [SLOP] verdict:** none
**Packages flagged as suspicious [SUS]:** none

## Architecture Patterns

### System Architecture Diagram

```
                          ┌─────────────────────────────────────────────┐
   configurable anchor    │              CRAWL DRIVER (new)               │
   pubkey (param, D-03)   │                                               │
        │                 │  ┌────────────────────────────────────────┐  │
        ▼                 │  │ STARTUP: seed anchor as 'discovered'    │  │
   upsert_pubkey ─────────┼─▶│         + reclaim sweep                 │  │
   (status='discovered')  │  │   UPDATE pubkeys SET status='discovered'│  │
                          │  │   WHERE status='in_progress' (D-06)     │  │
                          │  └────────────────────────────────────────┘  │
                          │                    │                          │
                          │                    ▼                          │
   ┌──────────────────────┼──────  bounded worker pool (CRAWL-04)  ───────┼──┐
   │ (Semaphore / JoinSet, N permits — Claude's discretion)               │  │
   │                      │                    │                          │  │
   │  per worker loop:    │                    ▼                          │  │
   │  ┌───────────────────┼───────────────────────────────────────────┐  │  │
   │  │ 1. CLAIM batch (own short txn, D-04):                          │  │  │
   │  │    SELECT id,pubkey FROM pubkeys WHERE status='discovered'     │  │  │
   │  │    ORDER BY id LIMIT N FOR UPDATE SKIP LOCKED                  │──┼──┼──┼──▶ pubkeys
   │  │    → UPDATE those ids SET status='in_progress',claimed_at=now()│  │  │     (status
   │  │    (lock released at commit — NOT held during fetch)          │  │  │      = queue)
   │  └───────────────────────────────────────────────────────────────┘  │  │
   │                      │                    │ batch of authors         │  │
   │                      │                    ▼                          │  │
   │  ┌───────────────────┼───────────────────────────────────────────┐  │  │
   │  │ 2. FAN OUT to all curated relays (D-08):                      │  │  │
   │  │    for relay in curated: acquire_validated_lists_client(...)  │──┼──┼──┼──▶ relays
   │  │    (per-relay rate limit + pagination already inside, Ph2)    │  │  │   (the wild)
   │  │    → collect RAW event UNION across relays                    │  │  │
   │  └───────────────────────────────────────────────────────────────┘  │  │
   │                      │                    │ raw event union          │  │
   │                      │                    ▼                          │  │
   │  ┌───────────────────┼───────────────────────────────────────────┐  │  │
   │  │ 3. ingest_events(union) ONCE → Vec<ValidatedFollowList>       │  │  │
   │  │    (verify + cross-relay dedup + newest-wins, Phase 2)        │  │  │
   │  └───────────────────────────────────────────────────────────────┘  │  │
   │                      │                    │                          │  │
   │                      │      ┌─────────────┴──────────────┐           │  │
   │                      │      ▼                            ▼           │  │
   │  ┌───────────────────┼──────────────┐   ┌────────────────────────┐  │  │
   │  │ 4a. PER list got:  │             │   │ 4b. author NOT in result│  │  │
   │  │  upsert follower id │             │   │   → 'not_found' (D-10)  │  │  │
   │  │  upsert each followee (DISCOVERY)─┼───┼──▶ new followees land    │──┼──┼──▶ pubkeys
   │  │  apply_follow_list(GRAPH-02) ─────┼───┼──  as 'discovered'       │  │  │   (+follows
   │  │  → status='fetched' (terminal)    │   │   (CRAWL-02 reachability)│  │  │    edges)
   │  └───────────────────┼──────────────┘   └────────────────────────┘  │  │
   │                      │            transient err → status back to      │  │
   │                      │            'discovered', fetch_attempts+1;      │  │
   │                      │            >max → 'failed' (D-09/D-11)          │  │
   └──────────────────────┼─────────────────────────────────────────────────┘
                          │   loop until no 'discovered' rows remain
                          └─────────────────────────────────────────────┘
```

The "queue" never lives in process memory — it is `pubkeys.status`. New discoveries (followees) flow back into the same table as `discovered`, so the loop terminates exactly when the reachable component is exhausted (CRAWL-01/02). A crash at any point leaves at most the current batch as `in_progress`, which the next startup resets (CRAWL-03).

### Recommended Project Structure
```
src/
├── crawl/                 # NEW — the crawl driver (this phase)
│   ├── mod.rs             #   crawl orchestration entry + bounded worker pool (CRAWL-01/04)
│   ├── frontier.rs        #   claim_batch (FOR UPDATE SKIP LOCKED), reclaim_stale, seed_anchor (D-01/D-04/D-06)
│   └── apply.rs           #   the acquire→union→ingest→upsert→apply_follow_list seam per batch (GRAPH-02 wiring)
├── store/
│   ├── follows.rs         # EXISTING — apply_follow_list (GRAPH-02 writer; unchanged, verified)
│   └── pubkeys.rs         # EXISTING — upsert_pubkey, set_fetch_status; MAY add fetch_attempts helpers
├── relay/                 # EXISTING — acquire_validated_lists_client (Phase 2), unchanged
└── ingest/                # EXISTING — ingest_events, ValidatedFollowList (Phase 2), unchanged
migrations/
├── 0001_graph_schema.sql  # EXISTING
└── 0002_frontier.sql      # NEW — additive: 'in_progress' status, claimed_at, fetch_attempts (D-12)
tests/
├── edge_diff.rs           # EXISTING (Phase 1 GRAPH-02 unit-level)
├── graph_writer.rs        # NEW — GRAPH-02 verified against real ValidatedFollowList through the seam
├── frontier.rs            # NEW — claim/lease + reachability + crash-resume + concurrency
```

The exact module split is Claude's discretion; the constraint is that `apply_follow_list`, `ingest_events`, and `acquire_validated_lists_client` are **consumed, not modified**.

### Pattern 1: Edge-diff verification through the wired seam (GRAPH-02)
**What:** Verify that applying a `ValidatedFollowList` produces the correct add/remove edge delta in one transaction, and that re-applying the same event id touches zero edge rows.
**When to use:** This is the GRAPH-02 verification (success criterion 1). The writer already exists; the new work is the followee-pubkey → id resolution seam plus a test using **real validated events** (not synthetic id lists).

```rust
// Source: CONTEXT D-15/D-16 + src/store/follows.rs (existing writer) + src/ingest/mod.rs (ValidatedFollowList)
// The seam Phase 3 wires (the bridge from a ValidatedFollowList to the existing writer):
async fn apply_validated(pool: &PgPool, vfl: &ValidatedFollowList) -> Result<bool, StoreError> {
    let follower_id = upsert_pubkey(pool, &vfl.follower_pubkey.to_bytes()).await?;
    let mut followee_ids = Vec::with_capacity(vfl.followee_pubkeys.len());
    for fp in &vfl.followee_pubkeys {
        // upsert_pubkey is the DISCOVERY/enqueue mechanism (D-03): a new followee
        // lands as 'discovered' here — this is what makes CRAWL-02 structural.
        followee_ids.push(upsert_pubkey(pool, &fp.to_bytes()).await?);
    }
    // apply_follow_list already does: idempotency short-circuit on event_id,
    // self-follow drop, Rust-side diff, DELETE+INSERT+freshness UPDATE in ONE txn.
    apply_follow_list(pool, follower_id, vfl.event_id.as_bytes(), vfl.created_at, &followee_ids).await
}
```

**Verification property:** apply list `{A,B,C}` for follower F (event e1) → 3 edges inserted; apply `{A,C,D}` (event e2) → B deleted, D inserted, A/C untouched, net 3 edges; re-apply e2 → `Ok(false)`, zero edge rows changed (assert `follows` row count and a row-level `xmin`/count check). [CITED: src/store/follows.rs lines 47-71, 93-141]

### Pattern 2: Status-as-frontier BFS with structural reachability (CRAWL-01/02)
**What:** The BFS frontier is the `discovered` row-set; discovery is a side effect of `upsert_pubkey` on followees; reachability is enforced by the data-flow, not a query.
**When to use:** The whole crawl loop. There is no separate "is this pubkey reachable?" check — a pubkey is only ever inserted because someone already crawled followed it (anchor-seeded).

```rust
// Source: CONTEXT D-01/D-02/D-03 + src/store/pubkeys.rs
// Seed (D-03): the anchor is the only externally-inserted pubkey.
let anchor_id = upsert_pubkey(pool, &anchor_pubkey_bytes).await?; // lands 'discovered'
// Loop terminates when no 'discovered' rows remain. New followees discovered inside
// apply_validated() re-populate 'discovered', so the loop naturally expands the
// reachable component and stops at its boundary. NO depth column, NO ordering needed (D-02).
```

**Anti-pattern avoided:** an explicit `WHERE reachable_from_anchor` predicate or a recursive CTE. Reachability is an *invariant of how rows are inserted*, not a runtime filter. [CITED: CONTEXT D-02]

### Pattern 3: Batch claim with `FOR UPDATE SKIP LOCKED` + persisted lease (D-04/D-07)
**What:** Atomically claim N `discovered` rows, flip them to `in_progress`, stamp `claimed_at`, and release the lock immediately — the long fetch happens afterward with no lock held.
**When to use:** Each worker iteration, before fetching a batch.

```sql
-- Source: PostgreSQL docs (FOR UPDATE SKIP LOCKED) + standard PG job-queue pattern.
-- Batch claim N authors in one short transaction (D-04/D-07). CTE form gives LIMIT + RETURNING.
WITH claimed AS (
    SELECT id
    FROM pubkeys
    WHERE status = 'discovered'
    ORDER BY id                      -- D-02 order-agnostic; ORDER BY id makes it deterministic + index-friendly
    LIMIT $1                         -- batch size N (D-07)
    FOR UPDATE SKIP LOCKED           -- never block on a row another worker holds
)
UPDATE pubkeys p
SET status = 'in_progress', claimed_at = now()
FROM claimed
WHERE p.id = claimed.id
RETURNING p.id, p.pubkey;            -- driver gets ids + 32-byte keys to fetch
```

This whole statement is one implicit transaction (or wrap in `pool.begin()`/`commit()` to be explicit). The row lock from `FOR UPDATE` is released at commit — **immediately**, not during the multi-second relay fetch (D-04). Two concurrent workers running this never claim the same row (SKIP LOCKED), and never block each other. [VERIFIED: postgresql.org SELECT/UPDATE docs + standard queue pattern]

**Note on indexing:** the existing `pubkeys_status_idx` is partial on `('discovered','not_found','failed')` — it covers the `WHERE status='discovered'` claim scan. It does **not** include `in_progress` (Claude's discretion per CONTEXT code_context); since the claim query only reads `discovered` and the startup sweep does a one-time full scan for `in_progress`, an `in_progress` index is likely unnecessary — confirm during planning. [CITED: migrations/0001_graph_schema.sql line 44]

### Pattern 4: Bounded worker pool (CRAWL-04)
**What:** Cap the number of in-flight batch fetches so neither memory nor relay load grows unbounded; workers pull from the DB queue (not an in-memory channel), so backpressure is automatic.
**When to use:** The crawl orchestration entry point.

```rust
// Source: tokio Semaphore (standard backpressure primitive) + CONTEXT D-07/CRAWL-04.
// Option A — semaphore over a spawn loop:
let sem = Arc::new(Semaphore::new(concurrency)); // CRAWL-04 cap (param, default Claude's discretion)
loop {
    let batch = claim_batch(&pool, batch_size).await?;   // Pattern 3
    if batch.is_empty() { break; }                       // frontier drained → done
    let permit = sem.clone().acquire_owned().await?;     // blocks here when at cap → backpressure
    tokio::spawn(process_batch(pool.clone(), batch, permit /* dropped on completion */));
}
// Option B — fixed JoinSet of N workers each looping claim→fetch→apply.
// Either satisfies CRAWL-04; choice is Claude's discretion (CONTEXT).
```

Because the queue is `pubkeys.status` in the DB, the in-process footprint is bounded by `concurrency × batch_size` authors in flight — there is no unbounded in-memory queue. [CITED: CONTEXT CRAWL-04 / D-07]

### Pattern 5: Fan-out, union, single ingest pass (D-08)
**What:** For a claimed batch, query *every* curated relay, union the raw events, then run `ingest_events` once over the whole union.
**When to use:** Inside `process_batch`, after claiming, before applying.

```rust
// Source: CONTEXT D-08 + src/relay/mod.rs (acquire_validated_lists_client) + src/ingest/mod.rs.
// Fan out per relay (each call already rate-limits + paginates internally, Phase 2):
let mut all_lists: Vec<ValidatedFollowList> = Vec::new();
// NOTE: acquire_validated_lists_client already runs ingest_events per-relay. Per D-08 the
// intent is a single resolution pass over the cross-relay UNION so newest-wins resolves
// across relays. The planner must decide the exact composition: either (a) expose a
// raw-union fetch that collects events across relays then calls ingest_events ONCE, or
// (b) rely on ingest_events' idempotency since it already cross-relay dedups by event id.
// Phase 2's acquire_validated_lists is generic over an injected fetch source, which makes
// (a) straightforward to wire. RESOLVE during planning — see Open Question 1.
```

The ingest gate cross-relay dedups by event id and resolves the replaceable winner over whatever union it sees, so unioning before ingest introduces no double-processing (D-08). [CITED: CONTEXT D-08; src/ingest/mod.rs lines 120-147]

### Anti-Patterns to Avoid
- **Separate frontier/queue table.** D-01 explicitly rejects this — it would need to be kept consistent with graph state. Use `pubkeys.status`.
- **Recursive-CTE BFS in Postgres.** On CLAUDE.md "What NOT to Use" — the recursive executor can't hold visited-state efficiently. BFS is driver-side.
- **Holding the claim row lock during the fetch.** D-04 forbids it — the lock is for the flip only; a multi-second fetch under a row lock starves other workers and risks long-transaction bloat.
- **Exactly-once claiming machinery.** D-05 deliberately accepts at-least-once for in-flight work, relying on `apply_follow_list` idempotency. Do not build dedup/once-only delivery.
- **In-run reclaim sweep.** Deferred to Phase 4 (D-06). Phase 3 reclaims only at startup.
- **Depth / discovery-order columns.** D-02 forbids — traversal is order-agnostic.
- **Modifying `apply_follow_list` / `ingest_events` / `acquire_validated_lists_client`.** These are consumed and verified, not rebuilt (CONTEXT domain note + D-15/D-16).
- **Re-fetching `fetched` rows in the same crawl.** The claim query must select only `discovered` — never `fetched`. This is the core of CRAWL-03's no-redo guarantee.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Concurrency-safe dequeue | Custom row-locking / advisory-lock scheme | `SELECT ... FOR UPDATE SKIP LOCKED` (D-04) | Battle-tested Postgres primitive; atomic, contention-free, race-free. Used by Solid Queue, pg-boss, etc. [VERIFIED: postgresql.org] |
| Transactional edge diff | Re-implementing add/remove + atomicity | Existing `store::follows::apply_follow_list` | Already built, tested, idempotent, atomic (Phase 1). Phase 3 consumes it. |
| Replaceable-event newest-wins / dedup | Re-resolving winners in the crawl driver | Existing `ingest::ingest_events` | Already does verify + cross-relay dedup + newest-wins + bounds (Phase 2). |
| Pubkey → surrogate id + discovery | SELECT-then-INSERT race | Existing `store::pubkeys::upsert_pubkey` | One-round-trip `ON CONFLICT ... RETURNING`; upsert IS the enqueue (D-03). |
| Bounded concurrency / backpressure | Custom counters / unbounded channels | `tokio::sync::Semaphore` or `JoinSet` | Standard tokio primitives; permit acquisition provides natural backpressure (CRAWL-04). |
| Crash-safe queue persistence | Write-ahead log / checkpoint file | `pubkeys.status` in Postgres + startup sweep | The DB is already durable; the frontier riding `status` survives crashes for free (CRAWL-03). |

**Key insight:** Almost everything hard in this phase is already built and tested in Phases 1–2. The genuinely new code is small: a migration, a claim/sweep/seed module, a bounded worker loop, and the per-batch composition seam. The risk is *re-building* rather than *wiring* — the planner must frame tasks as "consume and verify X" not "implement X".

## Runtime State Inventory

> Phase 3 is not a rename/refactor phase, but it ships a schema migration touching a column whose values drive runtime behavior. The relevant question: does the additive migration introduce a new status value that any existing code/contract assumes is absent?

| Category | Items Found | Action Required |
|----------|-------------|------------------|
| Stored data | `pubkeys.status` gains a new legal value `'in_progress'`; existing rows are all in `{discovered, fetched, not_found, failed}` and are unaffected by adding the value to the CHECK domain. New columns `claimed_at`/`fetch_attempts` default NULL/0 on existing rows. | Additive migration only; no data migration. `ALTER TABLE ... DROP CONSTRAINT ... ADD CONSTRAINT` to widen the CHECK (Postgres has no in-place CHECK edit) — see Pitfall 1. |
| Live service config | None — no live service stores this string (single-operator daemon not yet running; Phase 4 builds the daemon). | None — verified: no daemon, no external service references `status` values. |
| OS-registered state | None. | None — verified: no OS registration in this codebase. |
| Secrets/env vars | None — `DATABASE_URL` is the only secret and is unchanged. | None. |
| Build artifacts | `.sqlx/` offline query metadata must be regenerated after the migration so the new claim/sweep/upsert queries type-check offline in CI (`cargo sqlx prepare`). Existing `.sqlx/` files for unchanged queries are still valid. | Run `cargo sqlx prepare` against a DB migrated to 0002 and commit the new `.sqlx/` files (Phase 1 convention, Pitfall 2). |
| Contract views | `pubkey_freshness` exposes `status`. If `in_progress` is exposed, downstream consumers (spam layer) would see a transient value not in the documented domain. D-12 leans toward NOT exposing it. | Decide `in_progress` exposure (D-12 / Open Question 2); if hidden, `pubkey_freshness` must filter or collapse it, and update `COMMENT ON` + SCHEMA.md status domain. |

## Common Pitfalls

### Pitfall 1: Widening a CHECK constraint is not idempotent by default
**What goes wrong:** Postgres cannot edit a CHECK constraint in place. The migration must `DROP` the old constraint and `ADD` the new one. A naive `ADD CONSTRAINT` fails on re-run (constraint already exists), breaking the Phase 1 idempotency convention.
**Why it happens:** The Phase 1 migration used an inline unnamed CHECK on `status`. Unnamed constraints get auto-generated names (e.g. `pubkeys_status_check`), which must be referenced to drop.
**How to avoid:** Use a guarded `DO $$ ... $$` block or `ALTER TABLE ... DROP CONSTRAINT IF EXISTS <name>` then `ADD CONSTRAINT <name> CHECK (...)` with an explicit name, plus `ADD COLUMN IF NOT EXISTS` for `claimed_at`/`fetch_attempts`. Test that re-running 0002 is a no-op (Phase 1 migration test convention).
**Warning signs:** Migration test `migrations.rs` fails on the second run; `_sqlx_migrations` shows the version applied but a manual re-run errors.

### Pitfall 2: Stale `.sqlx/` offline metadata breaks CI after new queries
**What goes wrong:** New `query!`/`query_scalar!` calls (claim, sweep, seed, fetch_attempts bump) have no committed `.sqlx/` entry, so `SQLX_OFFLINE=true cargo build` fails in CI.
**Why it happens:** sqlx compile-time checks need either a live `DATABASE_URL` or committed offline metadata; CI uses offline (Phase 1 established this).
**How to avoid:** After writing the migration and all new queries, run `cargo sqlx prepare -- --all-targets` against a DB migrated to 0002, and commit the new `.sqlx/*.json`. [CITED: 01-03-SUMMARY.md lines 132-134]
**Warning signs:** Green locally (live DB) but red in offline build.

### Pitfall 3: Re-claiming `fetched` rows (CRAWL-03 violation)
**What goes wrong:** A too-broad claim query (e.g. `WHERE status != 'in_progress'`) picks up already-completed `fetched` rows, re-fetching completed work after a restart — directly violating CRAWL-03 success criterion 3.
**Why it happens:** Conflating "needs work" with "not currently claimed."
**How to avoid:** The Phase 3 claim query selects **only** `status = 'discovered'`. `fetched`/`not_found`/`failed` are terminal for this phase (Phase 4's staleness loop re-enqueues `not_found`/`failed` later, not Phase 3). The crash-resume test must assert a `fetched` pubkey is never re-fetched after a kill/restart.
**Warning signs:** `fetch_count` on a completed pubkey increments after a restart with no new TTL trigger.

### Pitfall 4: Mistaking reachability-gating for a runtime check
**What goes wrong:** A planner adds a `WHERE reachable` predicate or a recursive CTE to "enforce CRAWL-02," reintroducing the exact anti-pattern CLAUDE.md forbids and that D-02 designed away.
**Why it happens:** CRAWL-02 reads like a filter ("only enqueue reachable pubkeys").
**How to avoid:** Reachability is **structural** (D-02): a pubkey row exists only because it was a followee in a list fetched from someone already in the graph, seeded by the anchor. The "gate" is `upsert_pubkey`-on-followee being the *only* insertion path besides the anchor seed. No query enforces it. The test for CRAWL-02 inserts an isolated "spam island" pubkey that nobody in the reachable set follows and asserts it is never claimed/fetched.
**Warning signs:** A `WITH RECURSIVE` or `reachable` column appears in the plan.

### Pitfall 5: A terminal status path that forgets to stamp `last_fetched_at` (FRESH-01)
**What goes wrong:** The `not_found` or `failed` terminal transition sets `status` but not `last_fetched_at`, leaving knowledge-age unrecorded and breaking FRESH-01 + the Phase 4 staleness scan's age comparison.
**Why it happens:** `apply_follow_list` stamps `last_fetched_at` on success, but `not_found`/`failed` go through `set_fetch_status`, which *does* stamp `last_fetched_at` — as long as the driver calls it with a timestamp. A direct `UPDATE status=...` that bypasses `set_fetch_status` would miss it.
**How to avoid:** Route every terminal transition through `set_fetch_status(pool, id, status, now)` (it stamps `last_fetched_at`), or ensure any custom UPDATE also sets it. [CITED: src/store/pubkeys.rs lines 54-70]
**Warning signs:** A `not_found`/`failed` row with NULL `last_fetched_at`.

### Pitfall 6: Long-held transaction from doing fetch work inside the claim transaction
**What goes wrong:** Wrapping the claim + the multi-second relay fetch in one transaction holds row locks and bloats Postgres (long-running transaction prevents vacuum, holds locks, starves other workers).
**Why it happens:** Tempting to "claim and process atomically."
**How to avoid:** D-04 mandates the claim is its **own short transaction**; the fetch runs lock-free afterward. The status flip to `fetched`/`failed`/`discovered`-on-retry is a separate short write. Crash safety comes from idempotency (D-05), not from a long transaction.
**Warning signs:** `pg_stat_activity` shows transactions open for seconds; `idle in transaction` during fetches.

### Pitfall 7: `fetch_attempts` retry counter never resets / unbounded re-queue loop
**What goes wrong:** A pubkey that always errors transiently bounces `discovered ↔ in_progress` forever if `fetch_attempts` isn't checked, hammering a flaky relay.
**Why it happens:** Missing the max-attempts terminal transition (D-09).
**How to avoid:** On transient error, bump `fetch_attempts` and return to `discovered` **only if** `fetch_attempts < max` (default ~3, param); otherwise set `failed` (D-09). The claim itself naturally interleaves retries (the row goes to the back of the `ORDER BY id` queue behavior is moot — it just becomes claimable again). [CITED: CONTEXT D-09]
**Warning signs:** A single pubkey's `fetch_attempts` climbing without bound; the crawl never terminating.

## Code Examples

### Additive frontier migration (D-12)
```sql
-- Source: CONTEXT D-12 + migrations/0001_graph_schema.sql convention (idempotent additive).
-- migrations/0002_frontier.sql
-- Widen the status CHECK domain to include 'in_progress' (DROP + ADD, named; Pitfall 1).
ALTER TABLE pubkeys DROP CONSTRAINT IF EXISTS pubkeys_status_check;
ALTER TABLE pubkeys ADD CONSTRAINT pubkeys_status_check
    CHECK (status IN ('discovered','in_progress','fetched','not_found','failed'));

-- Internal bookkeeping columns (D-12), hidden from contract views (Phase 1 D-11).
ALTER TABLE pubkeys ADD COLUMN IF NOT EXISTS claimed_at     TIMESTAMPTZ;
ALTER TABLE pubkeys ADD COLUMN IF NOT EXISTS fetch_attempts SMALLINT NOT NULL DEFAULT 0;

COMMENT ON COLUMN pubkeys.claimed_at IS
    'INTERNAL: lease timestamp — when a worker claimed this row (status=in_progress). Reset on startup reclaim (D-06). Not part of the public contract.';
COMMENT ON COLUMN pubkeys.fetch_attempts IS
    'INTERNAL: per-pubkey transient-error retry counter; >= max attempts transitions to failed (D-09). Not part of the public contract.';

-- pubkey_freshness exposure of 'in_progress' (D-12 / Open Question 2): lean toward NOT
-- exposing transient in_progress. If hidden, redefine the view to collapse/filter it, e.g.:
-- CREATE OR REPLACE VIEW pubkey_freshness AS
--   SELECT id,
--          CASE WHEN status = 'in_progress' THEN 'discovered' ELSE status END AS status,
--          last_fetched_at
--   FROM pubkeys;
-- (Decide during planning; update COMMENT ON + SCHEMA.md to match.)
```

### Startup reclaim sweep (D-06, CRAWL-03)
```rust
// Source: CONTEXT D-06. Startup-only: any in_progress at startup is a crash orphan.
async fn reclaim_stale_on_startup(pool: &PgPool) -> Result<u64, StoreError> {
    // No age threshold needed for the Phase 3 startup case (CONTEXT discretion note):
    // a clean shutdown leaves zero in_progress; whatever remains is a crash orphan.
    let rows = sqlx::query!(
        "UPDATE pubkeys SET status = 'discovered', claimed_at = NULL \
         WHERE status = 'in_progress'"
    )
    .execute(pool)
    .await?;
    Ok(rows.rows_affected())
}
```

### Per-author batch result resolution (D-07/D-09/D-10)
```rust
// Source: CONTEXT D-07/D-09/D-10. After ingest, resolve status PER author in the batch.
for claimed in &batch {
    match validated_for.get(&claimed.id) {
        Some(vfl) => { apply_validated(pool, vfl).await?; }            // -> 'fetched' (in apply_follow_list)
        None if relays_answered => set_fetch_status(pool, claimed.id, "not_found", now).await?, // D-10
        None /* transient error */ => {
            // bump attempts; back to discovered if under cap, else failed (D-09)
            requeue_or_fail(pool, claimed.id, max_attempts, now).await?;
        }
    }
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Polling queue with `SELECT ... FOR UPDATE` (blocking) | `SELECT ... FOR UPDATE SKIP LOCKED` | Postgres 9.5 (2016) | Contention-free multi-worker claims; the modern default for DB-backed queues (Solid Queue, pg-boss). [VERIFIED: postgresql.org] |
| Redis/SQS for job queues | Postgres-native queue via SKIP LOCKED | ongoing 2023–2026 | For a single-operator, already-Postgres system, a DB-resident queue removes an entire moving part — matches CLAUDE.md "favor simplicity." |

**Deprecated/outdated:** none relevant — the stack is current (sqlx 0.9, Postgres 17, nostr-sdk 0.44, all verified in CLAUDE.md on 2026-06-11).

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | The auto-generated CHECK constraint name on `pubkeys.status` is `pubkeys_status_check` (Postgres default naming). | Pitfall 1 / migration example | LOW — if the name differs, the `DROP CONSTRAINT IF EXISTS` no-ops and the migration fails on the duplicate ADD. Planner should verify the actual constraint name via `\d pubkeys` against a migrated DB before finalizing 0002. |
| A2 | `acquire_validated_lists_client` runs `ingest_events` once per relay call (per Phase 2 wiring), so achieving D-08's "single resolution pass over the cross-relay union" needs either a raw-union fetch path or reliance on ingest idempotency. | Pattern 5 / Open Question 1 | MEDIUM — affects how the fan-out seam is composed. The Phase 2 `acquire_validated_lists` generic-fetch-source design makes a union path feasible; resolve during planning. |
| A3 | An `in_progress` partial index is unnecessary (claim reads only `discovered`; sweep is a one-time startup scan). | Pattern 3 note | LOW — at most a one-time full-table scan at startup; if startup sweep latency matters at scale, add an index. Not correctness-affecting. |
| A4 | `fetch_attempts SMALLINT` is wide enough (max attempts default ~3). | migration example | LOW — SMALLINT max 32767 vastly exceeds any sane retry cap. |

**Note:** The architecture itself is **not** assumed — D-01..D-12 lock it. These assumptions are about implementation mechanics the planner should confirm.

## Open Questions

1. **D-08 cross-relay union composition.** Does the crawl driver (a) collect raw events across all curated relays into one union then call `ingest_events` once, or (b) call `acquire_validated_lists_client` per relay (which calls `ingest_events` per relay) and rely on the writer/ingest idempotency to converge on newest-wins?
   - What we know: `ingest_events` cross-relay dedups by event id and resolves the replaceable winner over whatever union it sees (D-08); Phase 2's `acquire_validated_lists` is generic over an injected fetch source, so a raw-union fetch is wireable without modifying ingest.
   - What's unclear: whether per-relay ingest + per-relay `apply_follow_list` calls could apply an older list then a newer one in sequence (still correct via newest-wins, but extra edge churn) vs. a single union pass that applies the winner once.
   - Recommendation: Prefer (a) — a raw-union-then-single-ingest path via the generic fetch seam — to honor D-08's "run `ingest::ingest_events` once" literally and avoid redundant edge writes. Confirm the exact `acquire_validated_lists` signature lets the driver collect a raw union before resolution.

2. **`in_progress` exposure in `pubkey_freshness` (D-12).** Expose the transient `in_progress` status to the contract, or collapse/hide it?
   - What we know: D-12 leans toward NOT exposing it (mid-flight churn, not a stable knowledge state); the contract documents the domain as `{discovered, fetched, not_found, failed}`.
   - Recommendation: Hide it — collapse `in_progress → discovered` in the view (a pubkey mid-fetch is, from the consumer's view, still "discovered but not yet fetched"). Update `COMMENT ON` and SCHEMA.md status domain accordingly so the contract stays truthful.

3. **Default values for the Phase 3 parameters** (batch size N, concurrency cap, max retry attempts). Phase 3 takes these as parameters (config-sourcing is Phase 4/OPS-01) but needs sane defaults.
   - Recommendation: pick conservative, relay-polite defaults (e.g. N≈50–100 authors/batch matching typical NIP-11 `max_limit`/author-chunk sizing, concurrency ≈ number of curated relays or a small multiple, max attempts = 3 per D-09). These are not load-bearing for Phase 3 correctness; the daemon will override them.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| PostgreSQL (server) | All persistence; tests via testcontainers | ✓ (via testcontainers, Phase 1) | 17 | — |
| Docker | testcontainers Postgres fixture in tests | ✓ (used Phases 1–2) | — | — |
| sqlx-cli | `cargo sqlx prepare` for offline metadata | ✓ (installed Phase 1) | 0.9.0 | live `DATABASE_URL` build |
| Rust toolchain | Build | ✓ | 1.94 (pinned) | — |
| Live nostr relays | crash-resume/E2E *may* use mock relay instead | n/a for unit tests | — | In-process scripted mock relay (Phase 2 `tests/mock_relay`) — preferred for deterministic frontier/crash tests |

**Missing dependencies with no fallback:** none.
**Missing dependencies with fallback:** Live relays — all Phase 3 verification can and should run against the Phase 2 in-process mock-relay fetch fn (deterministic, offline) rather than live relays; live-relay exercise is a Phase 4 daemon concern.

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in `#[tokio::test]` integration tests under `tests/` + `cargo test`; ephemeral Postgres via `testcontainers` 0.27 / `testcontainers-modules` (postgres) |
| Config file | none — Cargo + testcontainers fixture (`start_postgres()` from Phase 1 `tests/bootstrap`) |
| Quick run command | `SQLX_OFFLINE=true cargo test --test graph_writer` (single new suite) |
| Full suite command | `SQLX_OFFLINE=true cargo test` (all suites; testcontainers supplies the runtime DB) |

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| GRAPH-02 | Applying a replacing kind-3 (real `ValidatedFollowList`) inserts only added + deletes only removed edges in one txn; re-applying same event id touches zero edge rows | integration (Postgres) | `cargo test --test graph_writer apply_diff_adds_and_removes`, `...::same_event_zero_touch` | ❌ Wave 0 |
| CRAWL-01 | Crawl from a configured anchor discovers reachable pubkeys via BFS (anchor + multi-hop followees all end `fetched`) | integration (mock relay + Postgres) | `cargo test --test frontier bfs_reaches_full_component` | ❌ Wave 0 |
| CRAWL-02 | An isolated spam-island pubkey nobody reachable follows is never claimed/fetched | integration | `cargo test --test frontier spam_island_never_crawled` | ❌ Wave 0 |
| CRAWL-03 | Kill mid-crawl (orphan `in_progress`), restart → startup sweep resets orphans, `fetched` rows are never re-fetched | integration | `cargo test --test frontier crash_resume_no_redo`, `...::startup_reclaims_in_progress` | ❌ Wave 0 |
| CRAWL-04 | Concurrency is bounded: with cap=K, at most K batches are ever in flight; two workers never claim the same row (`FOR UPDATE SKIP LOCKED`) | integration (concurrent tasks, two pools) | `cargo test --test frontier bounded_concurrency`, `...::skip_locked_no_double_claim` | ❌ Wave 0 |
| FRESH-01 | Every fetched / not_found / failed pubkey has `last_fetched_at` stamped | integration | `cargo test --test frontier last_fetched_at_stamped_on_terminal` | ❌ Wave 0 |

**What must be proven (phase goal):**
- **Transactional correctness of edge diffs (GRAPH-02):** add/remove deltas applied atomically; idempotent on unchanged event id (zero edge rows). Test with *real* `ValidatedFollowList` values built from fixture events, driven through the wired `apply_validated` seam (not synthetic id arrays). Assert exact `follows` row deltas and that the unchanged re-apply returns `Ok(false)` and changes zero rows.
- **Crash-recovery of the frontier (CRAWL-03):** simulate a crash by leaving rows in `in_progress` (don't run a clean shutdown), then invoke the startup sweep and a second crawl pass; assert orphans return to `discovered` and become `fetched`, and that any pubkey already `fetched` before the "crash" is never re-fetched (assert `fetch_count` unchanged for those rows).
- **No-redo of completed work (CRAWL-03):** the claim query selects only `discovered`; assert a `fetched` row is invisible to `claim_batch`.
- **Newest-wins under concurrency (GRAPH-02/INGEST-03 boundary):** apply two events for the same follower (older then newer, and newer then older) possibly from concurrent workers; assert the newest (by created_at, lowest-id tie-break) wins and the edge set matches it. (`ingest_events` already owns resolution; the new surface to test is concurrent `apply_follow_list` calls converging correctly under MVCC — last-committed writer's resolved winner, with the idempotency short-circuit preventing redundant writes.)
- **Reachability-gating (CRAWL-02):** structural — prove the spam island is never inserted/claimed.
- **Bounded concurrency (CRAWL-04):** prove the in-flight count never exceeds the cap and SKIP LOCKED prevents double-claims.

### Sampling Rate
- **Per task commit:** `SQLX_OFFLINE=true cargo test --test graph_writer` (or `--test frontier` for queue tasks) — fast, targeted.
- **Per wave merge:** `SQLX_OFFLINE=true cargo test` (full suite; testcontainers Postgres).
- **Phase gate:** Full suite green + `SQLX_OFFLINE=true cargo build --all-targets` exit 0 (offline metadata committed) before `/gsd-verify-work`.

### Wave 0 Gaps
- [ ] `migrations/0002_frontier.sql` — the additive frontier migration (and a migration idempotency test extending `tests/migrations.rs`).
- [ ] `tests/graph_writer.rs` — GRAPH-02 verified against real `ValidatedFollowList` through the wired seam (success criterion 1).
- [ ] `tests/frontier.rs` — claim/lease (SKIP LOCKED no-double-claim), reachability (spam island), crash-resume (orphan reclaim + no-redo), bounded concurrency, terminal `last_fetched_at` stamping, retry/terminal-status transitions.
- [ ] Regenerated + committed `.sqlx/` metadata for all new queries (Pitfall 2) — required for offline CI build to stay green.
- [ ] Reuse Phase 2 `tests/mock_relay` scripted fetch fn for deterministic, offline frontier/crash tests (avoid live-relay flakiness).

## Sources

### Primary (HIGH confidence)
- `migrations/0001_graph_schema.sql` (read) — current schema, status CHECK domain, partial index, contract views, COMMENT ON convention.
- `src/store/follows.rs` (read) — `apply_follow_list` exact behavior: idempotency short-circuit, self-follow drop, Rust-side diff, single-transaction DELETE+INSERT+freshness UPDATE.
- `src/store/pubkeys.rs` (read) — `upsert_pubkey` (ON CONFLICT RETURNING; lands `discovered`), `set_fetch_status` (stamps `last_fetched_at`).
- `src/ingest/mod.rs` (read) — `ValidatedFollowList` contract + `ingest_events` cross-relay dedup + newest-wins resolution.
- `src/relay/mod.rs` (read) — `acquire_validated_lists_client` / `acquire_validated_lists` (generic injected fetch source), `connect_curated`, `spawn_notice_consumer`.
- `.planning/phases/03-.../03-CONTEXT.md` (read) — locked decisions D-01..D-12, discretion areas, deferred scope.
- `.planning/phases/01-.../01-03-SUMMARY.md`, `.../02-.../02-04-SUMMARY.md` (read) — established patterns, `.sqlx/` offline convention, the Phase 3 readiness note describing the exact seam.
- PostgreSQL official docs — `SELECT` / `UPDATE` (FROM, RETURNING, FOR UPDATE SKIP LOCKED semantics). [postgresql.org/docs]

### Secondary (MEDIUM confidence)
- Standard Postgres job-queue practice (`FOR UPDATE SKIP LOCKED` claim pattern) cross-checked across multiple sources (Netdata academy, BigBinary/Solid Queue, DB Pro blog) — confirms the claim/lease/status-machine pattern Phase 3 uses is canonical.

### Tertiary (LOW confidence)
- none.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — no new deps; all versions verified against committed Cargo.toml; CLAUDE.md locks the stack.
- Architecture: HIGH — fully determined by locked CONTEXT decisions D-01..D-12; existing code read directly to confirm seams.
- Pitfalls: HIGH — derived from the actual schema/code (CHECK widening, `.sqlx/` offline, terminal stamping) and the explicit anti-patterns in CLAUDE.md + CONTEXT.

**Research date:** 2026-06-13
**Valid until:** 2026-07-13 (30 days — stable locked stack; re-verify only if Cargo.toml/CLAUDE.md stack changes)

## Sources

- [PostgreSQL: Documentation 18: SELECT](https://www.postgresql.org/docs/current/sql-select.html)
- [PostgreSQL: Documentation 18: UPDATE](https://www.postgresql.org/docs/current/sql-update.html)
- [Using FOR UPDATE SKIP LOCKED For Queue Workflows | Netdata](https://www.netdata.cloud/academy/update-skip-locked/)
- [Solid Queue & understanding UPDATE SKIP LOCKED | BigBinary Blog](https://www.bigbinary.com/blog/solid-queue)
- [PostgreSQL FOR UPDATE SKIP LOCKED: The One-Liner Job Queue | DB Pro Blog](https://www.dbpro.app/blog/postgresql-skip-locked)
