---
phase: 3
slug: graph-writer-bfs-frontier
status: signed-off
nyquist_compliant: true
wave_0_complete: true
created: 2026-06-13
---

# Phase 3 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | `cargo test` (Rust built-in) + `cargo-nextest` |
| **Config file** | none — workspace `Cargo.toml` |
| **Quick run command** | `cargo nextest run --no-fail-fast` (or `cargo test`) |
| **Full suite command** | `cargo nextest run --workspace` |
| **Estimated runtime** | ~tens of seconds (DB integration tests gated behind a test Postgres) |

---

## Sampling Rate

- **After every task commit:** Run quick test command for the touched module
- **After every plan wave:** Run full suite command
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** ~60 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| T1 | 03-03 | 3 | GRAPH-02 | T-03-09 | edge-diff add/remove correctness through the wired seam | integration | `SQLX_OFFLINE=true cargo test --test graph_writer apply_diff_adds_and_removes` | ✅ | ✅ green |
| T1 | 03-03 | 3 | GRAPH-02 | T-03-09 | zero-touch idempotency on an unchanged event id | integration | `SQLX_OFFLINE=true cargo test --test graph_writer same_event_zero_touch` | ✅ | ✅ green |
| T1 | 03-03 | 3 | GRAPH-02 | T-03-10 | newest-wins over the cross-relay union (single ingest pass) | integration | `SQLX_OFFLINE=true cargo test --test graph_writer newest_wins_under_concurrent_apply` | ✅ | ✅ green |
| T2 | 03-03 | 3 | CRAWL-01 | — | anchor-seeded BFS reaches the full reachable component | integration | `SQLX_OFFLINE=true cargo test --test frontier bfs_reaches_full_component` | ✅ | ✅ green |
| T2 | 03-03 | 3 | CRAWL-02 | T-03-12 | structural spam-island exclusion (never inserted / never fetched) | integration | `SQLX_OFFLINE=true cargo test --test frontier spam_island_never_fetched_endtoend` | ✅ | ✅ green |
| T2 | 03-03 | 3 | CRAWL-03 | — | crash-resume reclaims orphans, never re-fetches a `fetched` row | integration | `SQLX_OFFLINE=true cargo test --test frontier crash_resume_no_redo` | ✅ | ✅ green |
| T2 | 03-03 | 3 | CRAWL-04 | T-03-11 | in-flight batch concurrency never exceeds the configured cap | integration | `SQLX_OFFLINE=true cargo test --test frontier bounded_concurrency` | ✅ | ✅ green |
| T2 | 03-03 | 3 | FRESH-01 | T-03-13 | every terminal (`fetched`/`not_found`/`failed`) row stamps `last_fetched_at` | integration | `SQLX_OFFLINE=true cargo test --test frontier last_fetched_at_stamped_on_terminal` | ✅ | ✅ green |
| T3 | 03-03 | 3 | (phase gate) | — | full suite green + offline build green + clean `.sqlx` | integration | `SQLX_OFFLINE=true cargo test && SQLX_OFFLINE=true cargo build --all-targets` | ✅ | ✅ green |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky. Run DB integration tests with `--test-threads=2` (known testcontainers container-creation race in this environment).*

---

## Wave 0 Requirements

- [x] `graph_writer.rs` test stubs — transactional edge-diff correctness (GRAPH-02), newest-wins idempotency under concurrency
- [x] `frontier.rs` test stubs — claim/lease (CRAWL-01), structural reachability enqueue (CRAWL-02), no re-fetch of `fetched` rows (CRAWL-03), crash-recovery sweep (CRAWL-04), terminal-state `last_fetched_at` stamping (FRESH-01)
- [x] Test Postgres fixture / migration harness for DB integration tests

*Wave 0 gaps identified in 03-RESEARCH.md Validation Architecture section.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Multi-day full-scale crawl resource profile | — | Requires full-scale run (Phase 4 observability) | Deferred — STATE.md concern, not gated here |

*All in-scope Phase 3 behaviors have automated verification.*

---

## Validation Sign-Off

- [x] All tasks have `<automated>` verify or Wave 0 dependencies
- [x] Sampling continuity: no 3 consecutive tasks without automated verify
- [x] Wave 0 covers all MISSING references
- [x] No watch-mode flags
- [x] Feedback latency < 60s
- [x] `nyquist_compliant: true` set in frontmatter

**Approval:** signed off 2026-06-13 — Wave 0 satisfied, all five Phase 3 success criteria proven green (GRAPH-02, CRAWL-01/02/03/04, FRESH-01), offline build + full suite green.
