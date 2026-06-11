# Project Research Summary

**Project:** Nostr Web-of-Trust Crawler & Data Layer
**Domain:** Distributed social-graph crawler / large-graph data layer (nostr)
**Researched:** 2026-06-11
**Confidence:** MEDIUM-HIGH

## Executive Summary

This project is a single-operator, unattended Rust daemon that BFS-crawls nostr kind-3 follow lists from a single anchor pubkey, persists a directed follow-graph into a shared PostgreSQL database, and keeps it fresh via staleness-driven re-fetches. The downstream consumer (a separate spam-scoring layer) reads the shared DB directly — the schema is the API boundary, not an HTTP service. The closest comparable system is Vertex `crawler_v2` (Go, 24/7, kind-3 + kind-10002, SQLite + Redis), but this project strips scoring and DVM exposure entirely and focuses on doing the data job better: honest graph, freshness-driven refresh, relay health tracking, and first-class observability.

The recommended approach is `nostr-sdk 0.44` on Tokio for the relay layer, `sqlx 0.9` against PostgreSQL 16/17 for persistence, and a clean two-halves architecture: an *acquisition half* (relay I/O, signature verification, replaceable-event dedup) decoupled from a *graph half* (edge diffing, frontier management, freshness) via a bounded `mpsc` channel. The BFS frontier must live in the DB from day one — not in memory — because a full reachable crawl will take days and will be interrupted. Crash/resume falls out free when frontier state is DB-resident.

The four critical risks: (1) future-dated `created_at` timestamps permanently pinning follow lists to junk — clamp at ingest; (2) unverified relay output corrupting the graph — secp256k1-verify every event before storing; (3) full edge-rewrite on replacement causing catastrophic write amplification — edge-diff from day one; (4) relay rate-limiting/banning from aggressive uncapped subscriptions — read NIP-11 limits and enforce politeness before scaling concurrency.

## Key Findings

### Recommended Stack

The Rust nostr ecosystem has a clear canonical choice (`rust-nostr`), and the database decision is driven unambiguously by the project's cross-process shared-read boundary: PostgreSQL's MVCC gives the spam layer lock-free reads while the crawler writes continuously. The crawler does BFS in application memory, so Postgres's recursive-traversal weakness is irrelevant — the DB only does indexed adjacency lookups and freshness-filtered batch reads.

**Core technologies:**
- `nostr-sdk` 0.44.1: protocol layer — canonical maintained Rust nostr library; handles WebSocket lifecycle, signature verification, NIP-65 gossip, replaceable events. Do not reimplement.
- `tokio` 1.x: async runtime — required by nostr-sdk; handles thousands of concurrent relay sockets
- **PostgreSQL 16/17** via `sqlx` 0.9 (FIRM recommendation): persistence — MVCC for concurrent cross-process reads, `COPY` for bulk edge ingest, SQL schema as the stable public API. Surrogate `bigint` IDs in the edge table (not 32-byte pubkey blobs) are required at this scale. SQLite loses on multi-process write/read contention; RocksDB/LMDB/redb lose on the cross-project SQL contract.
- `tracing` + `metrics` + `metrics-exporter-prometheus`: observability — structured logs plus a Prometheus scrape endpoint for coverage/staleness/relay health
- `governor` 0.10: per-relay rate limiting / politeness

### Expected Features

**Must have (table stakes):**
- Event dedup + per-event signature verification — relays don't verify; forged events poison the graph
- Replaceable-event resolution — newest `created_at` wins, future-timestamp clamp, same-timestamp lowest-id tie-break
- Reachability-gated BFS frontier — only followees of accepted pubkeys ever get enqueued (spam islands stay unexplored structurally)
- Curated relay set + connection management — backoff, reconnect, EOSE handling, NIP-11 limit awareness, pagination
- Bounded concurrency / backpressure — bounded channels end-to-end
- Durable graph + per-pubkey freshness in the DB — crash/resume falls out free
- Staleness refresh loop — uniform TTL first; adaptive only after churn data exists
- Observability — coverage %, staleness histogram, relay health, queue depth

**Should have (competitive, v1.x):**
- NIP-65 outbox routing fallback — highest-value differentiator; needs a kind:10002 crawl + routing table
- Dead/degraded relay scoring + adaptive per-relay concurrency
- Adaptive refresh policy — grounded in observed kind-3 churn

**Defer (v2+):**
- NIP-77 negentropy bulk sync — only ~16% of relays support it
- Streaming live kind-3 subscriptions

**Never build (anti-features):**
- Scoring/trust propagation in the crawler — separate project
- Content/note fetching — graph only
- API surface — the DB schema is the contract

### Architecture Approach

Two halves with a hard boundary: an acquisition half (relay pool → validator/deduplicator) and a graph half (graph writer → durable frontier → staleness scanner), connected by a single bounded `mpsc<AcceptedFollowList>` channel. Relay code never touches the graph; graph code never awaits a socket. The BFS feedback loop is the graph writer enqueuing newly-discovered followees. Initial crawl and staleness refresh are one loop, not two: a unified durable frontier (`crawl_queue` in Postgres, leased via `FOR UPDATE SKIP LOCKED`) fed by two producers — the writer (new discoveries) and the staleness scanner (expired pubkeys). Edge-list diffing on kind-3 replacement (added/removed deltas in one transaction, with a same-event-id no-op short-circuit) is v1 architecture, not a later optimization.

