---
phase: 03-graph-writer-bfs-frontier
plan: 01
subsystem: database
tags: [postgres, sqlx, migration, frontier, nostr, testing]

# Dependency graph
requires:
  - phase: 01-graph-schema
    provides: "migrations/0001_graph_schema.sql (pubkeys.status TEXT+CHECK, pubkey_freshness contract view, INTERNAL column COMMENT convention)"
  - phase: 02-relay-ingest
    provides: "ValidatedFollowList output contract; offline event fixtures + ScriptedRelay used by Wave 0 scaffolds"
provides:
  - "Additive migration 0002_frontier.sql: widened status domain with transient 'in_progress' lease state"
  - "pubkeys.claimed_at + pubkeys.fetch_attempts internal lease/retry columns"
  - "pubkey_freshness redefined to collapse 'in_progress' -> 'discovered' (contract domain stays 4-valued)"
  - "Wave 0 test scaffolds tests/graph_writer.rs (GRAPH-02) and tests/frontier.rs (CRAWL-01..04, FRESH-01)"
affects: [03-02-frontier-module, 03-03-test-bodies, 04-observability, 05-relay-health]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Additive idempotent migration: DROP CONSTRAINT IF EXISTS + named ADD CONSTRAINT to widen a CHECK domain (cannot edit in place)"
    - "Contract-view collapse: CASE expression hides a transient internal status value from the public view"
    - "Wave 0 test scaffold: named #[ignore] stubs map 1:1 to the requirement test map so subsequent plans fill bodies rather than create files"

key-files:
  created:
    - "migrations/0002_frontier.sql"
    - "tests/graph_writer.rs"
    - "tests/frontier.rs"
  modified:
    - "tests/migrations.rs"

key-decisions:
  - "Auto-generated CHECK name confirmed as pubkeys_status_check (verified via pg_constraint against a 0001-migrated DB) — DROP uses the real name"
  - "'in_progress' collapsed to 'discovered' in pubkey_freshness so the public contract domain stays {discovered, fetched, not_found, failed}"
  - "No 'in_progress' partial index added — claim scan reads only 'discovered', startup reclaim is a one-time scan (RESEARCH A3)"
  - "No .sqlx regeneration needed — plan adds no new query!/query_scalar! macros in src/ (raw-SQL migration; tests use untyped sqlx::query); prepare confirmed zero drift"

patterns-established:
  - "Widen a Postgres CHECK domain via DROP CONSTRAINT IF EXISTS + explicitly-named ADD CONSTRAINT for clean re-runs"
  - "Hide transient internal lifecycle states from the public contract view with a CASE collapse, re-issuing the PUBLIC CONTRACT COMMENT after CREATE OR REPLACE VIEW"

requirements-completed: [GRAPH-02, CRAWL-03, FRESH-01]

# Metrics
duration: 13min
completed: 2026-06-13
---

# Phase 3 Plan 01: Frontier Migration & Wave 0 Test Scaffolds Summary

**Additive migration 0002 turns pubkeys.status into a crash-safe DB-resident BFS frontier (adds the transient 'in_progress' lease state, claimed_at + fetch_attempts columns, hides 'in_progress' from the contract view) and lays down named #[ignore] Wave 0 test scaffolds for every Phase 3 requirement.**

## Performance

- **Duration:** 13 min
- **Started:** 2026-06-13T13:51:00Z
- **Completed:** 2026-06-13T14:04:06Z
- **Tasks:** 4
- **Files modified:** 4 (3 created, 1 modified)

## Accomplishments
- `migrations/0002_frontier.sql`: idempotent additive migration widening the status CHECK to include `in_progress`, adding `claimed_at` (TIMESTAMPTZ) and `fetch_attempts` (SMALLINT NOT NULL DEFAULT 0), and redefining `pubkey_freshness` to collapse `in_progress` -> `discovered`.
- Confirmed the live auto-generated CHECK constraint name is `pubkeys_status_check` (verified via `pg_constraint` against a freshly 0001-migrated Postgres) before finalizing the DROP.
- Extended `tests/migrations.rs` with `migration_0002_widens_status_and_hides_in_progress`: proves the widened domain accepts `in_progress`, the view collapses it to `discovered`, and the internal columns are absent from the contract view (exactly `id, status, last_fetched_at`).
- Created `tests/graph_writer.rs` and `tests/frontier.rs` as compiling Wave 0 scaffolds with all 10 named `#[ignore]` stubs mapping 1:1 to the RESEARCH Test Map.
- Verified offline build green and `.sqlx/` metadata unchanged (no drift).

