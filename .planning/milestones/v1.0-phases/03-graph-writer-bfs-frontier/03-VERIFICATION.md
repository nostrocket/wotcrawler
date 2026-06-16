---
phase: 03-graph-writer-bfs-frontier
verified: 2026-06-13T23:30:00Z
status: passed
score: 8/8 must-haves verified
overrides_applied: 0
---

# Phase 3: Graph Writer & BFS Frontier Verification Report

**Phase Goal:** Accepted follow lists become durable graph state via transactional edge diffs, and a DB-resident reachability-gated BFS frontier drives discovery and survives crashes without redoing completed work.
**Verified:** 2026-06-13T23:30:00Z
**Status:** passed
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Applying a replacing kind-3 inserts only added edges and deletes only removed edges in one transaction, and re-applying the same event id touches zero edge rows. | VERIFIED | `apply_diff_adds_and_removes` and `same_event_zero_touch` tests pass in `tests/graph_writer.rs`. `apply_validated` calls the unmodified `apply_follow_list` which provides the transactional diff. Re-apply returns `Ok(false)` with zero edge touches confirmed. |
| 2 | A crawl starting from a configurable anchor pubkey discovers reachable pubkeys via BFS, enqueuing only pubkeys followed by someone already in the graph (spam islands stay unexplored). | VERIFIED | `bfs_reaches_full_component` confirms all 5 reachable seeds end `fetched` and spam island seed 99 is never inserted. `spam_island_never_fetched_endtoend` proves a pre-seeded island with no follow list resolves `not_found` with no synthetic edges. Structural reachability: `upsert_pubkey`-on-followee is the ONLY non-anchor insertion path in `apply_validated`. |
| 3 | Killing the crawler mid-crawl and restarting it resumes from the DB-resident frontier without refetching already-completed pubkeys. | VERIFIED | `crash_resume_no_redo` test passes: 2 orphaned `in_progress` rows reclaimed and completed; pre-`fetched` row's `fetch_count` unchanged (5 before and after restart). `reclaim_stale_on_startup` SQL resets `status='in_progress'` rows to `discovered`. |
| 4 | In-flight fetch concurrency is bounded end-to-end, so the frontier and queues do not grow without limit under load. | VERIFIED | `bounded_concurrency` test confirms peak in-flight batches is >= 2 and <= K=3 with `AtomicUsize` instrumentation. `run_crawl` uses `tokio::sync::Semaphore` with `acquire_owned()` before `tokio::spawn` — blocking at the cap. Queue lives in `pubkeys.status` (DB), not in memory. |
| 5 | Every pubkey records when its follow-list knowledge was last acquired or confirmed. | VERIFIED | `last_fetched_at_stamped_on_terminal` confirms all three terminal states (`fetched`, `not_found`, `failed`) produce non-NULL `last_fetched_at`. `requeue_or_fail` stamps on terminal `failed`; `set_fetch_status` stamps on `fetched`/`not_found`; writer stamps on `fetched`. |
| 6 | Migration 0002 widens the status domain to include `in_progress`, adds `claimed_at` and `fetch_attempts` columns, and hides `in_progress` from the contract view. | VERIFIED | `migrations/0002_frontier.sql` exists with DROP/ADD CONSTRAINT, ADD COLUMN IF NOT EXISTS, and CREATE OR REPLACE VIEW with CASE collapse. `migration_0002_widens_status_and_hides_in_progress` test passes: in_progress accepted, viewed as discovered, internal columns absent from view. |
| 7 | Two concurrent claim_batch calls never return the same id (FOR UPDATE SKIP LOCKED). | VERIFIED | `skip_locked_no_double_claim` passes: two separate pools claim M=40 rows, zero overlap in id sets, union covers all 40 rows exactly once, neither call blocks. |
| 8 | A spam-island pubkey nobody reachable follows is never claimed. | VERIFIED | `spam_island_never_crawled` (claim-level: insertion is the gate, not the claim query — no reachability predicate); `bfs_reaches_full_component` (end-to-end: seed 99 never inserted). |

**Score:** 8/8 truths verified

### Deferred Items

