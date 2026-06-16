# Phase 3: Graph Writer & BFS Frontier - Context

**Gathered:** 2026-06-13
**Status:** Ready for planning

<domain>
## Phase Boundary

Accepted follow lists become durable graph state, and a DB-resident,
reachability-gated BFS frontier drives discovery and survives crashes without
redoing completed work. (GRAPH-02, CRAWL-01, CRAWL-02, CRAWL-03, CRAWL-04,
FRESH-01)

This phase delivers:
- **GRAPH-02 verification + wiring:** wire the `ValidatedFollowList` →
  `store::follows::apply_follow_list` seam (resolve each followee pubkey to its
  surrogate id via `upsert_pubkey`, then apply the diff) and formally verify the
  edge-diff writer against real validated events — applying a replacing kind-3
  inserts only added edges and deletes only removed edges in one transaction, and
  re-applying the same event id touches zero edge rows. **The writer itself
  already exists from Phase 1 (D-15/D-16) — Phase 3 consumes and verifies it, it
  does not rebuild it.**
- **The crawl driver + DB-resident frontier:** BFS from a configurable anchor
  pubkey over follow edges, enqueuing only pubkeys reachable through follows
  (CRAWL-01/02), with bounded end-to-end fetch concurrency (CRAWL-04) and
  crash-safe resume (CRAWL-03).
- **One additive migration** extending the freshness model to back the frontier
  (see D-09 below).

**Out of this phase:** the daemon binary / main loop, the continuous staleness
refresh loop and any in-run (non-startup) reclaim sweep, metrics/observability,
graceful shutdown (all Phase 4); NIP-65 outbox fallback and relay health scoring
(Phase 5). Config *sourcing* (anchor, batch size, concurrency, retry cap from a
config file) is OPS-01 / Phase 4 — Phase 3 takes these as parameters with sane
defaults, it does not build the config-file layer.

</domain>

<decisions>
## Implementation Decisions

### Frontier representation
- **D-01:** The frontier is **status-driven on the existing `pubkeys` table — no
  separate frontier table.** The frontier *is* the set of `pubkeys.status =
  'discovered'` rows, served by the existing `pubkeys_status_idx` partial index.
  One source of truth; no follower-table / frontier-table sync to keep
  consistent.
- **D-02:** Traversal is **order-agnostic** — the goal is to reach every reachable
  pubkey exactly once; order does not matter. Claim any `discovered` row (e.g. by
  `id`). **No discovery-sequence / depth column.** The CRAWL-02 reachability gate
  holds structurally regardless of order: a pubkey only ever becomes `discovered`
  because it was a followee in a follow list we fetched from someone already in
  the graph (seeded by the anchor). Spam islands nobody legitimate points to are
  never inserted, so they are never crawled.
- **D-03:** The anchor pubkey is the **only seed**: pre-insert the configurable
  anchor as a `discovered` pubkey row; the BFS expands purely by applying fetched
  follow lists (every followee gets `upsert_pubkey`'d, landing as `discovered`
  unless already known).

### Claim & crash recovery
- **D-04:** Claiming uses a **persisted lease**: a claim flips status
  `discovered → in_progress` and stamps `claimed_at`, in its **own short
  transaction** using `SELECT ... FOR UPDATE SKIP LOCKED` to prevent two workers
  claiming the same row. The DB row lock is **not** held for the duration of the
  (multi-second, multi-relay) fetch — only for the claim flip.
- **D-05:** In-flight work is **re-fetched after a crash (at-least-once)**. This
  is safe because `apply_follow_list` is idempotent on an unchanged event id
  (re-applying touches zero edge rows). CRAWL-03's guarantee is about not
  refetching *completed* (`fetched`) work, not about exactly-once for in-flight.
- **D-06:** Stale `in_progress` rows are reclaimed to `discovered` by a
  **startup-only sweep** (a clean shutdown leaves none; a crash leaves some). This
  fully satisfies CRAWL-03 within Phase 3 scope. A continuous in-run reclaim sweep
  for a long-lived process belongs with the Phase 4 daemon loop and is deferred.

### Fetch unit / batching
- **D-07:** A worker **batch-claims N authors at once** and fetches them as one
  author-chunked request set (reusing the existing
  `acquire_validated_lists_client` author-chunking machinery). N is a parameter
  (config knob in Phase 4). This serves the core "each list fetched roughly once"
  / relay-goodwill constraint by minimizing relay round-trips at
  millions-of-pubkeys scale. A partially-failing batch resolves status per author
  (see retry decisions).
