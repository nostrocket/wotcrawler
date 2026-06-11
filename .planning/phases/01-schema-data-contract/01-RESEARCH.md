# Phase 1: Schema & Data Contract - Research

**Researched:** 2026-06-11
**Domain:** PostgreSQL schema design, sqlx migrations, raw-SQL-as-contract, cross-process MVCC concurrency, transactional edge-diff writer
**Confidence:** HIGH

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**Consumer contract surface**
- **D-01:** The spam layer queries **stable SQL views**, not base tables. Views insulate the consumer from internal schema changes; the views ARE the public contract.
- **D-02:** Contract documented redundantly: a committed **SCHEMA.md** (every contract view/column, types, semantics, example queries) plus **`COMMENT ON`** statements in migrations so the contract is introspectable from psql.
- **D-03:** Contract views expose **surrogate bigint ids only** on edge data. A separate id→pubkey lookup view resolves ids to 32-byte pubkeys at the edges of the consumer's computation. No pubkey join on the hot edge path.
- **D-04:** Contract versioning is **informal**: a "Contract changes" changelog section in SCHEMA.md. No version table, no versioned view names — single operator owns both projects.

**Edge metadata richness**
- **D-05:** Edges are **bare**: `(follower_id, followee_id)` and nothing else. Freshness lives per-pubkey, not per-edge. The trust walk needs structure only.
- **D-06:** Kind-3 p-tag **relay hints and petnames are discarded** at ingest. Relay discovery is NIP-65's job (Phase 5).
- **D-07:** Per pubkey, retain the **applied event's id and created_at** (no raw event JSON). Supports newest-wins resolution (INGEST-03) and the "same event id touches zero edge rows" idempotency check (GRAPH-02).
- **D-08:** **Self-follows are dropped at ingest** (filtered in the store layer). Rule documented in SCHEMA.md.

**Freshness model shape**
- **D-09:** Per-pubkey fetch lifecycle is a **status enum** (`discovered` / `fetched` / `not_found` / `failed`) plus `last_fetched_at` and `last_confirmed_at` timestamps. Indexable for the Phase 4 staleness scanner and Phase 5 not_found→NIP-65 fallback.
- **D-10:** FRESH-03 churn columns (`last_changed_at`, `fetch_count`, `change_count`) are **included in Phase 1** — cheap per-pubkey, populated naturally by the writer, no later migration on the big table.
- **D-11:** Contract views **expose freshness** (status, last_fetched_at) but **hide internal crawl bookkeeping** (counters, failure detail).
- **D-12:** Discovered-but-unfetched pubkeys ARE **visible in contract views** with their status, so the consumer sees honest knowledge boundaries during the multi-day initial crawl.

**Migration scope strategy**
- **D-13:** Phase 1 migrations define **graph + freshness only** (pubkeys, follows, contract views). Frontier (Phase 3), relay registry/health (Phase 5), kind:10002 storage (Phase 2) arrive as additive migrations in their own phases.
- **D-14:** Concurrent reader+writer access (success criterion 3) is proven by an **automated integration test**: a writer task doing store-layer edge upserts and a separate reader connection running contract-view queries simultaneously, asserting neither blocks. Runs against a test Postgres.
- **D-15:** The Phase 1 store layer ships the **full write API, including the transactional edge-diff writer** (apply replacing kind-3 as insert-added/delete-removed in one transaction; same event id = zero edge rows touched). This exercises the schema for real.
- **D-16:** GRAPH-02 stays mapped to Phase 3: Phase 1 **builds** the edge-diff writer API; Phase 3 **consumes and formally verifies** it against real validated events. No roadmap edit.

### Claude's Discretion
- Index strategy (forward/reverse edge indexes, partial indexes on status/staleness), exact Postgres types, migration file naming, test database setup (testcontainers vs docker-compose), connection pool sizing — planner/researcher decide within the locked stack (sqlx 0.9, Postgres 16/17, raw SQL migrations via sqlx-cli).

### Deferred Ideas (OUT OF SCOPE)
- None — discussion stayed within phase scope. (Note for Phase 3 planner: the edge-diff writer API will already exist from Phase 1; Phase 3 verifies GRAPH-02 against real validated events rather than building the writer.)
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| GRAPH-01 | PostgreSQL schema stores pubkeys (surrogate bigint ids), directed follow edges, and per-pubkey freshness metadata, with versioned migrations | Standard Stack (sqlx-cli migrations + `_sqlx_migrations` tracking), Architecture Patterns (surrogate-id schema), Code Examples (migration + schema DDL) |
| GRAPH-03 | A separate process (the spam layer) can read the graph concurrently while the crawler writes, without coordination | Postgres MVCC guarantee (readers never block writers); Validation Architecture (concurrent reader+writer integration test, D-14) |
| GRAPH-04 | The schema is documented as the public contract for downstream consumers | Contract-views pattern (D-01/D-03), `COMMENT ON` introspection (D-02), SCHEMA.md doc (Architecture Patterns) |

**Schema must also accommodate (built here, consumed/verified later):**
- GRAPH-02 (Phase 3) — edge-diff writer API built here (D-15), verified there (D-16)
- FRESH-01 (Phase 3) — `last_fetched_at` / `last_confirmed_at` columns (D-09)
- FRESH-03 (Phase 4) — churn columns `last_changed_at` / `fetch_count` / `change_count` (D-10)
- INGEST-03 (Phase 2) — `applied_event_id` / `applied_created_at` columns for newest-wins (D-07)
</phase_requirements>

