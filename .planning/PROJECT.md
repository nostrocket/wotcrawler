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

> Note: Phase 4's two operator-UAT items (a live-relay crawl + Grafana dashboard render) are deferred to the operator — every automatable criterion passed in-session. Remaining Active work: NIP-65 outbox routing & relay-health-driven routing (Phase 5).

### Active

- [ ] Crawler discovers all pubkeys reachable through follows starting from a single configurable anchor pubkey
- [ ] Crawler fetches kind-3 follow lists from public relays using a hybrid strategy: curated relay set as the workhorse, NIP-65 relay hints as fallback when a pubkey isn't found
- [ ] Crawler handles nostr realities: events arrive out of order, in duplicate, with no canonical copy; kind 3 is replaceable — only the newest event per pubkey counts
- [ ] Every pubkey's follow-list knowledge carries freshness metadata; relays are re-queried only when that knowledge ages out (staleness policy to be settled by research)
- [ ] Crawl effort is never spent on pubkeys nobody in the reachable graph points to (spam islands stay unexplored)
- [ ] The follow graph (pubkeys, directed follow edges, freshness metadata) persists in a shared database the separate spam layer can read independently
- [ ] Runs as a long-running daemon: initial full crawl, then continuous staleness-driven refresh
- [ ] Observability: operator can see crawl coverage, staleness distribution, and relay health well enough to trust the daemon unattended

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

## Constraints

- **Tech stack**: Rust — performance and memory control for a large graph and thousands of concurrent relay connections
- **Architecture**: Shared database as the project boundary — the spam layer consumes the graph by reading the DB, not via an API or library
- **Scale**: Must handle low millions of pubkeys / hundreds of millions of edges — full reachable component, not hop-limited
- **Efficiency**: Each follow list fetched from relays roughly once; re-fetches only on staleness — relay goodwill and bandwidth are finite
- **Operations**: Single operator, self-hosted — favor simplicity over deployment polish

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| Rust for the crawler | Performance/memory control at millions of nodes, strong async story for relay connections | — Pending |
| Shared database as spam-layer boundary | Loose coupling between projects via schema; no API service to maintain | — Pending |
| Full reachable set (no hop limit) | Crawl scope is structural reachability; the short trust walk lives in the spam layer, not the crawler | — Pending |
| Hybrid relay strategy (curated set + NIP-65 fallback) | Curated relays catch most users cheaply; outbox hints recover the rest | — Pending |
| Database engine: research to recommend | Postgres vs SQLite vs alternatives needs weighing against edge volume and cross-project read access | — Pending |
| Staleness policy: research to recommend | Uniform TTL vs adaptive refresh needs grounding in real kind-3 update behavior | — Pending |
| v1 includes observability | An unattended daemon you can't inspect can't be trusted; metrics are part of done, not a follow-up | — Pending |

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
*Last updated: 2026-06-15 — Phase 4 (Daemon, Staleness Loop & Observability) complete*
