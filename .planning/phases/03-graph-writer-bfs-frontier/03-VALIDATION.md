---
phase: 3
slug: graph-writer-bfs-frontier
status: draft
nyquist_compliant: false
wave_0_complete: false
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
| TBD | TBD | TBD | GRAPH-02 / CRAWL-01..04 / FRESH-01 | — / — | N/A | unit/integration | `cargo nextest run` | ❌ W0 | ⬜ pending |

*Planner refines this map from the Validation Architecture section of 03-RESEARCH.md. Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `graph_writer.rs` test stubs — transactional edge-diff correctness (GRAPH-02), newest-wins idempotency under concurrency
- [ ] `frontier.rs` test stubs — claim/lease (CRAWL-01), structural reachability enqueue (CRAWL-02), no re-fetch of `fetched` rows (CRAWL-03), crash-recovery sweep (CRAWL-04), terminal-state `last_fetched_at` stamping (FRESH-01)
- [ ] Test Postgres fixture / migration harness for DB integration tests

*Wave 0 gaps identified in 03-RESEARCH.md Validation Architecture section.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Multi-day full-scale crawl resource profile | — | Requires full-scale run (Phase 4 observability) | Deferred — STATE.md concern, not gated here |

*All in-scope Phase 3 behaviors have automated verification.*

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 60s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
