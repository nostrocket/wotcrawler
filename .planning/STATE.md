---
gsd_state_version: 1.0
milestone: v1.0
milestone_name: milestone
status: executing
stopped_at: Completed 02-05-PLAN.md - fetch BLOCKERs CR-01 to CR-04 closed
last_updated: "2026-06-13T12:47:08.868Z"
last_activity: 2026-06-13
progress:
  total_phases: 5
  completed_phases: 2
  total_plans: 15
  completed_plans: 15
  percent: 40
---

# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-06-11)

**Core value:** From one anchor pubkey, maintain a complete and continuously fresh follow graph of everyone reachable through follows — fetched efficiently — so a downstream trust/spam layer can read it from a shared database at any time.
**Current focus:** Phase 02 — relay-acquisition-validation

## Current Position

Phase: 3
Plan: Not started
Status: Ready to execute
Last activity: 2026-06-13

Progress: [█████░░░░░] 50%

## Performance Metrics

**Velocity:**

- Total plans completed: 12
- Average duration: — min
- Total execution time: — hours

**By Phase:**

| Phase | Plans | Total | Avg/Plan |
|-------|-------|-------|----------|
| 02 | 12 | - | - |

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
| Phase 02 P04 | 4 | 1 task | 2 files |
| Phase 02 P05 | 8 | 2 tasks | 3 files |
| Phase 02 P06 | 1 | 1 tasks | 3 files |
| Phase 02 P07 | 3 | - tasks | - files |
| Phase 02 P08 | 3 | - tasks | - files |
| Phase 02 P08 | 3 | 2 tasks | 2 files |
| Phase 02 P09 | 22 | 2 tasks | 4 files |
| Phase 02 P10 | 5 | 2 tasks | 3 files |
| Phase 02 P11 | 6 | 2 tasks | 4 files |
| Phase 02 P12 | 3m | 2 tasks | 2 files |

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
- [Phase 2]: [02-04] Acquire pipeline shipped — Phase 02 complete. relay::acquire_validated_lists composes the raw paged fetch stream through ingest::ingest_events (composition-only seam: zero validation logic, grep-gated); acquire_validated_lists_client is the production wrapper over fetch_complete + a live Client. E2E test drives a two-window adversarially-polluted mock-relay stream through the wired pipeline and proves exactly the deduped/newest-wins/self-drop-filtered ValidatedFollowList emerges (forged/unsolicited/future-dated excluded; second-window event wins, proving resolution across both paged windows — T-02-14/T-02-15). src/ingest untouched (ingest_events already pub). No deviations, no new deps.
- [Phase ?]: 02-05 Fetch completeness and safety BLOCKERs closed: page_back now INCLUSIVE (CR-03); paginate_chunk dedups cross-window, stops on zero new ids, and enforces MAX_PAGES_PER_CHUNK=10000 budget (CR-04); fetch_window_with_deadline constructs RelayError::FetchTimeout on elapsed greater-or-equal timeout because the SDK returns a partial Ok (CR-02); pre-verify dedup_by_id removed so dedup follows verify::accept to defeat id-squat (CR-01 fetch half).
- [Phase 02-06]: CR-01 closed: ingest_events now verifies before dedup so only verified event ids enter the cross-relay seen-set; a forged id-squat copy (T-02-14) can no longer consume a genuine id and suppress the honest follow list. Genuine dedup preserved.
- [Phase ?]: [02-07] NIP-11 fetch hardened (CR-06/WR-02): shared LazyLock reqwest client with 10s request + 5s connect timeout (T-02-18); MAX_NIP11_BYTES=64KiB stream-and-bail body bound (T-02-19); MAX_ADVERTISED_LIMIT=5000 upper-clamp on advertised max_limit so an absurd value cannot defeat count-vs-cap pagination (T-02-13, Pitfall 1).
- [Phase ?]: [02-08] Rate limiter correctness: shared Arc<DirectLimiter> per relay so concurrent acquire() obeys one GCRA quota (CR-05/T-02-10); backoff saturates at failures >= 64 closing the u128 checked_shl zero-delay window at 119..=127 (WR-01/T-02-20). Production wiring is 02-09.
- [Phase ?]: [Phase 2]: [02-09] WR-03 closed — paginate_chunk_gated gates every fetch_events behind RateLimiterRegistry::acquire (T-02-10); acquire_validated_lists_client sources max_limit from LimitCache (T-02-13); spawn_notice_consumer routes NOTICE/CLOSED into record_notice via the shared registry (T-02-09).
- [Phase ?]: [Phase 2]: [02-10] BLOCKER 1 closed (CR-03 residual / RELAY-03): paginate_chunk tracks prev_until across iterations; a repeated pinned until=T that stays capped with zero new ids returns Err(FetchTimeout) so the caller requeues rather than silently completing a truncated follow list. Genuine exhaustion (short window, or first page-back into the boundary second) still breaks Ok. Reused FetchTimeout (no new variant). Deterministic-relay test (prefix_for_until_fetch_fn) proves it RED then GREEN.
- [Phase ?]: [02-11] WR-03 residual / RELAY-04 closed: per-relay GCRA limiter keyed on the caller's individual relay_url threaded through fetch_complete/fetch_complete_with_timeout; pool_label demoted to diagnostics, never the acquire() key. Two pooled relays now mint two independent limiter keys; GCRA state survives pool churn.

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

Last session: 2026-06-13T11:09:17.375Z
Stopped at: Completed 02-05-PLAN.md - fetch BLOCKERs CR-01 to CR-04 closed
Resume file: None
