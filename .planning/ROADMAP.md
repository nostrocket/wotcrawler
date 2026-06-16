# Roadmap: Nostr Web-of-Trust Crawler & Data Layer

## Overview

From one anchor pubkey, this project builds and continuously refreshes a complete directed follow graph of everyone reachable through nostr kind-3 follows, persisted in a shared PostgreSQL database the downstream spam layer reads directly.

## Milestones

- ✅ **v1.0 Crawler & Data Layer** — Phases 1–5 (shipped 2026-06-16) — full graph crawler + data layer: schema/contract, relay acquisition + validation, transactional graph writer + crash-safe BFS frontier, unattended daemon with staleness loop + observability, and NIP-65 outbox-routing fallback + relay health scoring. Full detail: [milestones/v1.0-ROADMAP.md](milestones/v1.0-ROADMAP.md).

## Phases

<details>
<summary>✅ v1.0 Crawler & Data Layer (Phases 1–5) — SHIPPED 2026-06-16</summary>

- [x] Phase 1: Schema & Data Contract (3/3 plans) — completed 2026-06-12
- [x] Phase 2: Relay Acquisition & Validation (12/12 plans) — completed 2026-06-13
- [x] Phase 3: Graph Writer & BFS Frontier (3/3 plans) — completed 2026-06-13
- [x] Phase 4: Daemon, Staleness Loop & Observability (5/5 plans) — completed 2026-06-15
- [x] Phase 5: NIP-65 Outbox Routing & Relay Health (4/4 plans) — completed 2026-06-16

Full phase details, success criteria, and milestone summary: [milestones/v1.0-ROADMAP.md](milestones/v1.0-ROADMAP.md).
Audit: [milestones/v1.0-MILESTONE-AUDIT.md](milestones/v1.0-MILESTONE-AUDIT.md) (status: tech_debt — 29/29 requirements satisfied, operator UAT deferred).

</details>

## Progress

| Phase | Milestone | Plans Complete | Status | Completed |
|-------|-----------|----------------|--------|-----------|
| 1. Schema & Data Contract | v1.0 | 3/3 | Complete | 2026-06-12 |
| 2. Relay Acquisition & Validation | v1.0 | 12/12 | Complete | 2026-06-13 |
| 3. Graph Writer & BFS Frontier | v1.0 | 3/3 | Complete | 2026-06-13 |
| 4. Daemon, Staleness Loop & Observability | v1.0 | 5/5 | Complete | 2026-06-15 |
| 5. NIP-65 Outbox Routing & Relay Health | v1.0 | 4/4 | Complete | 2026-06-16 |

## Deferred to operator UAT (v1.0)

- Live-relay crawl run against real curated relays (Phase 4/5) — every automatable criterion passed in-session.
- Grafana dashboard render against a live Prometheus (OBS-05).

See `.planning/STATE.md` → Deferred Items.