None — all phase 3 success criteria are met within this phase.

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `migrations/0002_frontier.sql` | Additive frontier migration with status widen, new columns, view collapse | VERIFIED | File exists, 59 lines, contains DROP/ADD CONSTRAINT, ADD COLUMN IF NOT EXISTS, CREATE OR REPLACE VIEW with CASE, COMMENT ON. |
| `src/crawl/frontier.rs` | seed_anchor, claim_batch (FOR UPDATE SKIP LOCKED), reclaim_stale_on_startup, requeue_or_fail | VERIFIED | File exists, 165 lines, all four functions implemented, ClaimedAuthor struct. SKIP LOCKED confirmed in claim CTE at line 67. |
| `src/crawl/mod.rs` | crawl module entry, pub mod frontier + apply, run_crawl (Semaphore-bounded), DEFAULT constants | VERIFIED | File exists, 241 lines, pub mod apply + frontier, DEFAULT_BATCH_SIZE=64, DEFAULT_CONCURRENCY=8, DEFAULT_MAX_ATTEMPTS=3, Semaphore at line 142, run_crawl at line 117. |
| `src/crawl/apply.rs` | apply_validated seam, process_batch (fan-out/union/single-ingest, per-author resolution) | VERIFIED | File exists, 200 lines, apply_validated calls apply_follow_list, process_batch calls acquire_validated_lists ONCE, ingest_events ONCE per batch. |
| `src/lib.rs` | pub mod crawl registration | VERIFIED | `pub mod crawl;` at line 9. |
| `tests/graph_writer.rs` | 3 GRAPH-02 tests through wired seam | VERIFIED | 3 tests, no #[ignore], all pass: apply_diff_adds_and_removes, same_event_zero_touch, newest_wins_under_concurrent_apply. |
| `tests/frontier.rs` | 12 frontier tests (7 module + 5 end-to-end) | VERIFIED | 12 tests, no #[ignore], all pass across two runs (1 transient testcontainers flake re-ran clean, documented flake in env note). |
| `.sqlx/*.json` | Offline metadata for frontier queries (claim, reclaim, requeue) | VERIFIED | 3 frontier query metadata files confirmed: query-d3d0ca5 (claim CTE), query-aea53f9 (reclaim), query-e248607 (requeue). |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `src/crawl/frontier.rs claim_batch` | `pubkeys WHERE status='discovered'` | `FOR UPDATE SKIP LOCKED` CTE in short transaction | WIRED | SQL at lines 62-76, pool.begin()/commit() wrapping, WHERE status='discovered' confirmed at line 64. |
| `src/crawl/frontier.rs requeue_or_fail` | `last_fetched_at` stamp on terminal | CASE on bumped fetch_attempts, NULL for non-terminal | WIRED | Lines 143-162: `last_fetched_at = CASE WHEN fetch_attempts + 1 >= $2::int2 THEN $3 ELSE last_fetched_at END`. |
| `src/lib.rs` | `src/crawl/mod.rs` | `pub mod crawl` | WIRED | Line 9 of lib.rs confirmed. |
| `src/crawl/apply.rs apply_validated` | `store::follows::apply_follow_list` | upsert follower + each followee, then apply diff | WIRED | Lines 59-78: upsert_pubkey for follower and every followee, then apply_follow_list called with resolved ids. |
| `src/crawl/apply.rs process_batch` | `relay::acquire_validated_lists` | single pass over cross-relay raw union | WIRED | Lines 136-144: acquire_validated_lists called ONCE (D-08), not per relay. |
| `src/crawl/mod.rs run_crawl` | `frontier::claim_batch` | claim -> acquire_owned (blocks at cap) -> spawn | WIRED | Lines 146-193: claim_batch call, Semaphore::acquire_owned before spawn, process_batch in spawned task. |

### Data-Flow Trace (Level 4)

| Artifact | Data Variable | Source | Produces Real Data | Status |
|----------|--------------|--------|-------------------|--------|
| `src/crawl/apply.rs apply_validated` | `follower_id`, `followee_ids`, `vfl` | `upsert_pubkey` (DB INSERT/ON CONFLICT), `ValidatedFollowList` from ingest gate | Yes — DB upserts return real surrogate ids; edge diff writes real rows | FLOWING |
| `src/crawl/frontier.rs claim_batch` | `rows` (Vec<ClaimedAuthor>) | `sqlx::query!` CTE returning `p.id, p.pubkey` | Yes — CTE UPDATE RETURNING from live pubkeys table | FLOWING |
| `src/crawl/mod.rs run_crawl` | `stats` (CrawlStats) | claim_batch → process_batch → DB writes | Yes — driven end-to-end by DB state; tests confirm real state transitions | FLOWING |

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| GRAPH-02 edge diff correctness | `SQLX_OFFLINE=true cargo test --test graph_writer -- --test-threads=2` | 3 passed, 0 failed | PASS |
| CRAWL-01/02/03/04 + FRESH-01 end-to-end | `SQLX_OFFLINE=true cargo test --test frontier -- --test-threads=2` (re-ran flaky test once) | 12 passed, 0 failed | PASS |
| Migration idempotency + view collapse | `SQLX_OFFLINE=true cargo test --test migrations -- --test-threads=2` | 3 passed, 0 failed | PASS |
| Offline build | `SQLX_OFFLINE=true cargo build --all-targets` | exit 0 | PASS |
| Anti-pattern guard: no RECURSIVE/reachable in src/crawl/ | `grep -rn "RECURSIVE\|reachable" src/crawl/` | no output | PASS |
| No #[ignore] stubs remaining | `grep -n "#\[ignore\]" tests/frontier.rs tests/graph_writer.rs` | no output | PASS |
| No debt markers (TBD/FIXME/XXX) | Grep across all phase files | no output | PASS |

