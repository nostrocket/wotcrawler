---
phase: 01-schema-data-contract
verified: 2026-06-12T07:18:38Z
status: passed
score: 11/11 must-haves verified
overrides_applied: 0
human_verification_resolved: 2026-06-12T07:45:00Z via 01-UAT.md (both items confirmed pass by user)
human_verification:
  - test: "Run the full integration test suite against Docker (cargo test -- requires Docker daemon)"
    expected: "10 tests pass (bootstrap, migrations x2, contract x3, edge_diff x4, concurrency x1)"
    why_human: "Tests require a live Docker daemon and a running Postgres container; cannot be confirmed from offline/static analysis alone. The orchestrator reports 10/0 pass/fail but this verifier cannot re-run Docker tests in this environment."
  - test: "Verify concurrency test actually exercises the writer path (CR-02 / WR-06)"
    expected: "After reader_and_writer_do_not_block, at least one row exists in follows and fetch_count > 0 on the follower pubkey, confirming the writer made progress and the reader faced a live writer"
    why_human: "The writer loop uses `let _ = apply_follow_list(...).await` — errors are silently discarded. If the writer fails on its first call, the reader still completes without a hang, so the test passes vacuously. Manually inspect: add a post-test assertion that fetch_count > 0 or watch the test output for any writer panics."
---

# Phase 1: Schema & Data Contract Verification Report

**Phase Goal:** The shared PostgreSQL database the spam layer consumes exists with its schema established as the documented public contract, and the crawler can read/write it concurrently with other processes.
**Verified:** 2026-06-12T07:18:38Z
**Status:** passed (human verification resolved via 01-UAT.md, 2026-06-12)
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths

Verified against all four roadmap Success Criteria plus all plan must-haves from plans 01-01, 01-02, and 01-03.

| #  | Truth | Status | Evidence |
|----|-------|--------|----------|
| 1  | A fresh database migrates from empty via versioned migrations; re-running is a no-op | VERIFIED | `migrations/0001_graph_schema.sql` exists; `tests/migrations.rs::migrations_idempotent` runs `sqlx::migrate!` twice and asserts the `_sqlx_migrations` applied-count is unchanged on the second run |
| 2  | Schema stores pubkeys with surrogate bigint IDs, directed follow edges keyed on those IDs, and per-pubkey freshness columns | VERIFIED | Migration defines `pubkeys` with `BIGINT GENERATED ALWAYS AS IDENTITY`, `follows(follower_id, followee_id)` bigint FK references, and all 7 freshness/churn/applied columns; confirmed by `schema_shape` test |
| 3  | A second process can run read queries against the graph while the crawler's store layer writes, with neither blocking the other | VERIFIED (with caveat — see human check) | `tests/concurrency.rs::reader_and_writer_do_not_block` uses two distinct `PgPool`s; reader runs 100 `follow_edges` SELECTs with a 5-second timeout while a writer loops `apply_follow_list`; caveat: writer errors are silently swallowed via `let _ = apply_follow_list(...)` |
| 4  | A committed schema document describes every table and column a downstream consumer reads, sufficient to query without reading crawler code | VERIFIED | `SCHEMA.md` at repo root documents `follow_edges`, `pubkey_lookup`, `pubkey_freshness` with column types/semantics, status domain, self-follow rule, read-only consumer role, TEXT-vs-enum decision, and `## Contract changes` changelog; `COMMENT ON` in migration mirrors this |
| 5  | `cargo build` succeeds on toolchain >= 1.94 | VERIFIED | `SQLX_OFFLINE=true cargo build` exits 0 in this session; `rust-toolchain.toml` pins `channel = "1.94.0"`; `Cargo.toml` sets `rust-version = "1.94"` |
| 6  | Project compiles with sqlx 0.9, tokio, thiserror, anyhow, config as dependencies | VERIFIED | `Cargo.toml` contains exact locked versions; no diesel, sea-orm, or sqlx `uuid` feature present |
| 7  | An integration test can spin an ephemeral Postgres via testcontainers | VERIFIED | `tests/common/mod.rs` calls `Postgres::default().start().await?` and returns container handle + URL; wired into all 5 integration test files |
| 8  | The store layer connects a PgPool and runs migrations programmatically | VERIFIED | `src/store/mod.rs` exports `connect()` (PgPoolOptions) and `run_migrations()` calling `sqlx::migrate!("./migrations").run(pool)` |
| 9  | `upsert_pubkey` returns a stable surrogate id for a 32-byte pubkey (get-or-create) | VERIFIED | `src/store/pubkeys.rs` validates 32-byte length, runs `INSERT ... ON CONFLICT DO UPDATE ... RETURNING id`; `upsert_pubkey_is_idempotent` test asserts same id returned and no duplicate row |
| 10 | `apply_follow_list` applies a replacing kind-3 as insert-added + delete-removed in one transaction; same `applied_event_id` touches zero edge rows | VERIFIED | `src/store/follows.rs` lines 94-139: `pool.begin()`, DELETE removed, INSERT added ON CONFLICT DO NOTHING, freshness UPDATE, `tx.commit()`; idempotency short-circuit at lines 48-70 touches zero edges; proven by `edge_diff_writer` and `same_event_id_zero_touch` tests |
| 11 | Self-follows are dropped before the edge diff | VERIFIED | `src/store/follows.rs` lines 74-78 filter `id != follower_id` before the diff computation; proven by `self_follow_dropped` test and DB-level `CHECK (follower_id <> followee_id)` |

