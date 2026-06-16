---
phase: 01-schema-data-contract
plan: 03
subsystem: database
tags: [rust, sqlx, postgres, store-layer, edge-diff, mvcc, schema-contract]

# Dependency graph
requires:
  - phase: 01-01
    provides: "Rust crate + sqlx 0.9 + StoreError + start_postgres() testcontainers fixture"
  - phase: 01-02
    provides: "migrations/0001_graph_schema.sql (pubkeys, follows, 3 contract views, COMMENT ON)"
provides:
  - "store::connect(url) -> PgPool + store::run_migrations(&pool) programmatic migration runner"
  - "store::pubkeys::upsert_pubkey(&pool, &[u8]) -> i64 — 32-byte-validated get-or-create surrogate id"
  - "store::pubkeys::set_fetch_status(&pool, id, status, ts) — freshness lifecycle transition"
  - "store::follows::apply_follow_list(&pool, follower_id, event_id, created_at, &[i64]) -> bool — transactional edge-diff writer (D-15)"
  - "StoreError::InvalidPubkey(usize) boundary-validation variant (V5)"
  - "SCHEMA.md — committed public contract document (GRAPH-04)"
  - "Committed .sqlx/ offline query metadata (SQLX_OFFLINE=true builds pass in CI)"
affects: [phase-02-ingest, phase-03-frontier, phase-04-staleness, phase-05-relay, spam-layer-consumer]

# Tech tracking
tech-stack:
  added:
    - "chrono 0.4 (default-features off, clock) — DateTime<Utc> in the public store API"
    - "sqlx-cli 0.9.0 (dev tool, ~/.cargo) — cargo sqlx prepare for offline metadata"
  patterns:
    - "Transactional edge-diff writer: diff computed in Rust, DELETE+INSERT+freshness UPDATE in one pool.begin()/tx.commit()"
    - "Idempotency short-circuit on unchanged applied_event_id (zero edge rows touched, GRAPH-02)"
    - "ON CONFLICT (pubkey) DO UPDATE SET pubkey=EXCLUDED.pubkey RETURNING id — get-or-create in one round trip"
    - "Parameterized sqlx query!/query_scalar! macros only — never string-formatted SQL"
    - "Committed .sqlx/ offline metadata so compile-time query checks work without a live DB"
    - "Two distinct PgPools as the separate-process proxy for the MVCC concurrency test"

key-files:
  created:
    - src/store/pubkeys.rs
    - src/store/follows.rs
    - SCHEMA.md
    - tests/edge_diff.rs
    - tests/concurrency.rs
    - .sqlx/ (15 query metadata files)
  modified:
    - src/store/mod.rs
    - src/error.rs
    - Cargo.toml
    - Cargo.lock

key-decisions:
  - "Added chrono as a direct dependency for DateTime<Utc> in the public store API signatures (sqlx chrono feature already enabled)"
  - "Generated .sqlx/ with `cargo sqlx prepare -- --all-targets` so test-file query macros are covered for offline CI builds"
  - "Status passed as &str at the set_fetch_status boundary (TEXT+CHECK schema); the DB CHECK enforces the closed domain"

patterns-established:
  - "Edge-diff writer: short-circuit on unchanged event id, drop self-follows, Rust-side set diff, atomic transaction for DELETE+INSERT+freshness/churn"
  - "Store-boundary input validation: 32-byte pubkey length checked before any query (V5 / T-03-02)"
  - "Offline-first sqlx builds: .sqlx/ committed, SQLX_OFFLINE=true in CI"

requirements-completed: [GRAPH-03, GRAPH-04]

# Metrics
duration: 9min
completed: 2026-06-12
---

# Phase 01 Plan 03: Store Layer, Edge-Diff Writer & Schema Contract Summary

**sqlx store layer over the Phase 1 schema — pool wiring + programmatic migrations, get-or-create pubkey upsert with 32-byte validation, freshness transitions, and the transactional edge-diff writer (idempotent on unchanged event id, self-follow-dropping, atomic) — proven by green edge-diff and MVCC concurrency tests, documented by a committed SCHEMA.md, and CI-ready via committed `.sqlx/` offline metadata.**

## Performance

