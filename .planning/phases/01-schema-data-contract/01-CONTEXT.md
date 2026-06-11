# Phase 1: Schema & Data Contract - Context

**Gathered:** 2026-06-11
**Status:** Ready for planning

<domain>
## Phase Boundary

The shared PostgreSQL database the spam layer consumes exists: graph schema (pubkeys with surrogate bigint ids, directed follow edges, per-pubkey freshness metadata), versioned sqlx migrations, an sqlx store layer with the full write API (including the transactional edge-diff writer), stable contract views, and a committed SCHEMA.md documenting the public contract. Concurrent cross-process read/write access is proven by an automated test. (GRAPH-01, GRAPH-03, GRAPH-04)

Out of this phase: relay connections, event validation, crawl/frontier logic, daemon wiring, NIP-65 — those belong to Phases 2–5. Frontier, relay-registry, and kind:10002 tables are NOT defined in Phase 1 migrations.

</domain>

<decisions>
## Implementation Decisions

### Consumer contract surface
- **D-01:** The spam layer queries **stable SQL views**, not base tables. Views insulate the consumer from internal schema changes; the views ARE the public contract.
- **D-02:** Contract documented redundantly: a committed **SCHEMA.md** (every contract view/column, types, semantics, example queries) plus **`COMMENT ON`** statements in migrations so the contract is introspectable from psql.
- **D-03:** Contract views expose **surrogate bigint ids only** on edge data. A separate id→pubkey lookup view resolves ids to 32-byte pubkeys at the edges of the consumer's computation. No pubkey join on the hot edge path.
- **D-04:** Contract versioning is **informal**: a "Contract changes" changelog section in SCHEMA.md. No version table, no versioned view names — single operator owns both projects.

### Edge metadata richness
- **D-05:** Edges are **bare**: `(follower_id, followee_id)` and nothing else. Freshness lives per-pubkey, not per-edge. The trust walk needs structure only.
- **D-06:** Kind-3 p-tag **relay hints and petnames are discarded** at ingest. Relay discovery is NIP-65's job (Phase 5).
- **D-07:** Per pubkey, retain the **applied event's id and created_at** (no raw event JSON). This supports newest-wins resolution (INGEST-03) and the "same event id touches zero edge rows" idempotency check (GRAPH-02).
- **D-08:** **Self-follows are dropped at ingest** (filtered in the store layer). Rule documented in SCHEMA.md.

### Freshness model shape
- **D-09:** Per-pubkey fetch lifecycle is a **status enum** (`discovered` / `fetched` / `not_found` / `failed`) plus `last_fetched_at` and `last_confirmed_at` timestamps. Indexable for the Phase 4 staleness scanner and Phase 5 not_found→NIP-65 fallback.
- **D-10:** FRESH-03 churn columns (`last_changed_at`, `fetch_count`, `change_count`) are **included in Phase 1** — cheap per-pubkey, populated naturally by the writer, no later migration on the big table.
- **D-11:** Contract views **expose freshness** (status, last_fetched_at — "honestly aged" is core value) but **hide internal crawl bookkeeping** (counters, failure detail).
- **D-12:** Discovered-but-unfetched pubkeys ARE **visible in contract views** with their status, so the consumer sees honest knowledge boundaries during the multi-day initial crawl.

### Migration scope strategy
- **D-13:** Phase 1 migrations define **graph + freshness only** (pubkeys, follows, contract views). Frontier (Phase 3), relay registry/health (Phase 5), kind:10002 storage (Phase 2) arrive as additive migrations in their own phases.
- **D-14:** Concurrent reader+writer access (success criterion 3) is proven by an **automated integration test**: a writer task doing store-layer edge upserts and a separate reader connection running contract-view queries simultaneously, asserting neither blocks. Runs against a test Postgres.
- **D-15:** The Phase 1 store layer ships the **full write API, including the transactional edge-diff writer** (apply replacing kind-3 as insert-added/delete-removed in one transaction; same event id = zero edge rows touched). This exercises the schema for real.
- **D-16:** GRAPH-02 stays mapped to Phase 3: Phase 1 **builds** the edge-diff writer API; Phase 3 **consumes and formally verifies** it against real validated events. No roadmap edit.

### Claude's Discretion
- Index strategy (forward/reverse edge indexes, partial indexes on status/staleness), exact Postgres types, migration file naming, test database setup (testcontainers vs docker-compose), connection pool sizing — planner/researcher decide within the locked stack (sqlx 0.9, Postgres 16/17, raw SQL migrations via sqlx-cli).

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Project planning
- `.planning/PROJECT.md` — Core value, scale constraints (low millions of pubkeys, hundreds of millions of edges), consumer contract context
- `.planning/REQUIREMENTS.md` — GRAPH-01, GRAPH-03, GRAPH-04 (this phase) and GRAPH-02/FRESH-01..03 (schema must accommodate)
- `.planning/ROADMAP.md` — Phase 1 success criteria and downstream phase dependencies on this schema

### Stack decisions
- `CLAUDE.md` — Locked technology stack: Rust, sqlx 0.9 (runtime-tokio, tls-rustls), PostgreSQL 16/17, sqlx-cli migrations, surrogate bigint ids, raw-SQL-as-contract; "What NOT to Use" table

No external specs or ADRs exist yet — this is a greenfield repo.

</canonical_refs>

<code_context>
## Existing Code Insights

Greenfield repository — no source code exists yet (only CLAUDE.md and .planning/). Everything built in this phase sets the project's precedents: crate layout, migration conventions, error handling (anyhow/thiserror per stack doc), test patterns.

### Integration Points
- The schema/views are consumed by: Phase 2 (kind:10002 additive tables), Phase 3 (frontier tables + edge-diff writer verification), Phase 4 (staleness scanner queries), Phase 5 (not_found fallback queries), and the external spam-layer project (contract views).

</code_context>

<specifics>
## Specific Ideas

- The contract should let the spam layer weight knowledge by age — freshness exposure is a feature of the contract, not an internal detail.
- Edge views must stay cheap: no pubkey-table join on the hot path; consumers resolve ids only at the boundary of their computation.

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope. (Note for Phase 3 planner: the edge-diff writer API will already exist from Phase 1; Phase 3 verifies GRAPH-02 against real validated events rather than building the writer.)

</deferred>

---

*Phase: 1-Schema & Data Contract*
*Context gathered: 2026-06-11*
