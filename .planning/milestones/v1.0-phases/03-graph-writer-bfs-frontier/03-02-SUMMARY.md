---
phase: 03-graph-writer-bfs-frontier
plan: 02
subsystem: crawl
tags: [postgres, sqlx, frontier, skip-locked, crawl, nostr]

# Dependency graph
requires:
  - phase: 03-graph-writer-bfs-frontier
    plan: 01
    provides: "migration 0002 (status domain widened with 'in_progress', claimed_at + fetch_attempts columns, pubkey_freshness collapse); tests/frontier.rs Wave 0 scaffold"
  - phase: 01-graph-schema
    provides: "pubkeys table, pubkeys_status_idx partial index, set_fetch_status / upsert_pubkey store primitives"
provides:
  - "src/crawl module registered via pub mod crawl"
  - "frontier::seed_anchor (D-03 anchor seed via upsert_pubkey)"
  - "frontier::claim_batch (FOR UPDATE SKIP LOCKED CTE in a short txn, D-04/D-07)"
  - "frontier::reclaim_stale_on_startup (in_progress -> discovered sweep, D-06)"
  - "frontier::requeue_or_fail (fetch_attempts cap + FRESH-01 terminal stamping, D-09/D-11)"
  - "ClaimedAuthor { id, pubkey } claim result struct"
  - "crawl::{DEFAULT_BATCH_SIZE, DEFAULT_CONCURRENCY, DEFAULT_MAX_ATTEMPTS} documented defaults"
affects: [03-03-crawl-loop, 04-observability, 04-staleness-loop]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "FOR UPDATE SKIP LOCKED claim CTE in its OWN short transaction so the row lock releases at commit, not during the fetch (D-04, Pitfall 6)"
    - "Single-UPDATE branch-on-bumped-value: SET fetch_attempts = fetch_attempts + 1 then CASE on (fetch_attempts + 1) to atomically decide requeue-vs-terminal without a read-modify-write race"
    - "$2::int2 cast in SQL so an i16 bind matches a SMALLINT column (fetch_attempts + 1 alone promotes to int4 and would force an i32 bind)"

key-files:
  created:
    - "src/crawl/mod.rs"
    - "src/crawl/frontier.rs"
  modified:
    - "src/lib.rs"
    - "tests/frontier.rs"
    - ".sqlx/ (3 new query metadata files)"

key-decisions:
  - "No fetch_attempts helper added to src/store/pubkeys.rs — the requeue branch is a single self-contained UPDATE in frontier.rs, so an extra store helper added indirection without clarity (plan made it optional)"
  - "requeue_or_fail is one atomic UPDATE that branches on (fetch_attempts + 1) via CASE, avoiding a read-then-write race and routing the terminal `failed` path through a last_fetched_at stamp (FRESH-01)"
  - "An under-cap requeue clears claimed_at (lease released) but deliberately does NOT touch last_fetched_at — the knowledge was retried, not refreshed"
  - "kept the 4 end-to-end crawl-loop tests (bfs_reaches_full_component, crash_resume_no_redo, bounded_concurrency, last_fetched_at_stamped_on_terminal) as #[ignore] scaffolds for 03-03; this plan fills only the frontier-module tests"

patterns-established:
  - "DB-resident work-queue claim: FOR UPDATE SKIP LOCKED CTE + RETURNING in a dedicated short transaction; lock released before the long downstream work"
  - "Terminal-status transition that bumps a retry counter and stamps freshness in one UPDATE via CASE on the post-increment value"

requirements-completed: [CRAWL-01, CRAWL-02, CRAWL-03, FRESH-01]

# Metrics
duration: 9min
completed: 2026-06-13
---

# Phase 3 Plan 02: BFS Frontier Queue Primitives Summary

**Built the `src/crawl::frontier` queue/lease mechanics over `pubkeys.status` — seed the anchor (D-03), batch-claim `discovered` rows with `FOR UPDATE SKIP LOCKED` in a short lease transaction (D-04/D-07), reclaim crash-orphaned `in_progress` leases at startup (D-06), and resolve transient-vs-terminal retries with a `fetch_attempts` cap that stamps `last_fetched_at` on the terminal `failed` path (D-09/D-11/FRESH-01) — all proven by 7 passing integration tests.**

