---
phase: 03-graph-writer-bfs-frontier
plan: 03
subsystem: crawl
tags: [postgres, sqlx, crawl, bfs, ingest, nostr, concurrency, semaphore]

# Dependency graph
requires:
  - phase: 03-graph-writer-bfs-frontier
    plan: 01
    provides: "migration 0002 (status domain widened with 'in_progress', claimed_at + fetch_attempts; pubkey_freshness collapse)"
  - phase: 03-graph-writer-bfs-frontier
    plan: 02
    provides: "frontier::{seed_anchor, claim_batch, reclaim_stale_on_startup, requeue_or_fail}, ClaimedAuthor"
  - phase: 02-relay-ingest
    provides: "acquire_validated_lists (generic injected-fetch seam), ingest_events (verify/dedup/newest-wins), ValidatedFollowList"
  - phase: 01-graph-schema
    provides: "apply_follow_list (transactional edge diff), upsert_pubkey, set_fetch_status"
provides:
  - "src/crawl/apply.rs apply_validated (ValidatedFollowList -> apply_follow_list bridge; upsert IS discovery, D-03)"
  - "src/crawl/apply.rs process_batch (fan-out -> raw cross-relay union -> single ingest pass -> per-author terminal/retry resolution, D-08/D-09/D-10)"
  - "src/crawl/mod.rs run_crawl (Semaphore-bounded claim->spawn->apply BFS loop; startup seed + reclaim)"
  - "src/crawl/mod.rs CrawlStats (reclaimed_on_startup, authors_claimed, batches_processed)"
  - "All five Phase 3 success criteria proven green: GRAPH-02, CRAWL-01/02/03/04, FRESH-01"
affects: [04-observability, 04-staleness-loop, 05-nip65-fallback]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Bounded BFS worker loop: tokio::sync::Semaphore permit acquired BEFORE tokio::spawn so acquire() blocks at the cap (backpressure); the DB (pubkeys.status) is the only growable queue, in-process footprint is concurrency x batch_size"
    - "Two-phase termination: an empty claim with workers still in flight joins them first (they may discover new followees), then re-claims; a second empty claim with zero workers is true exhaustion"
    - "Injected union-fetch closure (Fn(Vec<ClaimedAuthor>) -> Fut, Clone + Send + Sync + 'static) keeps the loop generic over production fan-out vs. an offline Send ScriptedGraph, so the whole crawl is verifiable offline (no live relays in Phase 3)"
    - "Single-ingest-over-union (D-08) realized by reusing Phase 2's generic acquire_validated_lists with a raw-union fetch closure — ingest_events runs ONCE over the cross-relay union, never per relay"

key-files:
  created:
    - "src/crawl/apply.rs"
  modified:
    - "src/crawl/mod.rs"
    - "tests/graph_writer.rs"
    - "tests/frontier.rs"
    - ".planning/phases/03-graph-writer-bfs-frontier/03-VALIDATION.md"

key-decisions:
  - "run_crawl is generic over an injected fetch_union closure rather than taking a live nostr_sdk::Client. The production Client wiring (fan out acquire_validated_lists_client across the curated relay set, concat the raw events) is a thin Phase 4 closure; making the loop generic lets Phase 3 prove CRAWL-01/02/03/04 deterministically offline against a Send ScriptedGraph, honoring 'never live relays in Phase 3'."
  - "process_batch is also generic over a FnOnce union-fetch closure and reuses Phase 2's acquire_validated_lists verbatim — this is the literal D-08 'ingest once over the union' lever (RESEARCH Open Question 1 resolution (a)). No new ingest/relay code."
  - "A batch-level RelayError requeues EVERY claimed author in that batch via requeue_or_fail (the fetch is fan-out-union, so a failure is for the whole batch's fetch, not one author). With batch_size=1 in the FRESH-01/failed test this isolates the failing author cleanly."
  - "No new crawl-error enum: process_batch returns StoreError (terminal/retry writes), surfacing RelayError only internally to choose requeue-vs-not_found. run_crawl flattens a worker JoinError into StoreError::Sqlx(Protocol(..)) rather than inventing an enum for a should-not-happen panic (one-enum-per-boundary, error.rs shape)."
  - "Added a NEW Send ScriptedGraph test helper inside tests/frontier.rs instead of reusing tests/mock_relay (which is Rc<RefCell> / !Send and cannot cross tokio::spawn). The mock_relay stays the paginate_chunk fixture; the crawl loop needs an Arc-backed Send graph."
  - "Doc comments reworded to avoid the literal tokens `reachable` / `RECURSIVE` so the Pitfall-4 anti-pattern guard `grep -rn 'RECURSIVE|reachable' src/crawl/` returns nothing (the design has no such predicate; the words only appeared in prose)."