- **D-08:** For a claimed batch, **fan out to all curated relays → collect the raw
  event union → run `ingest::ingest_events` once.** This is the "fetch from the
  wild" model: newest-wins across relays, resilient to any single relay
  missing/lagging a pubkey. The ingest gate already cross-relay dedups by event id
  and resolves the replaceable winner over the full union, so unioning before
  ingest introduces no double-processing. Per-relay rate limiting
  (`RateLimiterRegistry`) already applies inside the fetch path.

### Failure & retry policy
- **D-09:** **Bounded in-crawl retry for transient errors, then a recorded
  terminal status.** On a transient fetch error, return the author to
  `discovered` and bump a per-pubkey **`fetch_attempts`** counter (so it is
  re-claimed later in the same crawl, naturally interleaved rather than hammering
  a flaky relay immediately). After a **configurable max attempts (default ~3)**,
  set status `failed`.
- **D-10:** **`not_found`** (relays answered but no kind-3 exists for that author)
  is **terminal for this phase** and is the specific target for the Phase 5 NIP-65
  outbox fallback (RELAY-05). `failed` (errors exhausted retries) is the
  errored-out terminal state.
- **D-11:** Both `failed` and `not_found` are recorded (status + `last_fetched_at`
  stamped) so the Phase 4 staleness loop can re-enqueue them later — the existing
  `pubkeys_status_idx` partial index already targets
  `('discovered','not_found','failed')`.

### Additive migration (D-09 schema delta)
- **D-12:** This phase ships **one additive migration** extending the freshness
  model to back the frontier:
  - add `'in_progress'` to the `pubkeys.status` CHECK constraint domain,
  - add `claimed_at TIMESTAMPTZ` (lease timestamp, internal),
  - add `fetch_attempts` (small int, internal retry counter).
  These are **internal bookkeeping columns** — keep them out of the contract views
  (consistent with Phase 1 D-11: contract views hide crawl bookkeeping). Update
  `COMMENT ON` to label them INTERNAL. The `'in_progress'` status is internal too;
  decide whether `pubkey_freshness` exposes it or collapses it (lean toward NOT
  exposing transient `in_progress` in the contract — it is mid-flight churn, not a
  stable knowledge state). Follows Phase 1 D-13: frontier support arrives as its
  own additive migration.

### Claude's Discretion
- Worker-pool / concurrency mechanism (fixed task pool pulling from the DB queue
  vs. a semaphore over a spawn loop), exact bounded-concurrency implementation for
  CRAWL-04, batch size N default, retry-cap default, claim batch SQL, the
  `in_progress` exposure choice in `pubkey_freshness`, and GRAPH-02 verification
  test construction (real validated events through the wired seam) — all
  planner/researcher decisions within the locked stack (sqlx 0.9, Postgres 16/17,
  `FOR UPDATE SKIP LOCKED`). The startup reclaim sweep's "stale" definition is
  effectively "any `in_progress` at startup" (D-06), so no age threshold is needed
  for the Phase 3 startup case.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Project planning
- `.planning/PROJECT.md` — Core value (each list fetched roughly once; relay
  goodwill finite), scale constraints (low millions of pubkeys, hundreds of
  millions of edges)
- `.planning/REQUIREMENTS.md` — GRAPH-02, CRAWL-01..04, FRESH-01 (this phase);
  RELAY-05 (Phase 5 NIP-65 fallback that consumes `not_found`); FRESH-02/03
  (Phase 4 staleness loop that re-enqueues `failed`/`not_found`)
- `.planning/ROADMAP.md` — Phase 3 goal + 5 success criteria; Phase 4/5
  dependencies on the frontier and terminal statuses this phase produces
- `.planning/phases/01-schema-data-contract/01-CONTEXT.md` — D-13 (frontier is an
  additive Phase 3 migration), D-15/D-16 (writer built in Phase 1; Phase 3 verifies
  GRAPH-02), D-09 (status enum), D-11 (contract views hide bookkeeping)

### Stack decisions
- `CLAUDE.md` — Locked stack: Rust, sqlx 0.9, PostgreSQL 16/17, in-memory BFS
  frontier driven by indexed DB adjacency lookups (NOT recursive-CTE BFS — see
  "What NOT to Use"); surrogate bigint ids on the edge table