- **Duration:** ~9 min
- **Started:** 2026-06-12T06:57:31Z
- **Completed:** 2026-06-12T07:07Z
- **Tasks:** 3
- **Files modified:** 10 (6 created, 4 modified, plus 15 `.sqlx/` metadata files)

## Accomplishments
- Full Phase 1 write API: `connect` / `run_migrations`, `upsert_pubkey` / `set_fetch_status`, and the transactional edge-diff writer `apply_follow_list` (D-15).
- GRAPH-03 proven: a reader on a separate pool runs 100 `follow_edges` SELECTs concurrently with a churning writer; MVCC means neither blocks (`reader_and_writer_do_not_block`).
- GRAPH-04 delivered: committed `SCHEMA.md` documents all three contract views + columns, status semantics, the self-follow rule, bare-edge/id-resolution guidance, the read-only consumer role, the TEXT-vs-enum decision, and a `## Contract changes` changelog.
- Edge-diff writer is idempotent on an unchanged `applied_event_id` (zero edge rows touched, only confirm counters bump — GRAPH-02 property), drops self-follows (D-08), and applies the whole diff atomically in one transaction (Pitfall 4).
- Committed `.sqlx/` offline metadata makes `SQLX_OFFLINE=true cargo build` pass without a live DB (Pitfall 2).

## Task Commits

Each task was committed atomically:

1. **Task 1: Store pool wiring + pubkey upsert/freshness API** - `21b45cc` (feat)
2. **Task 2: Transactional edge-diff writer + edge-diff tests** - `1d6e731` (test) — the writer impl (`follows.rs`) shipped in `21b45cc` so the crate compiled; Task 2's distinct artifact is the green test suite.
3. **Task 3: Concurrency test + SCHEMA.md + committed .sqlx** - `4f0af1b` (test)

**Plan metadata:** (this commit) — docs: complete plan

_Note: this was a TDD plan; the writer and its tests landed across the Task 1/Task 2 commits because `mod.rs` references `follows.rs` and must compile at Task 1._

## Final Store API (for Phase 2/3 consumers)

```rust
// src/store/mod.rs
pub async fn connect(database_url: &str) -> Result<PgPool, StoreError>;
pub async fn run_migrations(pool: &PgPool) -> Result<(), StoreError>;

// src/store/pubkeys.rs
pub async fn upsert_pubkey(pool: &PgPool, pubkey: &[u8]) -> Result<i64, StoreError>;
pub async fn set_fetch_status(pool: &PgPool, id: i64, status: &str, ts: DateTime<Utc>)
    -> Result<(), StoreError>;

// src/store/follows.rs
pub async fn apply_follow_list(
    pool: &PgPool,
    follower_id: i64,
    event_id: &[u8],
    created_at: DateTime<Utc>,
    followee_ids: &[i64],
) -> Result<bool, StoreError>; // Ok(false) = unchanged-event-id short circuit
```

- `apply_follow_list` accepts **resolved followee ids only**. The caller resolves followee pubkeys → ids via `upsert_pubkey` first; kind-3 p-tag relay hints and petnames are discarded at the ingest boundary and never reach the store (D-06).
- Returns `Ok(true)` if the edge set or applied event changed, `Ok(false)` on the unchanged-event-id short circuit.
- `StoreError::InvalidPubkey(usize)` is returned by `upsert_pubkey` for any non-32-byte input.

## SCHEMA.md Contract Surface

`SCHEMA.md` (repo root, committed) documents, for the downstream spam layer:
- The three contract views with every column + type + semantics: `follow_edges (follower_id, followee_id)`, `pubkey_lookup (id, pubkey)`, `pubkey_freshness (id, status, last_fetched_at)`.
- Status domain + meaning: `discovered` (seen, unfetched — honest boundary), `fetched`, `not_found`, `failed`.
- The self-follow drop rule (D-08), bare-edge / id-resolution-at-boundary guidance (D-03/D-05), the recommended read-only consumer DB role granting SELECT on the three views only (V4), the TEXT-vs-enum status decision (Pitfall 5), and a `## Contract changes` changelog seeded with the initial Phase 1 entry (D-04).
- At least one example query per view.

## .sqlx Offline Metadata

`.sqlx/` (15 `query-*.json` files) is committed and **not** gitignored. Generated with `cargo sqlx prepare -- --all-targets` against an ephemeral Postgres 17 container. `SQLX_OFFLINE=true cargo build` and `SQLX_OFFLINE=true cargo build --all-targets` both succeed with no `DATABASE_URL`.