## Task Commits

Each task was committed atomically:

1. **Task 1: Verify CHECK constraint name + write frontier migration** - `c2ccb8c` (feat)
2. **Task 2: Extend migration idempotency test for 0002** - `9626262` (test)
3. **Task 3: Scaffold tests/graph_writer.rs and tests/frontier.rs** - `4018767` (test)
4. **Task 4: Verify .sqlx offline metadata** - no commit (verification-only; `cargo sqlx prepare` reported zero drift, offline build green)

**Plan metadata:** committed with this SUMMARY (docs: complete plan)

## Files Created/Modified
- `migrations/0002_frontier.sql` - Additive frontier migration (status domain widen, claimed_at + fetch_attempts, pubkey_freshness collapse, re-issued contract COMMENTs).
- `tests/migrations.rs` - Added `migration_0002_widens_status_and_hides_in_progress` (widened domain + collapse + hidden internal columns).
- `tests/graph_writer.rs` - GRAPH-02 Wave 0 scaffold: `apply_diff_adds_and_removes`, `same_event_zero_touch`, `newest_wins_under_concurrent_apply`.
- `tests/frontier.rs` - Frontier Wave 0 scaffold: `bfs_reaches_full_component`, `spam_island_never_crawled`, `crash_resume_no_redo`, `startup_reclaims_in_progress`, `bounded_concurrency`, `skip_locked_no_double_claim`, `last_fetched_at_stamped_on_terminal`.

## Decisions Made
- Verified the live CHECK constraint name (`pubkeys_status_check`) against a real 0001-migrated DB rather than trusting the assumed name — matched expectation (RESEARCH A1).
- Collapsed `in_progress` to `discovered` in `pubkey_freshness` (Open Question 2 recommendation) so the documented public contract domain stays four-valued and the spam layer never sees the transient lease state.
- Did not add an `in_progress` partial index (RESEARCH A3 — claim reads only `discovered`, reclaim is a one-time startup scan).
- Scaffold stubs import only existing surfaces (`store`, `apply_follow_list`, `upsert_pubkey`); `web_of_trust::crawl` is referenced only in doc-comment prose (the module lands in 03-02), so both suites compile today.

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered
- The `schema_shape` test in `tests/migrations.rs` failed once with a testcontainers "container does not expose port 5432/tcp" error — a known testcontainers/Docker startup race, unrelated to migration 0002. Re-running the suite passed all tests (the parallel `migrations_idempotent` test, which exercises 0002, passed on both runs). No code change required.

## Threat Model Coverage
- **T-03-01 (Tampering / DDL re-run):** migration is additive + idempotent (named DROP+ADD, ADD COLUMN IF NOT EXISTS); `migrations_idempotent` proves the re-run no-op covers 0002.
- **T-03-02 (Information disclosure / contract view):** `in_progress` collapsed to `discovered`, internal columns absent — verified by `migration_0002_widens_status_and_hides_in_progress`.
- **T-03-03 (Tampering / SQL injection in tests):** all new test queries use `$1` bind parameters; no string-formatted SQL.
- **T-03-SC (cargo installs):** no packages added this plan.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- The schema delta every later Phase 3 task builds on is in place: the claim query can now read/write `in_progress`, requeue can bump `fetch_attempts`, and the lease can stamp `claimed_at`.
- Both Wave 0 test files exist as compiling, named, ignored scaffolds — 03-02 implements the `crawl` module and 03-03 fills the stub bodies (un-ignoring each as its behavior lands).
- No blockers introduced.

## Self-Check: PASSED

---
*Phase: 03-graph-writer-bfs-frontier*
*Completed: 2026-06-13*