## Performance

- **Duration:** ~9 min
- **Started:** 2026-06-13T14:06:57Z
- **Completed:** 2026-06-13T14:15:30Z
- **Tasks:** 2
- **Files modified:** 5 (2 created, 2 modified, 3 new .sqlx metadata files)

## Accomplishments
- `src/crawl/mod.rs`: new crawl module entry with a requirement-naming doc-block (CRAWL-01..04 + FRESH-01), `pub mod frontier;`, and three constant-with-rationale defaults — `DEFAULT_BATCH_SIZE` (64, near a typical NIP-11 author-chunk), `DEFAULT_CONCURRENCY` (8), `DEFAULT_MAX_ATTEMPTS` (3, per D-09). The bounded worker loop that consumes them is explicitly deferred to 03-03 (documented in-module).
- `src/crawl/frontier.rs`: `seed_anchor` (verbatim `upsert_pubkey`, no new SQL), `claim_batch` (claim CTE `WHERE status='discovered' ... FOR UPDATE SKIP LOCKED` in its own `pool.begin()`/`commit()` short transaction, `RETURNING p.id, p.pubkey`), `reclaim_stale_on_startup` (one-shot `in_progress -> discovered`, `claimed_at = NULL`, returns `rows_affected()`), and `requeue_or_fail` (single atomic UPDATE bumping `fetch_attempts` and branching on the post-increment value to either requeue to `discovered` or transition terminal `failed` with `last_fetched_at` stamped). Reuses `StoreError` — no new error enum. `ClaimedAuthor { id, pubkey: Vec<u8> }` carries the claim result.
- `src/lib.rs`: registered `pub mod crawl;`.
- `tests/frontier.rs`: filled 7 frontier-module tests, all passing — `seed_anchor_lands_discovered`, `claim_never_returns_fetched`, `skip_locked_no_double_claim` (two separate pools, timeout-guarded, disjoint id sets covering all rows exactly once), `startup_reclaims_in_progress`, `requeue_under_cap_returns_to_discovered`, `requeue_at_cap_sets_failed_and_stamps`, `spam_island_never_crawled` (claim-level portion). Kept the 4 crawl-loop tests as `#[ignore]` 03-03 scaffolds.
- `.sqlx/`: regenerated offline metadata for the claim CTE, reclaim sweep, and requeue UPDATE; `SQLX_OFFLINE=true cargo build --all-targets` exits 0.

## Task Commits

1. **Task 1: Implement the frontier queue module (seed, claim, reclaim, requeue)** — `fae3444` (feat)
2. **Task 2: Regenerate and commit .sqlx offline metadata** — `884c09a` (chore)

**Plan metadata:** committed with this SUMMARY (docs: complete plan)

## Files Created/Modified
- `src/crawl/mod.rs` (created) — crawl module entry, `pub mod frontier;`, documented `DEFAULT_BATCH_SIZE` / `DEFAULT_CONCURRENCY` / `DEFAULT_MAX_ATTEMPTS`.
- `src/crawl/frontier.rs` (created) — `seed_anchor`, `claim_batch`, `reclaim_stale_on_startup`, `requeue_or_fail`, `ClaimedAuthor`.
- `src/lib.rs` (modified) — `pub mod crawl;` registration + doc line.
- `tests/frontier.rs` (modified) — 7 frontier-module test bodies; 4 ignored 03-03 scaffolds retained.
- `.sqlx/query-d3d0ca52...json` (claim CTE), `.sqlx/query-efa182d0...json` (reclaim sweep), `.sqlx/query-a312a681...json` (requeue UPDATE) — new offline metadata.

