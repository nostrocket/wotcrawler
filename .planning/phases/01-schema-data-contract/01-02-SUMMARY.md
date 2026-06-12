---
phase: 01-schema-data-contract
plan: 02
subsystem: graph-schema
tags: [postgres, sqlx, migration, contract-views, schema]
requires:
  - "01-01: Rust crate + sqlx 0.9 (migrate feature) + start_postgres() testcontainers fixture"
provides:
  - "migrations/0001_graph_schema.sql — idempotent versioned migration (pubkeys, follows, indexes, 3 contract views, COMMENT ON)"
  - "pubkeys table: surrogate bigint identity id, 32-byte bytea pubkey, TEXT+CHECK status, freshness/churn/applied columns"
  - "follows table: bare directed edges (follower_id, followee_id) bigint, composite PK, self-follow CHECK"
  - "Contract views: follow_edges, pubkey_lookup, pubkey_freshness — the spam-layer public API"
  - "Migration + contract integration tests (5 green against ephemeral Postgres)"
affects:
  - "Plan 03 (store layer + SCHEMA.md) writes through this schema and documents these views"
  - "Phase 2 kind:10002 / Phase 3 frontier / Phase 5 relay tables arrive as additive migrations on this base"
tech-stack:
  added: []
  patterns:
    - "Idempotent versioned migration (IF NOT EXISTS tables/indexes, CREATE OR REPLACE views)"
    - "TEXT + CHECK status (not native Postgres ENUM) — avoids sqlx enum-mapping pitfalls"
    - "BIGINT GENERATED ALWAYS AS IDENTITY surrogate ids"
    - "Stable contract views as the public API; base-table bookkeeping hidden"
    - "COMMENT ON labelling — PUBLIC CONTRACT vs INTERNAL — for introspectable contract"
    - "Parameterized sqlx queries only (regclass-cast bind for obj_description)"
key-files:
  created:
    - migrations/0001_graph_schema.sql
    - tests/migrations.rs
    - tests/contract.rs
  modified: []
decisions:
  - "Migration filename 0001_graph_schema.sql (sqlx-cli not installed; created the versioned file directly — prefix is illustrative per plan)"
  - "Status stored as TEXT + CHECK per RESEARCH Pitfall 5 / D-09 (A2)"
  - "Single migration file for all Phase 1 DDL per D-13 (A5)"
metrics:
  duration: ~2 min
  completed: 2026-06-12
---

# Phase 01 Plan 02: Graph Schema & Contract Views Summary

The Phase 1 PostgreSQL schema now exists as a single idempotent versioned migration (`migrations/0001_graph_schema.sql`) defining the `pubkeys` and `follows` tables with surrogate-bigint-id edges and per-pubkey freshness/churn/applied columns, the supporting reverse and partial-status indexes, three stable contract views (`follow_edges`, `pubkey_lookup`, `pubkey_freshness`), and `COMMENT ON` statements making the contract introspectable from psql — all proven by 5 green integration tests against ephemeral Postgres.

## Tasks Completed

| Task | Name | Commit | Files |
| ---- | ---- | ------ | ----- |
| 1 | Idempotent graph schema migration with contract views and comments | f5fe586 | migrations/0001_graph_schema.sql |
| 2 | Migration idempotency + schema-shape integration tests (TDD) | 3c4b027 | tests/migrations.rs |
| 3 | Contract-views and COMMENT ON integration tests (TDD) | 72ea39f | tests/contract.rs |

## Verification

- `cargo test --test migrations` — `2 passed; 0 failed` (`migrations_idempotent`, `schema_shape`).
- `cargo test --test contract` — `3 passed; 0 failed` (`contract_views_present`, `freshness_exposed_bookkeeping_hidden`, `contract_comments_present`).
- Migration grep checks: `CREATE TABLE IF NOT EXISTS pubkeys`/`follows` present; composite PK + self-follow CHECK present; status `TEXT ... CHECK (status IN (...))`; all three `CREATE OR REPLACE VIEW`; 3 `COMMENT ON VIEW`; 10 `PUBLIC CONTRACT:` labels; zero `CREATE INDEX CONCURRENTLY`; zero `CREATE TYPE`.
- Idempotency proven by `migrations_idempotent` running `sqlx::migrate!` twice and asserting `_sqlx_migrations` applied-count is unchanged on the second run.