**Score:** 11/11 truths verified

### Deferred Items

None — all Phase 1 success criteria are met or confirmed within Phase 1 scope.

Note: CR-01 (newest-wins `created_at` comparison) and CR-02 (idempotency check outside transaction) found in `01-REVIEW.md` are real correctness concerns but are **out-of-scope for Phase 1**:
- CR-01: Newest-wins enforcement is INGEST-03, explicitly assigned to Phase 2.
- CR-02: Concurrent-apply correctness is a Phase 2/3 concern (Phase 1 has no multi-relay ingestion pipeline). The `applied_event_id`-based idempotency (the Phase 1 must-have) does work correctly for single-caller use.

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `rust-toolchain.toml` | Pinned Rust toolchain >= 1.94 | VERIFIED | Contains `channel = "1.94.0"` |
| `Cargo.toml` | Crate manifest with locked deps, rust-version = 1.94, sqlx features | VERIFIED | `rust-version = "1.94"`, sqlx 0.9.0 with correct features, no diesel/sea-orm/uuid |
| `src/lib.rs` | Crate root re-exporting store and error modules | VERIFIED | Exports `pub mod error`, `pub mod store`, `pub use error::StoreError`; 9 lines (> 2) |
| `src/error.rs` | thiserror StoreError enum with `Sqlx` and `Migrate` variants | VERIFIED | Contains `StoreError` with `#[from] sqlx::Error`, `#[from] sqlx::migrate::MigrateError`, and `InvalidPubkey(usize)` |
| `tests/common/mod.rs` | Shared testcontainers Postgres bootstrap | VERIFIED | `Postgres::default().start().await?`, returns container + URL |
| `migrations/0001_graph_schema.sql` | Idempotent DDL: pubkeys, follows, indexes, contract views, COMMENT ON | VERIFIED | All required structures present; `CREATE TABLE IF NOT EXISTS`, `CREATE OR REPLACE VIEW`, 10 `PUBLIC CONTRACT:` labels |
| `tests/migrations.rs` | Migrate-from-empty + re-run no-op + schema-shape tests | VERIFIED | Contains `migrations_idempotent` and `schema_shape` |
| `tests/contract.rs` | Contract-views-present + COMMENT ON + freshness-exposed/bookkeeping-hidden | VERIFIED | Contains `contract_views_present`, `freshness_exposed_bookkeeping_hidden`, `contract_comments_present` |
| `src/store/follows.rs` | Transactional edge-diff writer `apply_follow_list` | VERIFIED | 143 lines; `apply_follow_list` present; transaction with `begin()`/`commit()` |
| `src/store/pubkeys.rs` | `upsert_pubkey` + `set_fetch_status` | VERIFIED | Both functions present; 32-byte validation; parameterized queries |
| `src/store/mod.rs` | `connect(PgPool)` + `run_migrations` | VERIFIED | Both functions present; `sqlx::migrate!` call |
| `SCHEMA.md` | Public contract document with all required sections | VERIFIED | Documents all three views, status semantics, self-follow rule, read-only role, Contract changes changelog |
| `tests/concurrency.rs` | Reader+writer non-blocking integration test | VERIFIED (with caveat) | `reader_and_writer_do_not_block` present; two distinct pools; 100 reads with 5s timeout; writer errors silently discarded |
| `tests/edge_diff.rs` | Edge-diff add/remove/zero-touch + self-follow-drop tests | VERIFIED | Contains `edge_diff_writer`, `same_event_id_zero_touch`, `self_follow_dropped`, `upsert_pubkey_is_idempotent` |
| `.sqlx/` (15 query metadata files) | Committed offline query metadata | VERIFIED | 15 `query-*.json` files present; `.gitignore` only ignores `/target`, not `.sqlx/` |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `tests/common/mod.rs` | `testcontainers_modules::postgres::Postgres` | `Postgres::default()` call | WIRED | Line 17: `Postgres::default().start().await?` |
| `tests/migrations.rs` | `migrations/0001_graph_schema.sql` | `sqlx::migrate!` runner | WIRED | Lines 22, 35: `sqlx::migrate!("./migrations").run(&pool)` |
| `follows.followee_id` | `pubkeys.id` | FK + reverse index | WIRED | Migration lines 32-33: `REFERENCES pubkeys(id)`; `follows_followee_idx` on line 40 |
| `src/store/follows.rs` | `follows` table | `pool.begin()` transaction | WIRED | Line 94: `let mut tx = pool.begin().await?`; DELETE+INSERT execute on `&mut *tx` |
| `tests/concurrency.rs` | `follow_edges` view | reader pool SELECT | WIRED | Line 69: `SELECT follower_id, followee_id FROM follow_edges LIMIT 1000` |
| `src/store/mod.rs` | `migrations/0001_graph_schema.sql` | `sqlx::migrate!` runner | WIRED | Line 44: `sqlx::migrate!("./migrations").run(pool).await?` |