## Summary

This is a greenfield Rust + PostgreSQL phase whose entire stack is already locked by CLAUDE.md (Rust, sqlx 0.9, Postgres 16/17, sqlx-cli raw-SQL migrations, surrogate bigint ids) and whose design is fully specified by CONTEXT.md decisions D-01 through D-16. Research therefore confirms the *mechanics* of those decisions rather than exploring alternatives: how sqlx-cli migrations achieve idempotent re-runs, how Postgres MVCC delivers the lock-free concurrent reader/writer guarantee (GRAPH-03) for free, how `COMMENT ON` + stable views form the introspectable public contract (GRAPH-04), and how the transactional edge-diff writer (D-15) is written with `INSERT ... ON CONFLICT DO NOTHING` for additions plus `DELETE` for removals inside one transaction.

All recommended crate versions were verified live against the crates.io sparse index on 2026-06-11 and match CLAUDE.md (sqlx 0.9.0, tokio 1.52.3, thiserror 2.0.18, anyhow 1.0.102, config 0.15.23). One **material correction**: sqlx 0.9.0 declares MSRV **Rust 1.94.0**, not the "1.84+" listed in CLAUDE.md — the planner must require a Rust toolchain ≥ 1.94. The test-database choice (Claude's discretion under D-14) resolves to **testcontainers 0.27.3 + testcontainers-modules 0.15.0 (postgres feature)**, which spins an ephemeral Postgres container per test run and proves the concurrency criterion without a developer-managed database.

**Primary recommendation:** Ship a single Phase-1 migration `0001_graph_schema.sql` (transactional, idempotent via `IF NOT EXISTS`) defining `pubkeys`, `follows`, the per-pubkey freshness/churn columns, the contract views, and `COMMENT ON` statements; build an sqlx store module exposing the full write API including the transactional edge-diff writer; and prove GRAPH-03 with a testcontainers-backed integration test. Standard `CREATE INDEX` (not `CONCURRENTLY`) is correct here because the tables are empty at migration time.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Pubkey / edge / freshness persistence | Database / Storage | — | The schema IS the project boundary; all state lives in Postgres. |
| Versioned schema evolution | Database / Storage | Build tooling (sqlx-cli) | Migrations are SQL files applied by sqlx-cli; the `_sqlx_migrations` table tracks state. |
| Public contract surface | Database / Storage | Docs (SCHEMA.md) | Stable views + `COMMENT ON` are the contract; SCHEMA.md mirrors it for humans. |
| Concurrent cross-process read/write | Database / Storage | — | Postgres MVCC handles this at the engine level; no application coordination. |
| Edge-diff write logic (newest-wins, add/remove) | API / Backend (Rust store layer) | Database (transaction) | Business logic (diffing, self-follow drop, newest-wins) lives in Rust; atomicity is delegated to a Postgres transaction. |
| Connection pooling | API / Backend (Rust store layer) | — | `sqlx::PgPool` owns connection lifecycle for the crawler process. |

## Standard Stack

> Stack is locked by CLAUDE.md. The table below confirms versions and pins the exact feature set Phase 1 needs. All versions verified against the crates.io sparse index (`https://index.crates.io`) on 2026-06-11.

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| sqlx | 0.9.0 | Async Postgres driver, compile-time-checked queries, built-in migrations + pool | Locked in CLAUDE.md. Raw SQL = schema-as-contract; `query!`/`query_as!` give compile-time verification. `[VERIFIED: crates.io sparse index]` |
| tokio | 1.52.3 | Async runtime required by sqlx | Locked. `[VERIFIED: crates.io sparse index]` |
| thiserror | 2.0.18 | Typed errors for the store module crate boundary | Locked (CLAUDE.md: typed library errors). `[VERIFIED: crates.io sparse index]` |
| anyhow | 1.0.102 | Application-level error plumbing (binary / tests) | Locked. `[VERIFIED: crates.io sparse index]` |
| config | 0.15.23 | Layered config (DB URL for store/tests) | Locked. `[VERIFIED: crates.io sparse index]` |
| PostgreSQL | 16 or 17 | Shared graph store | Locked (FIRM in CLAUDE.md). MVCC gives the GRAPH-03 guarantee. `[CITED: CLAUDE.md]` |

**Required sqlx feature set for Phase 1** (verified in sqlx 0.9.0 `features2`):
`["runtime-tokio", "tls-rustls", "postgres", "macros", "migrate", "chrono"]`
- `runtime-tokio` + `tls-rustls` — per CLAUDE.md version-compatibility note. In 0.9, `tls-rustls` resolves to `tls-rustls-ring`. `[VERIFIED: crates.io sparse index features2]`
- `postgres` — enables `sqlx-postgres`. `[VERIFIED]`
- `macros` — enables `query!`/`query_as!` and `migrate!`. `[VERIFIED]`
- `migrate` — enables the migration runner used by integration tests and `sqlx::migrate!`. `[VERIFIED]`
- `chrono` (or `time`) — maps `TIMESTAMPTZ` ↔ Rust datetime for the freshness columns. Pick one; `chrono` is the more common default. `[VERIFIED]`
- `uuid` — NOT needed (pubkeys are 32-byte `bytea`, ids are `bigint`).

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| testcontainers | 0.27.3 | Ephemeral Docker containers from Rust tests | Backs the D-14 concurrent reader/writer test and any migration round-trip test. `[VERIFIED: crates.io sparse index]` |
| testcontainers-modules | 0.15.0 (feature `postgres`) | Prebuilt Postgres container module | One-line ephemeral Postgres per test; returns a connection string. `postgres` feature confirmed present. `[VERIFIED: crates.io sparse index]` |

### Development Tools
| Tool | Purpose | Notes |
|------|---------|-------|
| sqlx-cli | `sqlx migrate add/run`, `cargo sqlx prepare` | `cargo install sqlx-cli --no-default-features --features postgres,rustls`. Generates `.sqlx/` offline metadata so compile-time query checks work in CI without a live DB. `[CITED: CLAUDE.md]` |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| testcontainers (D-14) | docker-compose + `DATABASE_URL` env | docker-compose needs a developer-managed lifecycle and a fixed port; testcontainers is self-contained per test run and parallelizes cleanly. Both valid under D-14; testcontainers is the lower-friction default for an automated criterion-3 test. |
| sqlx | diesel / sea-orm | Explicitly rejected in CLAUDE.md — ORM DSL hides the schema that IS the public API. |
| `chrono` feature | `time` feature | Either maps `TIMESTAMPTZ`. Pick one and stay consistent; do not enable both unnecessarily. |

**Installation:**
```toml
# Cargo.toml
[dependencies]
sqlx = { version = "0.9.0", default-features = false, features = ["runtime-tokio", "tls-rustls", "postgres", "macros", "migrate", "chrono"] }
tokio = { version = "1.52", features = ["rt-multi-thread", "macros"] }
thiserror = "2.0.18"
anyhow = "1.0.102"
config = "0.15.23"

[dev-dependencies]
testcontainers = "0.27.3"
testcontainers-modules = { version = "0.15.0", features = ["postgres"] }
```
```bash
cargo install sqlx-cli --no-default-features --features postgres,rustls
```

**Version verification:** Done 2026-06-11 against the crates.io sparse index. ⚠️ **sqlx 0.9.0 declares `rust_version: 1.94.0`** (verified in the index metadata). CLAUDE.md says "Rust 1.84+" — the planner must set the toolchain floor to **1.94** (e.g. `rust-version = "1.94"` in Cargo.toml / a `rust-toolchain.toml`), or `cargo build` fails. testcontainers 0.27.3 and testcontainers-modules 0.15.0 declare MSRV 1.88 (covered by 1.94). `[VERIFIED: crates.io sparse index]`

## Package Legitimacy Audit

| Package | Registry | Age | Downloads | Source Repo | Verdict | Disposition |
|---------|----------|-----|-----------|-------------|---------|-------------|
| sqlx | crates.io | since 2019-06 | ~2.2M/wk | github.com/launchbadge/sqlx | OK | Approved |
| tokio | crates.io | since 2016-07 | ~13M/wk | github.com/tokio-rs/tokio | OK | Approved |
| thiserror | crates.io | since 2019-10 | ~20M/wk | github.com/dtolnay/thiserror | OK | Approved |
| anyhow | crates.io | since 2019-10 | ~12M/wk | github.com/dtolnay/anyhow | OK | Approved |
| config | crates.io | since 2015-04 | ~1.6M/wk | (config-rs) | OK | Approved |
| testcontainers | crates.io | since 2018-08 | ~730k/wk | (testcontainers-rs) | OK | Approved |
| testcontainers-modules | crates.io | since 2023-06 | ~461k/wk | (testcontainers-rs-modules) | OK | Approved |

**Packages removed due to [SLOP] verdict:** none
**Packages flagged as suspicious [SUS]:** none
All seven packages returned verdict `OK` from `gsd-tools query package-legitimacy check --ecosystem crates`. Crates have no postinstall/npm-style script vector. `[VERIFIED: package-legitimacy seam]`

## Architecture Patterns

### System Architecture Diagram

```
                          CRAWLER PROCESS (Phase 2-5 fills this in)
                                       │
                                       │ applies validated kind-3
                                       ▼
   ┌───────────────────────── Rust store module (Phase 1 builds) ─────────────────────────┐
   │                                                                                       │
   │   upsert_pubkey(pubkey) -> id        get_or_create id, status='discovered'            │
   │   set_fetch_status(id, status, ts)   freshness lifecycle transitions                  │
   │   apply_follow_list(follower_id,     ── TRANSACTION ──────────────────────────┐       │
   │     event_id, created_at, followees) │ 1. if event_id == applied_event_id:     │       │
   │     [the edge-diff writer, D-15]     │      bump fetch_count, return (no-op)    │       │
   │                                      │ 2. drop self-follows (D-08)             │       │
   │                                      │ 3. resolve followee pubkeys -> ids      │       │
   │                                      │ 4. DELETE removed edges                 │       │
   │                                      │ 5. INSERT added edges ON CONFLICT NOP   │       │
   │                                      │ 6. UPDATE pubkeys freshness+churn       │       │
   │                                      └─────────────────────────────────────────┘       │
   └───────────────────────────────────────────┬───────────────────────────────────────────┘
                                                │ sqlx::PgPool (TLS rustls)
                                                ▼
   ┌──────────────────────────────── PostgreSQL 16/17 ─────────────────────────────────────┐
   │  pubkeys(id BIGSERIAL PK, pubkey BYTEA UNIQUE, status, last_fetched_at,                │
   │          last_confirmed_at, last_changed_at, fetch_count, change_count,                │
   │          applied_event_id, applied_created_at)                                         │
   │  follows(follower_id BIGINT, followee_id BIGINT)  PK(follower_id, followee_id)          │
   │  indexes: follows(follower_id), follows(followee_id), pubkeys(status) partial          │
   │                                                                                        │
   │  CONTRACT VIEWS (public API, GRAPH-04):                                                 │
   │    follow_edges(follower_id, followee_id)            -- bare ids, hot path (D-03/D-05)  │
   │    pubkey_lookup(id, pubkey)                         -- id->32-byte resolution boundary │
   │    pubkey_freshness(id, status, last_fetched_at)     -- honest aging (D-11/D-12)        │
   │  + COMMENT ON every contract view & column (D-02)                                       │
   └────────────────────────────────────────────┬───────────────────────────────────────────┘
                                                 │  MVCC: readers never block writers
                                                 ▼
                          SPAM LAYER PROCESS (separate project) — reads contract views only
```

File-to-implementation mapping is in the Component Responsibilities below, not the diagram.

### Recommended Project Structure
```
.
├── Cargo.toml                      # deps + rust-version = "1.94"
├── rust-toolchain.toml             # pin toolchain >= 1.94 (sqlx 0.9 MSRV)
├── .sqlx/                          # cargo sqlx prepare offline metadata (committed)
├── migrations/
│   └── 0001_graph_schema.sql       # pubkeys, follows, freshness, views, COMMENT ON (D-13)
├── SCHEMA.md                       # public contract doc (D-02, D-04 changelog section)
├── src/
│   ├── lib.rs                      # crate root, re-exports store
│   ├── store/
│   │   ├── mod.rs                  # PgPool wiring, run_migrations()
│   │   ├── pubkeys.rs              # upsert_pubkey, set_fetch_status
│   │   └── follows.rs             # apply_follow_list (edge-diff writer, D-15)
│   └── error.rs                    # thiserror StoreError
└── tests/
    └── concurrency.rs              # D-14 reader+writer integration test (testcontainers)
```

### Pattern 1: Idempotent versioned migration (GRAPH-01, success criterion 1)
**What:** A single transactional migration file using `IF NOT EXISTS` everywhere so re-running is a no-op.
**When to use:** All Phase 1 DDL.
**Key facts:**
- `sqlx migrate run` compares `migrations/` against the `_sqlx_migrations` table (`version BIGINT PK, description, installed_on, success, checksum BYTEA`) and applies only *pending* versions; already-applied versions are skipped — that is what makes re-running a no-op. `[VERIFIED: sqlx docs + launchbadge/sqlx CLI README, web-cross-checked]`
- sqlx wraps each Postgres migration in a transaction *by default*, so DDL either fully applies or rolls back. Writing `IF NOT EXISTS` additionally guards against a partially-applied state and satisfies "re-running from empty is a no-op" defensively. `[VERIFIED: launchbadge/sqlx issue #1966, web]`
- **Do NOT use `CREATE INDEX CONCURRENTLY` in Phase 1.** It cannot run inside a transaction block; it would force a `-- no-transaction` migration. The tables are empty at migration time, so plain `CREATE INDEX` is instant and correct. `CONCURRENTLY` belongs to a *later* phase that adds indexes to an already-large table. `[VERIFIED: launchbadge/sqlx #3527, Postgres docs, web]`

### Pattern 2: Stable contract views as the public API (GRAPH-04, D-01/D-03/D-11/D-12)
**What:** Downstream consumers query views, never base tables. Views decouple the internal schema from the contract.
**When to use:** Every column a downstream consumer reads.
- `follow_edges` exposes bare `(follower_id, followee_id)` — no pubkey join, keeping the hot trust-walk path cheap (D-03, D-05).
- `pubkey_lookup` exposes `(id, pubkey)` for id→32-byte resolution only at the boundary of the consumer's computation.
- `pubkey_freshness` exposes `(id, status, last_fetched_at)` and includes `discovered`/unfetched rows so the consumer sees honest knowledge boundaries (D-11, D-12). It hides counters and failure detail.
- Every view and contract column gets a `COMMENT ON` so `\d+` / `pg_description` makes the contract introspectable from psql (D-02).

### Pattern 3: Transactional edge-diff writer (D-15, builds GRAPH-02)
**What:** Apply a replacing kind-3 as `DELETE` removed edges + `INSERT ... ON CONFLICT DO NOTHING` added edges in one transaction, updating freshness/churn atomically.
**When to use:** The store layer's `apply_follow_list`.
- Compute the diff in Rust (current followee-id set vs. new set): `added = new − current`, `removed = current − new`.
- Same `event_id` as `applied_event_id` ⇒ short-circuit: touch zero edge rows, optionally bump `fetch_count` + `last_confirmed_at`. This is the GRAPH-02 idempotency property the writer must guarantee.
- `INSERT ... ON CONFLICT DO NOTHING` is atomic per-row and tolerates concurrent inserts of the same edge. `[VERIFIED: Postgres INSERT docs, web]`
- Self-follows filtered before the diff (D-08).
- Wrap in `let mut tx = pool.begin().await?; ...; tx.commit().await?;` so a failure leaves no partial edge state — supports OPS-02 (consistent DB state) downstream.

### Anti-Patterns to Avoid
- **Storing 32-byte pubkeys in the `follows` table** — bloats the largest table and every index. Use surrogate `bigint` ids (CLAUDE.md "What NOT to Use", D-03).
- **Exposing base tables to the spam layer** — couples the consumer to internal schema churn. Views only (D-01).
- **`CREATE INDEX CONCURRENTLY` inside a Phase-1 migration** — fails inside the default transaction; unnecessary on empty tables.
- **Per-edge metadata / relay hints / petnames** — explicitly out of scope (D-05, D-06).
- **Storing raw event JSON per pubkey** — only `applied_event_id` + `applied_created_at` are retained (D-07).
- **A version table or versioned view names for the contract** — informal changelog only (D-04).
- **diesel/sea-orm** — hides the schema that is the public contract (CLAUDE.md).

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Migration versioning / idempotent re-run | Custom "have I run this?" tracking table | sqlx-cli + `_sqlx_migrations` + checksums | sqlx already tracks version, checksum, success; detects edited-after-apply migrations. |
| Concurrent reader/writer isolation | Application-level locks, advisory locks, read replicas | Postgres MVCC (default Read Committed) | Engine guarantees readers never block writers — GRAPH-03 is satisfied with zero coordination code. |
| Connection lifecycle / pooling | A hand-rolled connection manager | `sqlx::PgPool` | Built-in async pool with sizing, health, timeouts. |
| Ephemeral test database | Manual local Postgres + teardown scripts | testcontainers-modules `postgres` | Per-test container with auto-cleanup; parallel-safe. |
| Atomic upsert of an edge | SELECT-then-INSERT race handling | `INSERT ... ON CONFLICT DO NOTHING` | Single atomic statement, concurrency-safe. |
| Contract introspection | A custom doc generator | `COMMENT ON` + `\d+` / `pg_description` | Native, lives with the schema, can't drift from DDL. |

**Key insight:** Nearly every "hard" part of this phase (idempotent migrations, concurrency, pooling, atomic upsert) is solved by Postgres + sqlx defaults. The only genuinely custom logic is the *diff computation* in the edge-diff writer; the *atomicity* of applying it is delegated to a Postgres transaction.

## Common Pitfalls

### Pitfall 1: Toolchain below sqlx 0.9 MSRV
**What goes wrong:** `cargo build` fails with a rustc version error; CLAUDE.md's "1.84+" is too low.
**Why it happens:** sqlx 0.9.0 declares `rust_version: 1.94.0`.
**How to avoid:** Add `rust-version = "1.94"` to Cargo.toml and a `rust-toolchain.toml` pinning ≥ 1.94.
**Warning signs:** "package requires rustc 1.94" at build time.

### Pitfall 2: Compile-time query checks fail in CI without a database
**What goes wrong:** `query!`/`query_as!` need either a live `DATABASE_URL` or committed offline metadata; CI breaks.
**Why it happens:** sqlx verifies queries against a real schema at compile time.
**How to avoid:** Run `cargo sqlx prepare` after migrations stabilize and commit the `.sqlx/` directory; set `SQLX_OFFLINE=true` in CI.
**Warning signs:** "set DATABASE_URL to use query macros" compile errors.

### Pitfall 3: Non-idempotent migration breaks "re-run from empty is a no-op"
**What goes wrong:** Success criterion 1 fails if a migration assumes empty state but uses bare `CREATE TYPE`/`CREATE TABLE`.
**Why it happens:** A partially-applied migration (or a CREATE TYPE for the status enum) errors on re-attempt.
**How to avoid:** `CREATE TABLE IF NOT EXISTS`, `CREATE INDEX IF NOT EXISTS`, `CREATE OR REPLACE VIEW`; for the enum use a `DO $$ ... IF NOT EXISTS ... CREATE TYPE ... $$` guard or `CREATE TYPE` in its own first migration. (Note: sqlx runs the SQL and the `_sqlx_migrations` update in separate transactions for Postgres, so idempotent SQL is the safety net. `[VERIFIED: launchbadge/sqlx #1966]`)
**Warning signs:** "type already exists" / "relation already exists" on a second `migrate run`.

### Pitfall 4: Edge-diff writer not actually atomic
**What goes wrong:** A crash between DELETE-removed and INSERT-added leaves the follow list half-applied; GRAPH-02 verification (Phase 3) fails.
**Why it happens:** Statements executed on the pool directly instead of inside one transaction.
**How to avoid:** `pool.begin()` → all diff statements on `&mut *tx` → `tx.commit()`.
**Warning signs:** Edge counts that don't match the applied event after an induced failure.

### Pitfall 5: Status enum representation mismatch between Rust and Postgres
**What goes wrong:** sqlx fails to map the Postgres enum to a Rust type.
**Why it happens:** Postgres native `ENUM` types require `#[derive(sqlx::Type)]` with `#[sqlx(type_name = "...")]`; a mismatch in name/case fails at runtime.
**How to avoid:** Either use a native enum with a matching `#[sqlx(type_name = "fetch_status", rename_all = "lowercase")]` derive, or store status as `TEXT` with a `CHECK` constraint (simpler, fewer mapping pitfalls). Decide and document in SCHEMA.md.
**Warning signs:** "unexpected type" decode errors at query time.

## Code Examples

> Verified patterns. SQL/sqlx syntax confirmed against official Postgres and sqlx documentation (cross-checked via web). Exact column set follows D-05/D-07/D-09/D-10.

### Migration skeleton (`migrations/0001_graph_schema.sql`)
```sql
-- Source: pattern per sqlx-cli README + Postgres docs; idempotent per CONTEXT D-13
-- Status as TEXT + CHECK avoids native-enum sqlx mapping pitfalls (Pitfall 5).

CREATE TABLE IF NOT EXISTS pubkeys (
    id                 BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    pubkey             BYTEA NOT NULL UNIQUE,                       -- 32-byte x-only key
    status             TEXT  NOT NULL DEFAULT 'discovered'
                       CHECK (status IN ('discovered','fetched','not_found','failed')),
    last_fetched_at    TIMESTAMPTZ,                                 -- FRESH-01 (D-09)
    last_confirmed_at  TIMESTAMPTZ,                                 -- FRESH-01 (D-09)
    last_changed_at    TIMESTAMPTZ,                                 -- FRESH-03 (D-10)
    fetch_count        BIGINT NOT NULL DEFAULT 0,                   -- FRESH-03 (D-10)
    change_count       BIGINT NOT NULL DEFAULT 0,                   -- FRESH-03 (D-10)
    applied_event_id   BYTEA,                                       -- INGEST-03 newest-wins (D-07)
    applied_created_at TIMESTAMPTZ                                  -- INGEST-03 (D-07)
);

CREATE TABLE IF NOT EXISTS follows (
    follower_id BIGINT NOT NULL REFERENCES pubkeys(id),
    followee_id BIGINT NOT NULL REFERENCES pubkeys(id),
    PRIMARY KEY (follower_id, followee_id),
    CHECK (follower_id <> followee_id)                              -- self-follow guard (D-08)
);

CREATE INDEX IF NOT EXISTS follows_followee_idx ON follows (followee_id);     -- reverse / in-degree
-- forward lookups covered by the PK's leading column (follower_id).
CREATE INDEX IF NOT EXISTS pubkeys_status_idx ON pubkeys (status)
    WHERE status IN ('discovered','not_found','failed');           -- staleness scanner (Phase 4)

-- Contract views (public API, GRAPH-04 / D-01,D-03,D-11,D-12)
CREATE OR REPLACE VIEW follow_edges AS
    SELECT follower_id, followee_id FROM follows;                  -- bare ids, hot path (D-05)

CREATE OR REPLACE VIEW pubkey_lookup AS
    SELECT id, pubkey FROM pubkeys;                                -- id -> 32-byte boundary (D-03)

CREATE OR REPLACE VIEW pubkey_freshness AS
    SELECT id, status, last_fetched_at FROM pubkeys;               -- honest aging (D-11,D-12)

-- Introspectable contract (D-02)
COMMENT ON VIEW follow_edges   IS 'PUBLIC CONTRACT: directed follow edges as (follower_id, followee_id) surrogate ids. Resolve ids via pubkey_lookup only at computation boundary.';
COMMENT ON VIEW pubkey_lookup  IS 'PUBLIC CONTRACT: maps surrogate id -> 32-byte x-only pubkey (bytea).';
COMMENT ON VIEW pubkey_freshness IS 'PUBLIC CONTRACT: per-pubkey knowledge status. status=discovered means seen-but-not-yet-fetched (honest boundary during initial crawl).';
COMMENT ON COLUMN pubkeys.applied_event_id IS 'INTERNAL: id of the kind-3 event currently applied; used for newest-wins + idempotency.';
```

### sqlx pool + migration runner (`src/store/mod.rs`)
```rust
// Source: sqlx PgPool / migrate! per sqlx docs
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(8)            // tune per OPS; crawler writer pool
        .connect(database_url)
        .await
}

pub async fn run_migrations(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await   // idempotent: applies only pending
}
```

### Transactional edge-diff writer core (`src/store/follows.rs`, D-15)
```rust
// Source: Postgres INSERT ... ON CONFLICT docs + sqlx transaction API
// `added` / `removed` are Vec<i64> of followee ids computed in Rust.
let mut tx = pool.begin().await?;

// idempotency short-circuit (GRAPH-02): same event id -> zero edge rows
// (caller checks applied_event_id == event_id before computing the diff)

for &followee_id in &removed {
    sqlx::query!("DELETE FROM follows WHERE follower_id = $1 AND followee_id = $2",
                 follower_id, followee_id)
        .execute(&mut *tx).await?;
}
for &followee_id in &added {
    sqlx::query!("INSERT INTO follows (follower_id, followee_id) VALUES ($1, $2) \
                  ON CONFLICT DO NOTHING",
                 follower_id, followee_id)
        .execute(&mut *tx).await?;
}

let changed = !added.is_empty() || !removed.is_empty();
sqlx::query!(
    "UPDATE pubkeys SET status='fetched', applied_event_id=$2, applied_created_at=$3, \
     last_fetched_at=now(), last_confirmed_at=now(), fetch_count=fetch_count+1, \
     last_changed_at = CASE WHEN $4 THEN now() ELSE last_changed_at END, \
     change_count = change_count + CASE WHEN $4 THEN 1 ELSE 0 END \
     WHERE id = $1",
    follower_id, event_id, created_at, changed)
    .execute(&mut *tx).await?;

tx.commit().await?;
```

### Concurrent reader+writer integration test (`tests/concurrency.rs`, D-14)
```rust
// Source: testcontainers-modules postgres + sqlx
use testcontainers_modules::postgres::Postgres;
use testcontainers::runners::AsyncRunner;

#[tokio::test]
async fn reader_and_writer_do_not_block() -> anyhow::Result<()> {
    let pg = Postgres::default().start().await?;
    let url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres",
                      pg.get_host_port_ipv4(5432).await?);
    let pool = store::connect(&url).await?;
    store::run_migrations(&pool).await?;

    // writer task: continuous edge upserts on one connection
    let writer = { let p = pool.clone(); tokio::spawn(async move { /* loop apply_follow_list */ }) };
    // reader: separate connection runs contract-view queries concurrently
    let reader_conn = sqlx::PgPool::connect(&url).await?;   // distinct pool == distinct process proxy
    for _ in 0..100 {
        let _rows = sqlx::query("SELECT follower_id, followee_id FROM follow_edges LIMIT 1000")
            .fetch_all(&reader_conn).await?;                // must never block on the writer
    }
    writer.abort();
    Ok(())
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| sqlx 0.8 MSRV ~1.78 | sqlx 0.9 MSRV 1.94 | sqlx 0.9.0 (2026-05-21) | Toolchain floor must be 1.94; correct the "1.84+" in CLAUDE.md. |
| Native Postgres `ENUM` for status fields | `TEXT` + `CHECK` constraint (common 2026 sqlx practice) | ongoing | Avoids sqlx enum-mapping pitfalls; trivially extensible. Either is valid; document the choice. |
| `SERIAL`/`BIGSERIAL` | `BIGINT GENERATED ALWAYS AS IDENTITY` | Postgres ≥ 10, now the recommended default | Standards-compliant identity columns; behaves identically as a surrogate id here. |

**Deprecated/outdated:**
- CLAUDE.md's "Rust 1.84+" floor — superseded by sqlx 0.9's 1.94 MSRV.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | testcontainers (over docker-compose) is the chosen test-DB approach | Standard Stack / Validation | Low — D-14 leaves this to discretion; docker-compose is a documented fallback. Switching is a test-only change. |
| A2 | Status stored as `TEXT + CHECK` rather than a native Postgres enum | Code Examples / Pitfall 5 | Low — both satisfy D-09; affects only the Rust type-mapping and a CHECK vs CREATE TYPE line. Planner may pick native enum. |
| A3 | Exact contract-view/column names (`follow_edges`, `pubkey_lookup`, `pubkey_freshness`) | Code Examples | Low — names are illustrative; the *shape* (bare-id edges, id→pubkey view, freshness view) is locked by D-03/D-11/D-12. SCHEMA.md will define final names. |
| A4 | Pool `max_connections(8)` | Code Examples | Low — sizing is explicit discretion; not load-bearing for Phase 1 correctness. |
| A5 | A single migration file `0001_graph_schema.sql` (vs. split files) | Project Structure | Low — file count/naming is discretion; idempotency and scope (D-13) are what matter. |

## Open Questions

1. **Native Postgres enum vs. TEXT+CHECK for `status`**
   - What we know: both map cleanly; TEXT+CHECK has fewer sqlx pitfalls, native enum is more self-documenting in `\d`.
   - What's unclear: operator preference.
   - Recommendation: default to TEXT+CHECK for Phase 1 simplicity; revisit only if the spam layer wants enum introspection. Document in SCHEMA.md.

2. **Does the concurrency test need to simulate two OS processes, or are two sqlx pools sufficient?**
   - What we know: MVCC isolation is per-connection/transaction, not per-process; two distinct pools against the same database exercise the same engine path a separate process would.
   - What's unclear: whether the verifier wants a literal second process for criterion-3 fidelity.
   - Recommendation: two distinct pools (one for the writer task, one for the reader) is a faithful and sufficient proxy; note the rationale in the test so the verifier accepts it.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Docker | testcontainers (D-14 test), local Postgres | ✓ | client 29.2.1 (desktop-linux context) | docker-compose Postgres / local install |
| Rust toolchain (cargo) | entire build | ✗ | — | **BLOCKING** — must install rustup + toolchain ≥ 1.94 |
| PostgreSQL server (psql/pg_isready) | runtime store, manual checks | ✗ | — | testcontainers spins ephemeral Postgres for tests; a server is needed for the live crawler later |
| sqlx-cli | migrations, `cargo sqlx prepare` | ✗ (not installed) | — | `cargo install sqlx-cli --no-default-features --features postgres,rustls` |

**Missing dependencies with no fallback:**
- **Rust toolchain (cargo) ≥ 1.94** — nothing can build until installed. First plan task should establish the toolchain (rustup + `rust-toolchain.toml` pin 1.94).

**Missing dependencies with fallback:**
- PostgreSQL server — not required for Phase 1 *tests* (testcontainers provides it via Docker, which is available). A persistent server is a later-phase operational concern.
- sqlx-cli — install via cargo as part of phase setup.

## Validation Architecture

> nyquist_validation is enabled (config: `workflow.nyquist_validation: true`).

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in test harness (`#[test]` / `#[tokio::test]`); cargo-nextest optional |
| Config file | none required (Cargo conventions); `SQLX_OFFLINE=true` env in CI |
| Quick run command | `cargo test --lib` (unit tests, no Docker) |
| Full suite command | `cargo test` (includes `tests/concurrency.rs` — requires Docker) |

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| GRAPH-01 | Fresh DB migrates from empty; re-run is a no-op | integration | `cargo test migrations_idempotent` | ❌ Wave 0 |
| GRAPH-01 | Schema has pubkeys(bigint id), follows(id,id), freshness cols | integration | `cargo test schema_shape` | ❌ Wave 0 |
| GRAPH-03 | Reader does not block on concurrent writer | integration | `cargo test reader_and_writer_do_not_block` | ❌ Wave 0 (D-14) |
| GRAPH-04 | Contract views exist with COMMENT ON; expose freshness, hide bookkeeping | integration | `cargo test contract_views_present` | ❌ Wave 0 |
| GRAPH-02* | Edge-diff writer: same event id touches zero edge rows; add/remove atomic | integration | `cargo test edge_diff_writer` | ❌ Wave 0 (built here per D-15; formally verified Phase 3 per D-16) |
| D-08 | Self-follows dropped at ingest | unit/integration | `cargo test self_follow_dropped` | ❌ Wave 0 |

### Sampling Rate
- **Per task commit:** `cargo test --lib` (fast unit tests)
- **Per wave merge:** `cargo test` (full suite incl. testcontainers Postgres)
- **Phase gate:** Full suite green before `/gsd-verify-work`

### Wave 0 Gaps
- [ ] `tests/concurrency.rs` — reader+writer non-blocking (GRAPH-03, D-14)
- [ ] `tests/migrations.rs` — idempotent migrate-from-empty + re-run no-op (GRAPH-01)
- [ ] `tests/contract.rs` — contract views + COMMENT ON present, freshness exposed/bookkeeping hidden (GRAPH-04)
- [ ] `tests/edge_diff.rs` — edge-diff writer add/remove/zero-touch + self-follow drop (D-15, D-08)
- [ ] Shared test fixture: testcontainers Postgres bootstrap + `run_migrations` helper
- [ ] Toolchain install: rustup + `rust-toolchain.toml` (≥ 1.94); `cargo install sqlx-cli`

## Security Domain

> security_enforcement enabled (config: `workflow.security_enforcement: true`, ASVS level 1).

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | No user auth surface in Phase 1 (single-operator daemon, no public API). |
| V3 Session Management | no | No sessions. |
| V4 Access Control | partial | DB-level: the spam layer should connect with a **read-only role** (GRANT SELECT on contract views only). Document in SCHEMA.md as the consumer's connection contract. |
| V5 Input Validation | yes | Pubkeys are 32-byte `bytea` — store-layer enforces length; sqlx parameterized queries prevent injection. |
| V6 Cryptography | no | Signature verification is Phase 2 (INGEST-01); Phase 1 stores already-validated data. Never hand-roll secp256k1 (CLAUDE.md). |

### Known Threat Patterns for Rust + Postgres + sqlx

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| SQL injection | Tampering | sqlx parameterized queries / `query!` macros exclusively — never string-format SQL. |
| Connection-string / DB-URL leakage in logs | Information Disclosure | Load DB URL via `config`; never log it; keep out of git. |
| Over-privileged consumer access | Elevation of Privilege | Spam layer connects as a read-only role limited to contract views, not base tables (V4). |
| Malformed/oversized pubkey input | Tampering / DoS | `bytea` length check at the store boundary; follow-list cap is INGEST-04 (Phase 2) but the schema must not assume bounded input. |

## Sources

### Primary (HIGH confidence)
- crates.io sparse index (`https://index.crates.io`) — verified 2026-06-11: sqlx 0.9.0 (MSRV 1.94.0, features2 incl. postgres/macros/migrate/runtime-tokio/tls-rustls), tokio 1.52.3, thiserror 2.0.18, anyhow 1.0.102, config 0.15.23, testcontainers 0.27.3, testcontainers-modules 0.15.0 (postgres feature present)
- `gsd-tools query package-legitimacy check --ecosystem crates` — all 7 packages verdict OK (downloads, age, repos)
- CLAUDE.md — locked stack, "What NOT to Use", version-compatibility table
- CONTEXT.md — decisions D-01..D-16

### Secondary (MEDIUM confidence)
- launchbadge/sqlx CLI README + docs.rs `sqlx::migrate` — `_sqlx_migrations` table, checksum, pending-only application (web, cross-checked with official docs)
- launchbadge/sqlx issues #1966 / #3527 — Postgres migration transaction handling, `-- no-transaction` and `CREATE INDEX CONCURRENTLY` constraint
- postgresql.org INSERT docs + multiple MVCC references (interdb.jp, heroku devcenter, postgresql.org/docs mvcc) — "readers never block writers", ON CONFLICT atomicity

### Tertiary (LOW confidence)
- General blog posts on TEXT-vs-enum and GENERATED IDENTITY conventions — treated as state-of-the-art guidance, not load-bearing.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — versions verified live against crates.io sparse index; features confirmed in features2.
- Architecture: HIGH — fully specified by locked CONTEXT decisions; patterns confirmed against Postgres/sqlx docs.
- Pitfalls: HIGH — MSRV, idempotency, CONCURRENTLY-in-transaction, and transaction-atomicity all verified against authoritative issues/docs.

**Research date:** 2026-06-11
**Valid until:** 2026-07-11 (stable stack; re-verify sqlx version if a 0.9.x bump lands before planning)