## Schema Contract (for Plan 03 store layer + SCHEMA.md)

### Table `pubkeys`

| Column | Type | Notes |
| ------ | ---- | ----- |
| id | BIGINT GENERATED ALWAYS AS IDENTITY PK | surrogate id |
| pubkey | BYTEA NOT NULL UNIQUE | 32-byte x-only key |
| status | TEXT NOT NULL DEFAULT 'discovered' | CHECK IN ('discovered','fetched','not_found','failed') |
| last_fetched_at | TIMESTAMPTZ | FRESH-01 (D-09) |
| last_confirmed_at | TIMESTAMPTZ | FRESH-01 (D-09) |
| last_changed_at | TIMESTAMPTZ | FRESH-03 churn (D-10) |
| fetch_count | BIGINT NOT NULL DEFAULT 0 | FRESH-03 churn (D-10) |
| change_count | BIGINT NOT NULL DEFAULT 0 | FRESH-03 churn (D-10) |
| applied_event_id | BYTEA | INGEST-03 newest-wins (D-07) |
| applied_created_at | TIMESTAMPTZ | INGEST-03 (D-07) |

### Table `follows`

| Column | Type | Notes |
| ------ | ---- | ----- |
| follower_id | BIGINT NOT NULL REFERENCES pubkeys(id) | |
| followee_id | BIGINT NOT NULL REFERENCES pubkeys(id) | |

- `PRIMARY KEY (follower_id, followee_id)` — forward-lookup index via leading column.
- `CHECK (follower_id <> followee_id)` — self-follow guard (D-08).

### Indexes

- `follows_followee_idx ON follows (followee_id)` — reverse / in-degree.
- `pubkeys_status_idx ON pubkeys (status) WHERE status IN ('discovered','not_found','failed')` — partial, Phase 4 staleness scanner.

### Contract views (public API)

| View | Columns | Purpose |
| ---- | ------- | ------- |
| follow_edges | (follower_id, followee_id) | bare directed edges, hot path (D-03/D-05) |
| pubkey_lookup | (id, pubkey) | id -> 32-byte resolution boundary (D-03) |
| pubkey_freshness | (id, status, last_fetched_at) | honest aging; exposes discovered rows, hides bookkeeping (D-11/D-12) |

All three views and their contract columns carry `PUBLIC CONTRACT:` `COMMENT ON`; internal columns (`applied_event_id`, `applied_created_at`, `last_changed_at`, `fetch_count`, `change_count`) carry `INTERNAL:` comments and are absent from the contract views.

## Migration Filename

`migrations/0001_graph_schema.sql` — applied by `sqlx::migrate!("./migrations")`. sqlx-cli is not installed; the versioned file was created directly (the `0001_` prefix is illustrative per the plan, and sqlx applies versioned files in lexical order).

## Deviations from Plan

None — plan executed exactly as written. Task 1's `<verify>` clause anticipated the test file landing in Task 2 (`|| echo "test file lands in Task 2"`); accordingly Task 1 was verified by acceptance-criteria grep and committed before the test files existed, then Tasks 2 and 3 added the green tests.

## Checkpoint / Auth Gates

None — fully autonomous plan, no checkpoints, no auth gates.

## Known Stubs

None — the migration, tables, views, comments, and tests are all complete and exercised.

## Self-Check: PASSED

- Files: migrations/0001_graph_schema.sql, tests/migrations.rs, tests/contract.rs all FOUND on disk.
- Commits: f5fe586, 3c4b027, 72ea39f all FOUND in git log.
- Tests: 5 passed, 0 failed across both test files.
