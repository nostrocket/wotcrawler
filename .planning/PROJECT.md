# Nostr Web-of-Trust Crawler & Data Layer

## What This Is

A continuously running crawler and data layer that turns nostr's scattered, adversarial kind-3 follow data into a locally held, always-current picture of the social graph. Starting from a single trusted anchor pubkey, it discovers everyone reachable through follows, fetches each follow list from public relays roughly once, remembers how current that knowledge is, and re-checks relays only when the knowledge ages out. It is the foundation for a separate spam-scoring project that propagates trust over this graph.

## Core Value

From one anchor pubkey, maintain a complete and continuously fresh follow graph of everyone reachable through follows — fetched from the wild efficiently (each list roughly once, refreshed only when stale) — so a downstream trust/spam layer can read it from a shared database at any time.

## Requirements

### Validated

- Reachable-component discovery logic: anchor-seeded, structurally reachability-gated BFS reaches the full follow component and never explores spam islands (proven by integration tests) — *Validated in Phase 3: Graph Writer & BFS Frontier*
- Durable graph state via transactional edge diffs (add/remove deltas, newest-wins idempotency on replaceable kind-3) persisted to the shared PostgreSQL schema — *Validated in Phase 3*
- Crash-safe DB-resident frontier: claim/lease via `FOR UPDATE SKIP LOCKED`, startup reclaim of orphaned work, no re-fetch of completed lists; every terminal fetch state stamps freshness metadata — *Validated in Phase 3*

- Single configurable `crawler` daemon binary: initial crawl → continuous uniform-TTL staleness refresh over the same frontier; graceful SIGTERM/SIGINT drain with zero orphaned leases; TOML config (+ env overrides) with fail-fast validation — *Validated in Phase 4: Daemon, Staleness Loop & Observability* (live-relay run + Grafana render deferred to operator UAT)
- Observability: Prometheus `/metrics` (coverage, staleness distribution, relay health, frontier depth, fetch rate, validation failures), axum `/health/live` + `/health/ready`, `tracing` structured logs, periodic progress summaries, committed Grafana dashboard JSON — *Validated in Phase 4*
- Per-pubkey churn capture (FRESH-03) accumulating to ground a future adaptive refresh policy — *Validated in Phase 4*

- Hybrid relay strategy: NIP-65 (kind:10002) write-relay fallback recovers pubkeys the curated set cannot supply (on-demand resolve + persist + re-validate), with a `nip65_recovered` metric quantifying the coverage gap — *Validated in Phase 5: NIP-65 Outbox Routing & Relay Health*
- Relay-health-driven routing: per-relay EWMA health score (connect failures, timeouts, rate-limit hits, latency) that skips degraded relays (with periodic probe-to-recover) and scales per-relay concurrency so healthy relays get more traffic — *Validated in Phase 5*

> Note: Phase 4's two operator-UAT items (a live-relay crawl + Grafana dashboard render) remain deferred to the operator and now also cover Phase 5's real-relay fallback/health-routing effectiveness — every automatable criterion across both phases passed in-session. The crawler/data-layer milestone (v1.0) is functionally complete.

> All eight original v1.0 Active requirements shipped and are captured under **Validated** above (Phases 1–5). They are retained here as the v1.0 acceptance record:
> - ✓ Crawler discovers all reachable pubkeys from a single configurable anchor — v1.0 (P3)
> - ✓ Hybrid relay strategy: curated set + NIP-65 fallback — v1.0 (P2 acquisition, P5 fallback)
> - ✓ Handles nostr realities: out-of-order / duplicate / replaceable newest-wins — v1.0 (P2)
> - ✓ Freshness metadata + staleness-driven re-query (uniform TTL) — v1.0 (P3 stamping, P4 loop)
> - ✓ Spam islands stay unexplored (structural reachability) — v1.0 (P3)
> - ✓ Follow graph persists in a shared DB the spam layer reads independently — v1.0 (P1)
> - ✓ Long-running daemon: initial crawl → continuous staleness refresh — v1.0 (P4)
> - ✓ Observability: coverage / staleness / relay-health visible for unattended operation — v1.0 (P4)

### Active (next milestone — v2 candidates)

- [ ] FRESH-04: Adaptive per-pubkey refresh intervals derived from observed churn (needs weeks of FRESH-03 data, now being accumulated)
- [ ] RELAY-07: NIP-77 negentropy bulk sync with supporting relays (~16% relay support today)
- [ ] RELAY-08: Streaming live kind-3 subscriptions for near-real-time graph updates
- [ ] Operator validation of the v1.0 daemon against real relays at scale (multi-day resource profile; curated-coverage % from `nip65_recovered`) — deferred operator UAT from v1.0

### Out of Scope