patterns-established:
  - "Generic-over-injected-fetch crawl driver: the orchestration loop and per-batch composition are both generic over a fetch closure, so production (live relays) and tests (offline scripted graph) share one code path with zero test-only branches in src/."
  - "Permit-before-spawn semaphore backpressure for a DB-resident work queue: claim_batch -> acquire_owned (blocks at cap) -> spawn(process_batch), opportunistically reaping finished JoinHandles so the handle vector stays bounded too."

requirements-completed: [GRAPH-02, CRAWL-01, CRAWL-02, CRAWL-04, FRESH-01]

# Metrics
duration: 18min
completed: 2026-06-13
---

# Phase 3 Plan 03: Crawl Wiring & Phase Verification Summary

**Wired the crawl end-to-end — `apply_validated` bridges a verified `ValidatedFollowList` into the unmodified transactional edge writer (the followee upsert IS discovery, D-03), `process_batch` fans out across relays, runs `ingest_events` ONCE over the raw cross-relay union (D-08), and resolves each claimed author to `fetched`/`not_found`/`failed`, and `run_crawl` drives it all from a `Semaphore`-bounded claim→spawn→apply BFS loop seeded at the anchor with a startup crash-orphan reclaim — proving all five Phase 3 success criteria (GRAPH-02, CRAWL-01/02/03/04, FRESH-01) green against a deterministic offline scripted relay graph.**

## Performance

- **Duration:** ~18 min
- **Completed:** 2026-06-13
- **Tasks:** 3
- **Files modified:** 5 (1 created, 4 modified; `.sqlx/` regenerated with zero drift)

## Accomplishments