### Data-Flow Trace (Level 4)

| Artifact | Data Variable | Source | Produces Real Data | Status |
|----------|---------------|--------|--------------------|--------|
| `src/store/follows.rs` (edge-diff writer) | `current_rows` / `added` / `removed` | `SELECT followee_id FROM follows WHERE follower_id = $1` on pool + `DELETE`/`INSERT` on tx | Yes — real Postgres query; tx writes real rows | FLOWING |
| `src/store/pubkeys.rs` (upsert) | surrogate `id` | `INSERT ... ON CONFLICT ... RETURNING id` | Yes — DB-generated bigint id | FLOWING |
| `tests/concurrency.rs` | `rows` from `follow_edges` | Live Postgres SELECT while writer upserts | Yes — queries real rows (may be empty mid-diff; that is expected) | FLOWING |

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| Offline build compiles | `SQLX_OFFLINE=true cargo build` | Finished dev profile in 0.35s | PASS |
| All 11 test targets enumerable | `SQLX_OFFLINE=true cargo test -- --list` | 11 tests listed across 5 files | PASS |
| `apply_follow_list` function exists | `grep -n "apply_follow_list" src/store/follows.rs` | Line 40: function definition | PASS |
| `run_migrations` calls `migrate!` | `grep -n "migrate!" src/store/mod.rs` | Line 44 | PASS |
| `SCHEMA.md` has Contract changes | `grep "Contract changes" SCHEMA.md` | Lines 15 and 173 | PASS |
| `.sqlx/` has 15 files | `ls .sqlx/` | 15 query-*.json files | PASS |
| No debt markers in source | `grep -rn "TBD\|FIXME\|XXX" src/ migrations/` | No matches | PASS |

### Probe Execution

