# Project Retrospective

*A living document updated after each milestone. Lessons feed forward into future planning.*

## Milestone: v1.0 — Crawler & Data Layer

**Shipped:** 2026-06-16
**Phases:** 5 | **Plans:** 27 | **Sessions:** 1 (autonomous run, phases 3–5; phases 1–2 prior)

### What Was Built
- PostgreSQL schema as the public contract (pubkeys/follows + 3 views, surrogate bigint ids), sqlx 0.9 store layer, 4 additive migrations (0001–0004).
- Relay acquisition + adversarial-input validation: curated-set connect with app-side backoff, NIP-11 limits, EOSE-distrusting pagination, per-relay GCRA rate limiting, and a verify→dedup→newest-wins ingest gate for kind:3 and kind:10002.
- Transactional edge-diff graph writer + a crash-safe DB-resident reachability-gated BFS frontier (`FOR UPDATE SKIP LOCKED` lease, startup reclaim, no-redo).
- A single `crawler` daemon: initial crawl → continuous uniform-TTL staleness refresh, graceful SIGTERM/SIGINT drain with zero orphaned leases, TOML+env config with fail-fast validation, Prometheus `/metrics` + axum `/health/*` + tracing + committed Grafana dashboard.
- NIP-65 outbox-routing fallback (recover curated-misses via write relays) + an in-memory EWMA relay-health score driving skip/probe routing and per-relay concurrency.

### What Worked
- **Research → pattern-map → plan → plan-check before every phase** caught real design issues early (e.g. the FRESH-03 churn columns already existing; migration 0003 being index-only) and kept the planner grounded in actual file:line analogs.
- **Adversarial code review after each phase found genuine concurrency bugs** that tests had missed — a swallowed-worker-error class bug (Phase 3 CR-01), a stale-timestamp-defeats-staleness bug (Phase 4 CR-01), and a per-relay-permit livelock + probe race (Phase 5 CR-01/CR-02). Chaining review→fix→re-verify materially improved correctness.
- **Reuse-not-rewrite discipline**: Phase 4's `run_daemon_loop` and Phase 5's fallback both reused the proven Phase 3 primitives via injected closures, keeping prior tests green throughout.

### What Was Inefficient
- **Testcontainers port-exposure flake** under full-suite parallelism forced per-binary `--test-threads` runs and repeated re-runs to distinguish flake from regression — real wall-clock cost across every phase. A shared-fixture or serialized-container harness would help.
- **`daemon_config` test isolation**: an env-var (`WOT__CONCURRENCY`) leaked across parallel tests, requiring `--test-threads=1`. Per-test unique env vars or a mutex would remove the foot-gun.
- **Plan-checker cosmetic blockers**: two phases tripped a blocking gate purely because the research "Open Questions" heading lacked a `(RESOLVED)` suffix though answers were present — a mechanical fix that cost a revision cycle.

### Patterns Established
- Migrations are additive + idempotent (`IF NOT EXISTS`, named CHECK, `COMMENT ON ... INTERNAL`, internal columns hidden from contract views).
- Counter metric names are un-suffixed in code; the Prometheus exporter appends `_total` and Grafana PromQL uses it (a Phase 4 lesson that prevented a `_total_total` bug in Phase 5).
- Deadlock-safe acquisition order for the daemon: global crawl permit → per-relay admission → GCRA token → fetch.
- Fallback/external fetch wired as injected closures so `crawl/apply.rs` stays free of the live Client and remains deterministically testable with ScriptedGraph.

### Key Lessons
1. **Code review earns its keep on concurrency-heavy daemon code** — every phase's review surfaced at least one real bug invisible to the green test suite; budget for review→fix as a standing post-execution step.
2. **Verify factual assumptions against the code during research** — "add columns" decisions were wrong twice (churn columns and the nip65 import path) and cheap to correct pre-plan, expensive to correct post-execution.
3. **Operator/live-relay validation is genuinely deferrable** when the bounded smoke-test covers every automatable criterion — but it must be recorded as tracked UAT debt, not silently dropped.

### Cost Observations
- Model mix: planning/execution/fixes on opus; plan-checking + verification + review on sonnet; exploration scouts on sonnet.
- Sessions: 1 autonomous run completed phases 3, 4, 5 plus the milestone lifecycle.
- Notable: research+pattern-map+plan-check overhead paid for itself by avoiding post-execution rework; the dominant inefficiency was test-infra flake, not planning.

---

## Cross-Milestone Trends

### Process Evolution

| Milestone | Sessions | Phases | Key Change |
|-----------|----------|--------|------------|
| v1.0 | 1 (autonomous) | 5 | Established the research→pattern-map→plan→check→execute→review→fix→verify chain per phase |

### Cumulative Quality

| Milestone | Tests | Coverage | Zero-Dep Additions |
|-----------|-------|----------|-------------------|
| v1.0 | ~26 test binaries (all green per-binary) | all 29 requirements verified | Phases 3 & 5 added zero new deps; Phase 4 added the planned daemon/observability stack |

### Top Lessons (Verified Across Milestones)

1. Adversarial code review catches concurrency bugs unit tests miss — *(v1.0; revisit each milestone)*
2. Verify research assumptions against the live code before planning — *(v1.0)*