**Major components:**
1. Relay connection pool — websocket lifecycle, NIP-11 awareness, per-relay backoff and rate limiting
2. Validator/deduplicator — signature verification, replaceable-event resolution, timestamp clamping, p-tag bounds
3. Graph writer — transactional edge diffing, freshness metadata updates
4. Durable frontier + scheduler — DB-resident BFS queue, SKIP LOCKED leasing, crash/resume
5. Staleness scanner — enqueues expired pubkeys into the same frontier
6. Metrics/observability — Prometheus endpoint tapping all components

### Critical Pitfalls

1. **Future-dated `created_at` pins follow lists to garbage** — `created_at` is adversary-controlled; one junk event blocks all real updates. Clamp future timestamps (reject >1h ahead) and implement the tie-break before the first crawl.
2. **Trusting relay output** — relays don't verify signatures and can forge events. Verify every event; recovery from skipping this is a full re-crawl.
3. **In-memory-only frontier** — a multi-day crawl that can't resume re-hammers relays and burns goodwill. Frontier, freshness, and visited state must be DB-resident from day one.
4. **EOSE ≠ complete** — relays cap results (often ~500) regardless of your `limit`. Paginate via `until` and chunk authors under `max_limit`, or silently lose pubkeys.
5. **Write amplification** — delete-all-then-reinsert on every refresh bloats Postgres and degrades the spam layer's concurrent reads. Edge-diff + id-equal short-circuit in v1.

## Implications for Roadmap

Research suggests this phase structure:

**Phase 1: Schema and Data Contract** — PostgreSQL schema (pubkeys with surrogate bigint IDs, directed edges, freshness columns, crawl queue), sqlx store layer, versioned migrations. Rationale: the schema is the spam layer's public API and gates everything; migrations on hundreds of millions of rows are expensive.

**Phase 2: Relay Acquisition and Validation** — relay pool wrapper, curated-set fetch, NIP-11 pagination, signature verification, replaceable-event dedup with timestamp clamp, backoff/reconnect, bounded channel. Rationale: the hardest external-integration risk, validated in isolation before coupling to persistence.

**Phase 3: Graph Writer and BFS Frontier** — transactional edge-diff writer, SKIP LOCKED durable frontier, BFS followee-enqueue loop, crash/resume. Rationale: completes the fetch→store vertical slice; edge-diffing and durable frontier are co-designed here to avoid the "bolt on later" trap.

**Phase 4: Daemon, Staleness Loop, and Observability** — scheduler wiring everything into one daemon, staleness scanner as second frontier producer, Prometheus metrics (coverage, staleness distribution, relay health, queue depth). Rationale: observability is part of v1 "done" per project constraints — it's how the operator trusts the daemon unattended.

**Phase 5: NIP-65 Outbox Routing and Relay Health** — kind:10002 crawl, routing table, fallback fetch path, relay scoring, adaptive concurrency. Rationale: deferred until Phase 4 metrics measure the coverage gap that justifies it.

**Phase 6: Adaptive Refresh** — per-pubkey refresh intervals based on observed churn. Rationale: kind-3 churn frequency has no public data; must follow weeks of operational instrumentation.

**Phase ordering rationale:** schema first (public API stability) → relay acquisition isolated (external risk) → graph writer + frontier co-designed (correctness traps) → daemon + observability (v1 requirement) → measured optimizations last (NIP-65, adaptive refresh).

**Research flags:** Phases 2, 3, 5, 6 warrant phase research (relay-pool quirks at scale, SKIP LOCKED queue patterns, routing-table design, refresh-policy design). Phases 1 and 4 are standard patterns.

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | Crate versions verified against crates.io on 2026-06-11 |
| Features | MEDIUM-HIGH | Table stakes from NIP specs (HIGH); differentiator sizing MEDIUM |
| Architecture | MEDIUM-HIGH | Grounded in prior art (Vertex crawler_v2) and nostr-sdk docs; scale numbers MEDIUM |
| Pitfalls | MEDIUM-HIGH | Protocol semantics HIGH; operational lessons synthesized from limited crawler post-mortem corpus |

**Overall: MEDIUM-HIGH**

**Known gaps:**
- **Kind-3 churn frequency** — no public data exists; must be instrumented from live operation before adaptive refresh can be designed. Do not assume.
- **Curated relay set coverage** — the fraction of reachable pubkeys discoverable without NIP-65 fallback is unknown until observability measures it; directly gates Phase 5 scope.
- **Initial crawl duration / resource profile at full scale** — plan for a multi-day initial crawl; instrument early.

---
*Researched: 2026-06-11*
*Sources: rust-nostr/nostr-sdk docs & crates.io, NIP-01/02/11/65/77 specs, vertex-lab crawler_v2, brainstorm, strfry, arxiv 2402.05709 (empirical nostr network analysis) — full citations in STACK.md, FEATURES.md, ARCHITECTURE.md, PITFALLS.md*
