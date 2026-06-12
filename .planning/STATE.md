---
gsd_state_version: 1.0
milestone: v1.0
milestone_name: milestone
status: executing
stopped_at: Completed 02-03-PLAN.md
last_updated: "2026-06-12T13:45:00.000Z"
last_activity: 2026-06-12 -- Completed 02-03 (relay acquisition transport — RELAY-01..04)
progress:
  total_phases: 5
  completed_phases: 1
  total_plans: 8
  completed_plans: 6
  percent: 38
---

# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-06-11)

**Core value:** From one anchor pubkey, maintain a complete and continuously fresh follow graph of everyone reachable through follows — fetched efficiently — so a downstream trust/spam layer can read it from a shared database at any time.
**Current focus:** Phase 02 — relay-acquisition-validation

## Current Position

Phase: 02 (relay-acquisition-validation) — EXECUTING
Plan: 4 of 4
Status: Executing Phase 02 (02-01 + 02-02 + 02-03 complete; 02-04 acquire pipeline next — wires fetch→ingest)
Last activity: 2026-06-12 -- Completed 02-03 (relay acquisition transport — RELAY-01..04)

Progress: [████░░░░░░] 38%

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
| Phase 02 P02 | 6 | 3 tasks | 9 files |
| Phase 02 P03 | 20 | 3 tasks | 12 files |

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
- [Phase 2]: [02-02] Ingest validation gate shipped (INGEST-01..05): verify::accept (Event::verify id+sig + kind/author gate), cross-relay HashSet<EventId> dedup orchestrator, kind-agnostic pick_winner (future-clamp + newest-wins + lowest-id tie-break, EventId derives Ord), reject-not-truncate followee extraction. 16 offline tests green. ingest_events gained a requested-author-set parameter the 02-01 stub omitted.
- [Phase 2]: [02-03] Relay acquisition transport shipped (RELAY-01..04): connect_curated (signer-less Client, custom RelayOptions via pool().add_relay), app-side capped-exponential-with-jitter backoff_delay (SDK reconnect is linear, so this satisfies RELAY-01), per-relay governor GCRA gate + rate-limited/blocked notice handling (RELAY-04), reqwest NIP-11 fetch + LimitCache with 500/20/10 defaults clamping non-positive values (RELAY-02), author-chunked until-window pagination where page_back compares count-vs-cap and never trusts EOSE + explicit per-fetch timeout (RELAY-03). 17 offline tests green. Mock relay = injected-fetch-fn (documented alternative to a ws mock). Fixed a u64→u32 truncation bug in the backoff saturation path.

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

Last session: 2026-06-12T13:45:00.000Z
Stopped at: Completed 02-03-PLAN.md
Resume file: .planning/phases/02-relay-acquisition-validation/02-04-PLAN.md (acquire pipeline — wires fetch_complete → ingest_events; uses LimitCache max_limit + RateLimiterRegistry gate)
