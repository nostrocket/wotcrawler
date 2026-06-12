---
gsd_state_version: 1.0
milestone: v1.0
milestone_name: milestone
status: executing
stopped_at: Completed 02-01-PLAN.md
last_updated: "2026-06-12T13:02:37.378Z"
last_activity: 2026-06-12 -- Completed 02-01 (relay/ingest foundation + spikes)
progress:
  total_phases: 5
  completed_phases: 1
  total_plans: 8
  completed_plans: 4
  percent: 25
---

# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-06-11)

**Core value:** From one anchor pubkey, maintain a complete and continuously fresh follow graph of everyone reachable through follows — fetched efficiently — so a downstream trust/spam layer can read it from a shared database at any time.
**Current focus:** Phase 02 — relay-acquisition-validation

## Current Position

Phase: 02 (relay-acquisition-validation) — EXECUTING
Plan: 2 of 4
Status: Executing Phase 02 (02-01 complete; Wave 2 — 02-02/02-03 next, parallel)
Last activity: 2026-06-12 -- Completed 02-01 (relay/ingest foundation + spikes)

Progress: [██░░░░░░░░] 25%

## Performance Metrics

**Velocity:**

- Total plans completed: 0
- Average duration: — min
- Total execution time: — hours

**By Phase:**

| Phase | Plans | Total | Avg/Plan |
|-------|-------|-------|----------|
| - | - | - | - |

**Recent Trend:**

- Last 5 plans: —
- Trend: —

*Updated after each plan completion*
| Phase 01 P01 | 120 | 3 tasks | 9 files |
| Phase 01 P02 | 2 | 3 tasks | 3 files |
| Phase 01 P03 | 9 | 3 tasks | 10 files |
| Phase 02 P01 | 18 | 4 tasks | 14 files |

## Accumulated Context

### Decisions

Decisions are logged in PROJECT.md Key Decisions table.
Recent decisions affecting current work:

- [Roadmap]: Schema-first ordering — the PostgreSQL schema is the spam layer's public API and gates all later work; migrations on hundreds of millions of rows are expensive to change later.
- [Roadmap]: RELAY-05 (NIP-65 fallback) and RELAY-06 (relay health) kept in v1 per user override (research suggested deferring); both land in Phase 5, gated behind Phase 4 coverage metrics.
- [Roadmap]: FRESH-04 (adaptive refresh) stays v2 — no public kind-3 churn data exists; needs weeks of FRESH-03 instrumentation first.
- [Phase ?]: [01-01] Pinned Rust toolchain to 1.94.0 (sqlx 0.9 MSRV) per RESEARCH MSRV correction.
- [Phase 1]: [01-02] Phase 1 schema shipped as single idempotent migration 0001_graph_schema.sql; status stored as TEXT+CHECK (not native enum, D-09); contract = 3 views (follow_edges, pubkey_lookup, pubkey_freshness) with PUBLIC CONTRACT COMMENT ON; freshness/churn/applied columns shipped now to avoid later migrations on the big table (D-07/D-10).
- [Phase ?]: [Phase 1]: [01-03] Phase 1 store layer shipped: connect/run_migrations, upsert_pubkey (32-byte-validated get-or-create), set_fetch_status, and the transactional edge-diff writer apply_follow_list (idempotent on unchanged event id, self-follow-dropping, atomic). SCHEMA.md public contract + committed .sqlx offline metadata. GRAPH-03 (MVCC concurrency) and GRAPH-04 (contract doc) proven by green tests.
- [Phase 2]: [02-01] Foundation shipped: nostr-sdk 0.44 / governor 0.10 / metrics 0.24 added (compiles on toolchain 1.94); relay + ingest module trees registered with stubs; RelayError/IngestError enums (count-and-skip vs genuine-error split); ValidatedFollowList output contract; offline nostr event fixtures.
- [Phase 2]: [02-01 SPIKE RELAY-01] nostr-relay-pool 0.44.1 reconnect is LINEAR (1 + diff/2) with ±3s jitter + 60s cap, NOT exponential — plan 02-03 MUST add an app-side capped-exponential-with-jitter backoff for fetch re-arm; SDK socket reconnect kept on. RELAY-01 not satisfied on SDK default alone.
- [Phase 2]: [02-01 SPIKE RELAY-02] No SDK NIP-11 accessor (RelayInformationDocument is parse-only; reqwest dev-dep only) — plan 02-03 MUST add reqwest + GET Accept: application/nostr+json; defaults max_limit=500 / max_subscriptions=20 / max_filters=10 when omitted.

### Pending Todos

None yet.

### Blockers/Concerns

- Curated relay set coverage (% of reachable pubkeys discoverable without NIP-65 fallback) is unknown until Phase 4 observability measures it — directly gates Phase 5 scope.
- Initial full crawl is expected to take multiple days; resource profile at full scale (low millions of pubkeys) is unmeasured. Instrument early.

## Deferred Items

Items acknowledged and carried forward from previous milestone close:

| Category | Item | Status | Deferred At |
|----------|------|--------|-------------|
| *(none)* | | | |

## Session Continuity

Last session: 2026-06-12T13:02:37.378Z
Stopped at: Completed 02-01-PLAN.md
Resume file: .planning/phases/02-relay-acquisition-validation/02-02-PLAN.md (Wave 2; 02-02 and 02-03 run in parallel)