### Probe Execution

No probes declared. Phase 3 has no `scripts/tests/probe-*.sh` files. All verification is via cargo test.

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|------------|-------------|--------|----------|
| GRAPH-02 | 03-01, 03-03 | Replacing kind-3 applied as transactional edge diff; unchanged event id touches zero edge rows | SATISFIED | `apply_diff_adds_and_removes`, `same_event_zero_touch`, `newest_wins_under_concurrent_apply` all pass. `apply_validated` -> `apply_follow_list` (unmodified). |
| CRAWL-01 | 03-02, 03-03 | Crawl starts from single configurable anchor pubkey, discovers via BFS | SATISFIED | `seed_anchor_lands_discovered`, `bfs_reaches_full_component` pass. `run_crawl` seeds anchor and runs two-phase termination BFS loop. |
| CRAWL-02 | 03-02, 03-03 | Only pubkeys followed by someone already in graph are enqueued | SATISFIED | `spam_island_never_crawled`, `spam_island_never_fetched_endtoend`, `bfs_reaches_full_component` pass. Structural: `upsert_pubkey`-on-followee is ONLY non-anchor insertion path. No reachability predicate or recursive CTE. |
| CRAWL-03 | 03-01, 03-02, 03-03 | Frontier is DB-resident; crash/restart resumes without refetching completed work | SATISFIED | `startup_reclaims_in_progress`, `claim_never_returns_fetched`, `crash_resume_no_redo` pass. claim selects only `discovered`; reclaim resets `in_progress`. |
| CRAWL-04 | 03-03 | In-flight fetch concurrency bounded end-to-end | SATISFIED | `bounded_concurrency` passes. Semaphore permit acquired before spawn; queue is pubkeys.status (DB), not in-memory. `skip_locked_no_double_claim` confirms two workers never claim same row. |
| FRESH-01 | 03-01, 03-02, 03-03 | Every pubkey records when follow-list knowledge was last acquired or confirmed | SATISFIED | `requeue_at_cap_sets_failed_and_stamps`, `last_fetched_at_stamped_on_terminal` pass. All three terminal states stamp `last_fetched_at`: writer stamps `fetched`, `set_fetch_status` stamps `not_found`, `requeue_or_fail` stamps `failed`. |

All 6 phase requirements are SATISFIED. No orphaned requirements (all 6 mapped in REQUIREMENTS.md traceability table under Phase 3 with status Complete).

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| No debt markers (TBD/FIXME/XXX) in any phase-modified file | — | — | — | — |
| No #[ignore] stubs remaining in test files | — | — | — | — |
| No RECURSIVE or reachable in src/crawl/ | — | — | — | — |

**Code Review findings (from 03-REVIEW.md — all resolved before verification):**

The code review identified 1 critical (CR-01: worker errors silently swallowed by `workers.retain`) and 2 warnings (WR-01: stale `claimed_at` on terminal `failed` rows; WR-02: `reclaim_stale_on_startup` not resetting `fetch_attempts`). The review status is `resolved` with all CR/WR items fixed. Evidence in the current codebase confirms the fixes:

- CR-01 fixed: `src/crawl/mod.rs` lines 205-217 use a draining collect that joins finished handles via `join_worker`, not silent `retain`. Worker errors surface immediately.
- WR-01 fixed: `src/crawl/frontier.rs` line 155 has `claimed_at = NULL` (unconditionally) in `requeue_or_fail`, not the stale `THEN claimed_at` branch.
- WR-02 fixed: `src/crawl/frontier.rs` line 106 has `fetch_attempts = 0` in `reclaim_stale_on_startup`, not just `claimed_at = NULL`.

### Human Verification Required

None. All Phase 3 behaviors are verifiable programmatically:
- Transactional edge-diff correctness verified by integration tests with a real Postgres (testcontainers).
- BFS frontier, crash-resume, bounded concurrency, and terminal stamping verified against a deterministic offline scripted relay graph.
- No live relay connections in Phase 3 (production live-relay wiring deferred to Phase 4).

### Gaps Summary

No gaps. All 5 ROADMAP success criteria are proven by passing integration tests. All 6 requirement IDs from the PLAN frontmatter are satisfied and cross-reference against REQUIREMENTS.md. All code review findings (CR-01, WR-01, WR-02) were fixed before verification. The offline build is clean, `.sqlx/` metadata covers all new queries, and no debt markers exist in any phase-modified file.

---

_Verified: 2026-06-13T23:30:00Z_
_Verifier: Claude (gsd-verifier)_