## Decisions Made
- Did not add a `fetch_attempts` bump helper to `src/store/pubkeys.rs` — the plan made it optional, and the requeue branch is a single self-contained UPDATE in `frontier.rs`, so a store helper would have added indirection without clarity.
- `requeue_or_fail` is one atomic UPDATE branching on `(fetch_attempts + 1)` via CASE, avoiding a read-modify-write race and routing the terminal `failed` write through a `last_fetched_at` stamp (FRESH-01 / Pitfall 5). The `$2::int2` cast keeps the `i16 max_attempts` bind aligned with the SMALLINT column after the int4 promotion of `fetch_attempts + 1`.
- An under-cap requeue clears `claimed_at` (lease released) but leaves `last_fetched_at` untouched — the list was retried, not refreshed; only terminal states record freshness.

## Deviations from Plan

None — plan executed exactly as written. (The optional `src/store/pubkeys.rs` helper was deliberately not added per the decision above; the plan listed it as optional, so this is not a deviation.)

## Issues Encountered
- The first full `cargo test --test frontier` run failed 3 of the 7 tests with `failed to create a container: Timeout error` — the known testcontainers/Docker container-creation race (7 tests each spawn their own Postgres concurrently, competing with other running Docker containers on the host). Re-running with `--test-threads=2` (fewer simultaneous container creations) passed all 7. No code change required — exactly the flake the environment note flagged.
- A first `cargo sqlx prepare` failed to compile (`expected i32, found i16`) because `fetch_attempts + 1` promotes to int4, inferring `$2` as `i32`. Fixed by casting `$2::int2` in SQL so the planned `i16 max_attempts` signature is preserved (Rule 3 — blocking type issue, fixed inline).

## Threat Model Coverage
- **T-03-04 (Tampering / concurrent claim race):** `claim_batch` uses `FOR UPDATE SKIP LOCKED`; `skip_locked_no_double_claim` (two separate pools) proves disjoint id sets with each row claimed exactly once and neither call blocking.
- **T-03-05 (DoS / long-held claim transaction):** the claim is its own `pool.begin()`/`commit()` short transaction; the lock releases at commit, before any fetch.
- **T-03-06 (Repudiation / terminal status without timestamp):** `requeue_or_fail` stamps `last_fetched_at` on the terminal `failed` path; `requeue_at_cap_sets_failed_and_stamps` asserts it is non-NULL.
- **T-03-07 (DoS / unbounded retry loop):** `fetch_attempts` cap transitions to terminal `failed`; `requeue_at_cap_sets_failed_and_stamps` proves a flaky pubkey terminates rather than bouncing.
- **T-03-08 (EoP / re-fetching completed work):** the claim selects only `status='discovered'`; `claim_never_returns_fetched` asserts a `fetched` row is never claimed.
- **T-03-SC (cargo installs):** no packages added this plan.

## User Setup Required
None — no external service configuration required.

## Next Phase Readiness
- The frontier primitives 03-03 composes are in place: `seed_anchor` to root the BFS, `claim_batch` to lease work, `reclaim_stale_on_startup` for the crash-resume sweep, and `requeue_or_fail` for the per-author transient-vs-terminal resolution. The `DEFAULT_*` knobs are documented and ready for the bounded worker loop.
- 4 end-to-end crawl-loop tests remain as `#[ignore]` scaffolds in `tests/frontier.rs` for 03-03 to fill (BFS reachability, crash-resume no-redo, bounded concurrency, full-loop terminal stamping).
- No blockers introduced.

## Self-Check: PASSED
- `src/crawl/mod.rs` — FOUND
- `src/crawl/frontier.rs` — FOUND (contains `FOR UPDATE SKIP LOCKED`)
- `tests/frontier.rs` — FOUND (7 tests pass, 4 ignored)
- Commit `fae3444` — FOUND
- Commit `884c09a` — FOUND
- `SQLX_OFFLINE=true cargo build --all-targets` — exit 0
- `SQLX_OFFLINE=true cargo test --test frontier` — 7 passed, 0 failed, 4 ignored

---
*Phase: 03-graph-writer-bfs-frontier*
*Completed: 2026-06-13*