## Verification

Full suite green offline (`SQLX_OFFLINE=true cargo test`, testcontainers supplies the runtime DB):
- `tests/edge_diff.rs` — 4 passed: `upsert_pubkey_is_idempotent`, `edge_diff_writer`, `same_event_id_zero_touch`, `self_follow_dropped`.
- `tests/concurrency.rs` — 1 passed: `reader_and_writer_do_not_block`.
- `tests/contract.rs` — 3 passed (unchanged from 01-02).
- `tests/migrations.rs` — 2 passed (unchanged from 01-02).
- `tests/bootstrap.rs` — passed (unchanged from 01-01).
- `SQLX_OFFLINE=true cargo build` — exit 0; `--all-targets` — exit 0.

## Decisions Made
- Added `chrono` 0.4 (default-features off, `clock`) as a direct dependency so `DateTime<Utc>` can appear in the public store API signatures (the sqlx `chrono` feature was already enabled but doesn't re-export ergonomically for public signatures).
- Ran `cargo sqlx prepare -- --all-targets` (not just the default lib target) so the `query!` macros in the test files are also covered by offline metadata.
- `set_fetch_status` takes `status: &str` (the TEXT+CHECK representation from the migration); the DB CHECK constraint enforces the closed domain rather than a Rust enum, consistent with the D-09/Pitfall-5 decision from 01-02.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Test seed literal out of u8 range**
- **Found during:** Task 2 (edge_diff tests)
- **Issue:** `self_follow_dropped` used `pk(300)` as an event-id seed, but the `pk(seed: u8)` helper takes a `u8`; `300` overflows and failed to compile (`literal out of range for u8`).
- **Fix:** Changed the seed to `pk(33)` (a distinct, in-range value).
- **Files modified:** tests/edge_diff.rs
- **Verification:** `cargo test --test edge_diff` recompiles and all 4 tests pass.
- **Committed in:** `1d6e731` (Task 2 commit)

---

**Total deviations:** 1 auto-fixed (1 bug)
**Impact on plan:** Trivial test-only fix in code authored during this plan; no scope creep, no production-code change.

## Issues Encountered
- `cargo install sqlx-cli` first attempt failed on a transient crates.io network timeout (same flakiness noted in 01-01). Resolved by retrying with `CARGO_NET_RETRY=10 CARGO_HTTP_TIMEOUT=120`; the install completed. Environment-only, no checkpoint warranted (the package is the canonical, audited sqlx-cli). sqlx-cli is installed only to `~/.cargo` — no system modification.

## User Setup Required

None - no external service configuration required. (The crawler's live Postgres server is a later-phase operational concern; Phase 1 tests use ephemeral testcontainers Postgres.)

## Next Phase Readiness
- Phase 2 (ingest) can call `upsert_pubkey` to resolve pubkeys → ids and `apply_follow_list` to apply validated kind-3 events; the writer's idempotency + newest-wins columns (`applied_event_id`, `applied_created_at`) are wired and exercised.
- Phase 3 consumes and formally verifies the edge-diff writer against real validated events (D-16) — the API and its GRAPH-02 idempotency property are now in place.
- The spam-layer consumer has a committed, documented contract (`SCHEMA.md` + `COMMENT ON`) and a proven concurrent-read guarantee (GRAPH-03).
- No blockers introduced. Pre-existing blockers (curated relay coverage unknown until Phase 4; full-scale resource profile unmeasured) are unchanged.

## Known Stubs
None — the store layer, edge-diff writer, contract doc, and tests are all complete and exercised against real Postgres.

## Self-Check: PASSED
- Files: src/store/mod.rs, src/store/pubkeys.rs, src/store/follows.rs, src/lib.rs, SCHEMA.md, tests/edge_diff.rs, tests/concurrency.rs, .sqlx/ — all FOUND on disk.
- Commits: 21b45cc, 1d6e731, 4f0af1b — all FOUND in git log.
- Tests: 10 integration tests + doc-tests pass under SQLX_OFFLINE=true; offline build (lib + all-targets) exits 0.

---
*Phase: 01-schema-data-contract*
*Completed: 2026-06-12*