No probes declared or present (`scripts/` directory does not exist). Step 7c: SKIPPED.

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|------------|-------------|--------|----------|
| GRAPH-01 | 01-01-PLAN.md, 01-02-PLAN.md | PostgreSQL schema stores pubkeys (surrogate bigint ids), directed follow edges, and per-pubkey freshness metadata, with versioned migrations | SATISFIED | `migrations/0001_graph_schema.sql` with `CREATE TABLE IF NOT EXISTS pubkeys/follows`; `tests/migrations.rs` green |
| GRAPH-03 | 01-03-PLAN.md | A separate process (the spam layer) can read the graph concurrently while the crawler writes, without coordination | SATISFIED (with caveat) | `tests/concurrency.rs::reader_and_writer_do_not_block` uses two distinct `PgPool`s proxying separate-process isolation; MVCC guarantee; see human check for writer-error-suppression caveat |
| GRAPH-04 | 01-02-PLAN.md, 01-03-PLAN.md | The schema is documented as the public contract for downstream consumers | SATISFIED | `SCHEMA.md` committed at repo root; `COMMENT ON` in migration; both document all three contract views with column semantics |

All three requirements mapped to Phase 1 are satisfied. GRAPH-02 (replacing kind-3 applied as transactional edge diff) is assigned to Phase 3 per REQUIREMENTS.md.

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| `tests/concurrency.rs` | 53 | `let _ = apply_follow_list(...).await` — writer errors silently discarded | WARNING | Concurrency test could pass vacuously if writer always fails; GRAPH-03 proof is weakened. Not a blocker because the reader-side of the proof (100 reads with 5s timeout) still exercises MVCC against a live Postgres instance. See human check #2. |
| `src/lib.rs` | 3 | `"(Plan 03)"` in rustdoc comment — internal planning artifact in public docs | INFO | Non-blocking; leaks planning artifact numbering into rustdoc. Noted in code review IN-03. |

No `TBD`, `FIXME`, or `XXX` debt markers found in any phase-modified file.

### Human Verification Required

#### 1. Full Integration Test Suite Execution

**Test:** Run `cargo test` (with Docker available) against all integration tests and confirm all 10 pass.

**Expected:** Output shows: 1 passed (bootstrap), 2 passed (migrations), 3 passed (contract), 4 passed (edge_diff), 1 passed (concurrency) — total 10/0.

**Why human:** Tests require a live Docker daemon and Postgres container. The orchestrator reports 10 passed/0 failed from a prior run, but this verifier cannot re-execute Docker-dependent tests in the current environment. The static analysis confirms all test functions exist and are substantively implemented.

#### 2. Concurrency Test Writer Progress Verification

**Test:** Add a temporary assertion after the reader loop in `tests/concurrency.rs` before `writer.abort()`:
```rust
let fetches: i64 = sqlx::query_scalar!(
    "SELECT fetch_count FROM pubkeys WHERE id = $1", follower
).fetch_one(&writer_pool).await?;
assert!(fetches > 0, "writer made no progress — GRAPH-03 proof is vacuous");
```
Run `cargo test --test concurrency -- --nocapture` and confirm the assertion passes (fetch_count > 0).

**Expected:** The assertion passes, confirming the writer actually wrote rows while readers were querying.

**Why human:** The current concurrency test at line 53 uses `let _ = apply_follow_list(...).await` — writer errors are silently discarded. If `apply_follow_list` fails on every iteration (e.g., due to a schema regression, FK violation, or pool exhaustion), the test still passes because the 100 reader queries return Ok without blocking. The GRAPH-03 proof is only sound if the writer actually made progress. This is code review finding WR-06.

### Gaps Summary

No technical blockers were found. All 11 must-have truths are VERIFIED through static artifact analysis and offline build verification.

The two code review critical findings (CR-01: newest-wins not enforced; CR-02: idempotency check outside transaction) do not fail any Phase 1 must-have:
- CR-01 (newest-wins) maps to INGEST-03, which is explicitly Phase 2.
- CR-02 (concurrent same-follower race) is a Phase 2/3 correctness concern; the Phase 1 idempotency short-circuit (same event id → zero touches) works correctly for single-caller scenarios.

The `human_needed` status is driven by WR-06: the concurrency test's writer error suppression makes the GRAPH-03 success criterion's proof unsound without human inspection confirming the writer made progress during the test run.

---

_Verified: 2026-06-12T07:18:38Z_
_Verifier: Claude (gsd-verifier)_