- Trust propagation / spam scoring — separate project; this project only builds and maintains the graph it consumes
- Constant-time pubkey → spam-verdict lookup — product of the spam layer, not the data layer
- Content moderation or event-content analysis — the system works entirely from social structure (kind 3, NIP-65), never note content
- Multi-anchor support — single trusted anchor pubkey is the model; revisit only if the spam layer demands it
- Polished deployment for third parties (Docker images, install docs) — single-operator infrastructure; config file + README is enough

## Context

- **Ecosystem**: nostr — pubkeys identified by secp256k1 keys, events distributed across independent public relays over websockets. Kind 3 (contact list) is a replaceable event: relays keep only the newest per pubkey, but different relays may hold different versions. Kind 10002 (NIP-65) advertises a user's preferred read/write relays.
- **Adversarial setting**: spam farms manufacture fake accounts and follow relationships at scale. The downstream spam layer defeats this via mutual follows from a trusted anchor (the one relationship spammers can't cheaply fake) over a deliberately short walk. The crawler's job is to make that computation possible: complete directed-edge data, honestly aged.
- **Scale**: full reachable set from a well-connected anchor — expect low millions of pubkeys and hundreds of millions of directed follow edges. Storage and access patterns must be designed for this from the start.
- **Consumer contract**: the spam layer is a separate codebase that reads the shared database directly. The database schema is effectively this project's public API.
- **Current state (post-v1.0, 2026-06-16)**: shipped the full crawler + data layer — ~5.2k LOC Rust src, ~6.1k LOC tests/migrations, 4 additive migrations (0001–0004), single `crawler` daemon binary. Stack: Rust 1.94, tokio 1.52, sqlx 0.9 (PostgreSQL), nostr-sdk 0.44, axum 0.8, metrics/Prometheus, tracing. All 29 v1.0 requirements satisfied and verified; cross-phase integration clean. Tests run on testcontainers Postgres (known port-exposure flake under full-suite parallelism — run per-binary). Deferred to operator: a live-relay run at scale + Grafana render.

## Constraints

- **Tech stack**: Rust — performance and memory control for a large graph and thousands of concurrent relay connections
- **Architecture**: Shared database as the project boundary — the spam layer consumes the graph by reading the DB, not via an API or library
- **Scale**: Must handle low millions of pubkeys / hundreds of millions of edges — full reachable component, not hop-limited
- **Efficiency**: Each follow list fetched from relays roughly once; re-fetches only on staleness — relay goodwill and bandwidth are finite
- **Operations**: Single operator, self-hosted — favor simplicity over deployment polish

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| Rust for the crawler | Performance/memory control at millions of nodes, strong async story for relay connections | ✓ Good — 5.2k LOC src, tokio daemon, clean async throughout |
| Shared database as spam-layer boundary | Loose coupling between projects via schema; no API service to maintain | ✓ Good — schema is the public contract (3 views), spam layer reads DB directly |
| Full reachable set (no hop limit) | Crawl scope is structural reachability; the short trust walk lives in the spam layer | ✓ Good — structural reachability via `upsert_pubkey`-on-followee, no recursive CTE |
| Hybrid relay strategy (curated set + NIP-65 fallback) | Curated relays catch most users cheaply; outbox hints recover the rest | ✓ Good — manual fallback at the not_found hook (P5); `nip65_recovered` quantifies the gap |
| Database engine: PostgreSQL | MVCC for concurrent spam-layer reads; COPY/upsert throughput; SQL as cross-project contract | ✓ Good — sqlx 0.9 raw-SQL-as-contract, .sqlx offline metadata, 4 additive migrations |
| Staleness policy: uniform TTL (v1), adaptive later | Uniform TTL ships now; adaptive (FRESH-04) needs real churn data first | ✓ Good — uniform humantime TTL scanner; FRESH-03 churn columns accumulating for v2 |
| v1 includes observability | An unattended daemon you can't inspect can't be trusted; metrics are part of done | ✓ Good — Prometheus /metrics + axum /health + tracing + Grafana JSON (P4) |
| In-memory relay health (EWMA), not persisted | A multi-day daemon re-learns health quickly; a table adds write load for marginal benefit | ✓ Good — `RelayHealthRegistry` EWMA drives routing + per-relay concurrency (P5) |
| Manual NIP-65 fallback, not nostr-sdk gossip(true) | Keeps the controllable/testable manual fetch path (pagination, rate limits, deterministic mocks) | ✓ Good — fallback is an injected closure, fully ScriptedGraph-testable |

## Evolution

This document evolves at phase transitions and milestone boundaries.

**After each phase transition** (via `/gsd-transition`):
1. Requirements invalidated? → Move to Out of Scope with reason
2. Requirements validated? → Move to Validated with phase reference
3. New requirements emerged? → Add to Active
4. Decisions to log? → Add to Key Decisions
5. "What This Is" still accurate? → Update if drifted

**After each milestone** (via `/gsd-complete-milestone`):
1. Full review of all sections
2. Core Value check — still the right priority?
3. Audit Out of Scope — reasons still valid?
4. Update Context with current state

---
*Last updated: 2026-06-16 after v1.0 milestone*