### Existing code this phase builds on (read before implementing)
- `src/store/follows.rs` — `apply_follow_list` (the transactional edge-diff writer
  Phase 3 wires + verifies; idempotent, self-follow-dropping, atomic)
- `src/store/pubkeys.rs` — `upsert_pubkey` (32-byte → surrogate id, lands new keys
  as `discovered`), `set_fetch_status` (drives the freshness lifecycle)
- `src/ingest/mod.rs` — `ValidatedFollowList` (the input contract: `follower_pubkey`,
  `event_id`, `created_at`, `followee_pubkeys`) and `ingest_events` (cross-relay
  dedup + newest-wins over the event union)
- `src/relay/mod.rs` — `acquire_validated_lists_client` (per-relay, author-chunked
  acquire path), `connect_curated`, `spawn_notice_consumer`
- `migrations/0001_graph_schema.sql` — current schema; the Phase 3 additive
  migration extends `status`/adds `claimed_at`/`fetch_attempts`

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- **`apply_follow_list` (src/store/follows.rs):** already does the full
  transactional diff + freshness/churn update + GRAPH-02 idempotency short-circuit.
  Phase 3 calls it; the seam is followee-pubkey → id resolution before the call.
- **`upsert_pubkey` (src/store/pubkeys.rs):** the discovery mechanism — every
  followee resolved through it lands as `discovered`, which IS the enqueue. No
  separate "enqueue" call needed.
- **`acquire_validated_lists_client` (src/relay/mod.rs):** author-chunked,
  per-relay, rate-limited, NIP-11-cap-sourced acquire path. The batch fetch unit
  (D-07) and per-relay fan-out (D-08) are built on top of this.
- **`pubkeys_status_idx` partial index:** already targets
  `('discovered','not_found','failed')` — the claim query and the Phase 4
  staleness scan both ride it. Note it does NOT currently include `in_progress`;
  the planner should decide whether the claim path needs it indexed.

### Established Patterns
- **Status-as-queue:** the freshness lifecycle (D-09, Phase 1) was deliberately
  built indexable for exactly this — Phase 3 adds `in_progress` and rides it.
- **Count-and-skip vs genuine-error split (`RelayError`/`IngestError`):** routine
  adversarial rejections are counted and skipped inside ingest; only genuine
  errors surface. The retry policy (D-09) keys off genuine `RelayError`s from the
  fetch path, not ingest count-and-skip.
- **Idempotent additive migrations (`IF NOT EXISTS`, `CREATE OR REPLACE`):** the
  Phase 3 migration must follow the Phase 1 idempotency convention.

### Integration Points
- **fetch → ingest → store:** the crawl driver composes
  `acquire_validated_lists_client` (per relay, fanned out) → union →
  `ingest_events` → for each `ValidatedFollowList`: `upsert_pubkey` follower +
  each followee → `apply_follow_list`.
- **Downstream consumers of this phase's output:** Phase 4 staleness loop
  (re-enqueues `failed`/`not_found`, adds in-run reclaim sweep), Phase 5 NIP-65
  fallback (consumes `not_found`).

</code_context>

<specifics>
## Specific Ideas

- The frontier must not require a second structure to stay consistent with graph
  state — reusing `pubkeys.status` was a deliberate choice over a dedicated table.
- Relay efficiency is a first-class value, not an optimization: batch-claim +
  fan-out-and-union exists specifically to honor "each list fetched roughly once."
- Crash safety leans on the writer's existing idempotency rather than on
  exactly-once claiming — simpler, and the idempotency was already proven in
  Phase 1.

</specifics>

<deferred>
## Deferred Ideas

- **Continuous (in-run) stale-lease reclaim sweep** — deferred to Phase 4 daemon
  loop (D-06). Phase 3 does startup-only reclaim.
- **Config-file sourcing of anchor / batch size N / concurrency cap / retry cap**
  — OPS-01, Phase 4. Phase 3 takes them as parameters with defaults.
- **NIP-65 fallback for `not_found` pubkeys** — Phase 5 (RELAY-05); Phase 3 only
  records the terminal `not_found` status.
- **Staleness/TTL re-enqueue of `failed`/`not_found`** — Phase 4 (FRESH-02);
  Phase 3 records the statuses the scanner will consume.

</deferred>

---

*Phase: 3-Graph Writer & BFS Frontier*
*Context gathered: 2026-06-13*