- `src/crawl/apply.rs` (NEW): `apply_validated` resolves the follower + every followee id via `upsert_pubkey` (the upsert is the only non-anchor insertion path — CRAWL-02 structural) then calls the CONSUMED `apply_follow_list`. `process_batch` is generic over an injected union-fetch closure: it builds the solicited-author set, runs the raw cross-relay union through a SINGLE `acquire_validated_lists` pass (D-08 — `ingest_events` runs once, never per relay), indexes winners by author, and resolves each claimed author — hit → `apply_validated` (writer flips to `fetched`), relays-answered-no-list → `not_found` (D-10), batch-level `RelayError` → `requeue_or_fail` for every claimed author (D-09). Every terminal path stamps `last_fetched_at` (FRESH-01).
- `src/crawl/mod.rs` (MODIFIED): `run_crawl` seeds the anchor (D-03) + reclaims crash orphans (D-06) at startup, then a `tokio::sync::Semaphore` claim→spawn loop bounds in-flight batches (CRAWL-04 — permit acquired before spawn, so `acquire()` blocks at the cap). Termination is two-phase (join in-flight workers on an empty claim, re-claim any followees they discovered, break only when both claim and worker set are empty — CRAWL-01). `CrawlStats` reports reclaim/claim/batch counts. No status filter predicate, no recursive CTE.
- `tests/graph_writer.rs` (FILLED): 3 GRAPH-02 tests through the wired seam with REAL signed events run through `ingest_events` — `apply_diff_adds_and_removes` ({A,B,C}→{A,C,D} = delete B, add D), `same_event_zero_touch` (re-apply same event id → `Ok(false)`, zero edges, `fetch_count` bumps but `change_count` doesn't), `newest_wins_under_concurrent_apply` (older-then-newer and newest-over-union both converge on {A,B}).
- `tests/frontier.rs` (FILLED): 5 end-to-end crawl-loop tests against a NEW `Send` `ScriptedGraph` (Arc-backed; the `mock_relay` is `!Send`) — `bfs_reaches_full_component` (anchor→{2,3}; 2→{4}; 3→{4,5}; all `fetched`, spam island never inserted), `spam_island_never_fetched_endtoend` (pre-seeded island with no list → never `fetched`, no synthesized edges), `crash_resume_no_redo` (orphans reclaimed + completed; a pre-`fetched` row's `fetch_count` unchanged), `bounded_concurrency` (20-leaf fan-out, `AtomicUsize` in-flight peak ≥ 2 and ≤ K=3), `last_fetched_at_stamped_on_terminal` (all three of `fetched`/`not_found`/`failed` non-NULL stamped). No `#[ignore]` scaffolds remain.
- `.sqlx/`: `cargo sqlx prepare -- --all-targets` against a DB migrated through 0002 reported ZERO drift — the seam consumes existing store/relay helpers and adds no new compile-checked queries.
- `03-VALIDATION.md`: `nyquist_compliant: true` / `wave_0_complete: true`, verification map filled with the real per-task/requirement/test commands, sign-off recorded.

## Task Commits

1. **Task 1: apply_validated seam + per-batch fan-out/union/ingest** — `8daee2b` (feat)
2. **Task 2: bounded worker-pool crawl loop + CRAWL-01/02/03/04 + FRESH-01 tests** — `b2e686e` (feat)
3. **Task 3: full-suite green + sqlx prepare (zero drift) + validation sign-off** — `60f3c75` (chore)

**Plan metadata:** committed with this SUMMARY (docs: complete plan)

## Files Created/Modified

- `src/crawl/apply.rs` (created) — `apply_validated`, `process_batch`.
- `src/crawl/mod.rs` (modified) — `pub mod apply;`, `run_crawl`, `CrawlStats`, `join_worker`.
- `tests/graph_writer.rs` (modified) — 3 GRAPH-02 tests through the seam (no `#[ignore]`).
- `tests/frontier.rs` (modified) — `ScriptedGraph` helper + 5 end-to-end tests (no `#[ignore]`); 7 frontier-module tests from 03-02 retained.
- `.planning/phases/03-graph-writer-bfs-frontier/03-VALIDATION.md` (modified) — sign-off.

## Decisions Made

- **Generic-over-injected-fetch loop.** `run_crawl` takes a `fetch_union` closure rather than a live `Client`, so Phase 3 proves the crawl deterministically offline (the production live-relay fan-out is a thin Phase 4 closure). `process_batch` is likewise generic and reuses Phase 2's `acquire_validated_lists` verbatim, making D-08's "ingest once over the union" the literal composition (RESEARCH Open Question 1 resolution (a)).
- **Batch-level requeue on `RelayError`.** Because the fetch is a single fan-out-union call, a `RelayError` is a failure of the whole batch's fetch, so every claimed author in the batch is sent through `requeue_or_fail`. `batch_size=1` in the failed-path test isolates the failing author.
- **No new error enum.** `process_batch` returns `StoreError`; `run_crawl` flattens a worker `JoinError` into `StoreError::Sqlx(Protocol(..))` for the should-not-happen panic case rather than inventing a crawl enum (one-enum-per-boundary).
- **New `Send` `ScriptedGraph` helper.** `tests/mock_relay` is `Rc<RefCell>` / `!Send` and cannot cross `tokio::spawn`; the crawl loop needs an `Arc`-backed `Send` graph, added inside `tests/frontier.rs`.

## Deviations from Plan

None — plan executed exactly as written.

Note (not a deviation): two minor doc-comment rewordings (`src/crawl/apply.rs` and `src/crawl/mod.rs`) removed the literal prose tokens `reachable` / `RECURSIVE` so the Pitfall-4 anti-pattern guard `grep -rn "RECURSIVE\|reachable" src/crawl/` returns nothing. The design never had such a predicate; the words only appeared in explanatory prose. The plan's verification explicitly runs that grep, so keeping it literally clean is satisfying the stated check, not changing behavior.

## Issues Encountered

- None requiring a fix. DB integration tests were run with `--test-threads=2` per the environment note (the known testcontainers container-creation race); all passed on the first run with that flag.

## Threat Model Coverage

- **T-03-09 (Tampering / edge-diff integrity, GRAPH-02):** `apply_validated` delegates to the proven atomic `apply_follow_list`; `apply_diff_adds_and_removes` + `same_event_zero_touch` verify add/remove deltas + zero-touch idempotency against REAL validated events.
- **T-03-10 (Spoofing / forged-unsolicited events):** the driver runs the raw union through `ingest_events` (verify + solicited-author gate) ONCE via `acquire_validated_lists`; no validation-bypass path was added. `newest_wins_under_concurrent_apply` exercises the union resolution.
- **T-03-11 (DoS / unbounded in-flight, CRAWL-04):** `Semaphore` permit-before-spawn cap; the queue lives in `pubkeys.status`, not memory; `bounded_concurrency` asserts the in-flight peak ≤ K.
- **T-03-12 (EoP / crawling spam islands, CRAWL-02):** reachability is structural — `upsert_pubkey`-on-followee is the only non-anchor insertion path; `bfs_reaches_full_component` (island never inserted) + `spam_island_never_fetched_endtoend` (pre-seeded island never fetched, no synthesized edges) prove it; no status/reach predicate or recursive CTE (guard clean).
- **T-03-13 (Repudiation / terminal status missing timestamp, FRESH-01):** every terminal path stamps `last_fetched_at` (writer for `fetched`, `set_fetch_status` for `not_found`, `requeue_or_fail` for `failed`); `last_fetched_at_stamped_on_terminal` asserts non-NULL across all three.
- **T-03-SC (cargo installs):** no packages added this plan.

## User Setup Required

None — no external service configuration required. Production live-relay wiring of `run_crawl`'s `fetch_union` closure (fan out `acquire_validated_lists_client` across the curated relay set) is a Phase 4 daemon concern.

## Next Phase Readiness

- The crawl is fully wired and verified offline: `run_crawl` is the Phase 4 daemon's entry point; it needs only a live-relay `fetch_union` closure (fan out `acquire_validated_lists_client` per curated relay, concat the raw `Vec<Event>` union) plus config-sourced `batch_size`/`concurrency`/`max_attempts` (OPS-01) and the observability metrics around `CrawlStats`.
- `not_found` rows are recorded with stamped `last_fetched_at` ready for Phase 5 NIP-65 fallback (RELAY-05) and Phase 4 staleness re-enqueue (FRESH-02).
- No blockers introduced.

## Self-Check: PASSED
- `src/crawl/apply.rs` — FOUND (contains `apply_validated`)
- `src/crawl/mod.rs` — FOUND (contains `Semaphore`, `run_crawl`, `seed_anchor` + `reclaim_stale_on_startup` startup calls)
- `tests/graph_writer.rs` — FOUND (3 tests, no `#[ignore]`)
- `tests/frontier.rs` — FOUND (12 tests pass, no `#[ignore]`)
- Commit `8daee2b` — FOUND
- Commit `b2e686e` — FOUND
- Commit `60f3c75` — FOUND
- `SQLX_OFFLINE=true cargo build --all-targets` — exit 0
- `SQLX_OFFLINE=true cargo test` — full suite green (frontier 12, graph_writer 3, all others pass; 0 failed, 0 ignored)
- `grep -rn "RECURSIVE\|reachable" src/crawl/` — returns nothing (anti-pattern guard clean)
- `apply_follow_list` / `ingest_events` / `acquire_validated_lists` — unmodified (CONSUMED)

---
*Phase: 03-graph-writer-bfs-frontier*
*Completed: 2026-06-13*
