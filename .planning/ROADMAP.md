# Roadmap: Nostr Web-of-Trust Crawler & Data Layer

## Overview

From one anchor pubkey, this project builds and continuously refreshes a complete directed follow graph of everyone reachable through nostr kind-3 follows, persisted in a shared PostgreSQL database the downstream spam layer reads directly. The journey starts by nailing the data contract (the schema is the public API), then proves the hardest external risk in isolation (relay acquisition + signature/replaceable-event validation), then closes the fetch→store loop with a transactional edge-diff writer and a DB-resident BFS frontier. Phase 4 wires everything into one unattended daemon with the staleness loop and the observability that makes "running unattended" trustworthy. Phase 5 adds NIP-65 outbox-routing fallback and relay health scoring to recover the pubkeys the curated set misses and to route around degraded relays.

## Phases

**Phase Numbering:**

- Integer phases (1, 2, 3): Planned milestone work
- Decimal phases (2.1, 2.2): Urgent insertions (marked with INSERTED)

Decimal phases appear between their surrounding integers in numeric order.

- [x] **Phase 1: Schema & Data Contract** - PostgreSQL graph schema, sqlx store layer, versioned migrations, documented public contract (completed 2026-06-12)
- [ ] **Phase 2: Relay Acquisition & Validation** - Curated relay pool with backoff/NIP-11 awareness feeding a signature-verifying, replaceable-event-resolving validator
- [ ] **Phase 3: Graph Writer & BFS Frontier** - Transactional edge-diff writer and DB-resident reachability-gated BFS frontier that survives restarts
- [ ] **Phase 4: Daemon, Staleness Loop & Observability** - One long-running daemon with TTL-driven refresh and the metrics/logging/health surface an operator trusts unattended
- [ ] **Phase 5: NIP-65 Outbox Routing & Relay Health** - kind:10002 routing-table fallback for missed pubkeys plus per-relay health scoring driving routing and concurrency

## Phase Details

### Phase 1: Schema & Data Contract

**Goal**: The shared PostgreSQL database the spam layer consumes exists with its schema established as the documented public contract, and the crawler can read/write it concurrently with other processes.
**Depends on**: Nothing (first phase)
**Requirements**: GRAPH-01, GRAPH-03, GRAPH-04
**Success Criteria** (what must be TRUE):

  1. A fresh database can be brought to the current schema by running versioned migrations from empty, and re-running them is a no-op.
  2. The schema stores pubkeys with surrogate bigint IDs, directed follow edges keyed on those IDs, and per-pubkey freshness columns.
  3. A second process can run read queries against the graph while the crawler's store layer writes, with neither blocking the other.
  4. A committed schema document describes every table and column a downstream consumer reads, sufficient for the spam layer to query without reading crawler code.**Plans**: 3 plans

**Wave 1**

  - [x] 01-01-PLAN.md — Toolchain + project scaffold + testcontainers Postgres fixture

**Wave 2** *(blocked on Wave 1 completion)*

  - [x] 01-02-PLAN.md — Idempotent graph schema migration + contract views + COMMENT ON + migration/contract tests

**Wave 3** *(blocked on Wave 2 completion)*

  - [x] 01-03-PLAN.md — sqlx store layer (pubkeys + edge-diff writer) + concurrency test + SCHEMA.md + .sqlx metadata

### Phase 2: Relay Acquisition & Validation

**Goal**: The crawler can pull kind-3 and kind:10002 events from a curated relay set politely and completely, and only correct, deduplicated, newest-wins follow lists emerge from the acquisition half.
**Depends on**: Phase 1
**Requirements**: RELAY-01, RELAY-02, RELAY-03, RELAY-04, INGEST-01, INGEST-02, INGEST-03, INGEST-04, INGEST-05
**Success Criteria** (what must be TRUE):

  1. The crawler maintains connections to a configurable curated relay set, automatically reconnecting with exponential backoff and jitter after drops.
  2. Fetches read each relay's NIP-11 limits and paginate (until-windows, author chunking) so capped result sets do not silently drop pubkeys, and EOSE is never treated as proof of completeness.
  3. Every event's signature is verified before it is accepted; invalid events are discarded and counted, and duplicate ids arriving from multiple relays are processed at most once.
  4. For each pubkey only the newest valid kind-3 (and kind:10002) is applied — future-dated created_at beyond the configurable clamp is rejected and same-timestamp ties break to the lowest event id.
  5. Malformed p-tags are skipped and oversized follow lists are bounded by a configurable cap without crashing the pipeline; per-relay rate limiting keeps request rates polite and rate-limit notices trigger backoff.

**Plans**: 4 plans

**Wave 1**

  - [x] 02-01-PLAN.md — Deps + module/error/ValidatedFollowList skeleton + event fixtures + RELAY-01/RELAY-02 API spikes

**Wave 2** *(blocked on Wave 1 completion; 02-02 and 02-03 run in parallel — disjoint files)*

  - [x] 02-02-PLAN.md — Ingest validation: verify gate, dedup, replaceable resolution, p-tag bounds (INGEST-01..05)
  - [x] 02-03-PLAN.md — Relay acquisition: reconnect+backoff, NIP-11 cache, governor rate limit, until-window pagination (RELAY-01..04)

**Wave 3** *(blocked on Wave 2; wires the two halves together)*

  - [x] 02-04-PLAN.md — Relay→ingest pipeline seam: fetch output through the ingest gate so ValidatedFollowList emerges end-to-end, proven by acquire_pipeline E2E test (RELAY-03 + INGEST-01..05)

### Phase 3: Graph Writer & BFS Frontier

**Goal**: Accepted follow lists become durable graph state via transactional edge diffs, and a DB-resident reachability-gated BFS frontier drives discovery and survives crashes without redoing completed work.
**Depends on**: Phase 2
**Requirements**: GRAPH-02, CRAWL-01, CRAWL-02, CRAWL-03, CRAWL-04, FRESH-01
**Success Criteria** (what must be TRUE):

  1. Applying a replacing kind-3 inserts only added edges and deletes only removed edges in one transaction, and re-applying the same event id touches zero edge rows.
  2. A crawl starting from a configurable anchor pubkey discovers reachable pubkeys via BFS, enqueuing only pubkeys followed by someone already in the graph (spam islands stay unexplored).
  3. Killing the crawler mid-crawl and restarting it resumes from the DB-resident frontier without refetching already-completed pubkeys.
  4. In-flight fetch concurrency is bounded end-to-end, so the frontier and queues do not grow without limit under load.
  5. Every pubkey records when its follow-list knowledge was last acquired or confirmed.

**Plans**: TBD

### Phase 4: Daemon, Staleness Loop & Observability

**Goal**: A single configurable daemon binary runs the initial crawl then continuous TTL-driven refresh, shuts down gracefully, and exposes enough metrics, logs, and health signals for an operator to trust it running unattended for days.
**Depends on**: Phase 3
**Requirements**: FRESH-02, FRESH-03, OBS-01, OBS-02, OBS-03, OBS-04, OBS-05, OPS-01, OPS-02
**Success Criteria** (what must be TRUE):

  1. One Rust daemon binary, configured via a config file (anchor pubkey, relay set, TTL, DB URL, concurrency caps), runs the initial crawl and then a continuous staleness-driven refresh.
  2. A staleness scanner enqueues pubkeys whose knowledge exceeds the configurable uniform TTL into the same frontier the initial crawl uses, and each refresh records whether the follow list actually changed.
  3. A Prometheus /metrics endpoint exposes crawl coverage, staleness distribution, relay health, frontier depth, fetch rate, and validation-failure counts, and a committed Grafana dashboard JSON renders them.
  4. An HTTP liveness/readiness endpoint reports daemon health to a supervisor, structured tracing logs at configurable levels, and periodic progress summaries (frontier size, fetch rate, coverage %) appear during a long crawl.
  5. Sending a shutdown signal drains in-flight work and leaves the database in a consistent state with no orphaned leases.

**Plans**: TBD

### Phase 5: NIP-65 Outbox Routing & Relay Health

**Goal**: Pubkeys the curated set cannot supply are recovered via their advertised NIP-65 write relays, and observed relay behavior drives routing and per-relay concurrency so the crawler steers around degraded relays.
**Depends on**: Phase 4
**Requirements**: RELAY-05, RELAY-06
**Success Criteria** (what must be TRUE):

  1. When a pubkey's kind-3 is not found on the curated relay set, the crawler falls back to fetching from that pubkey's NIP-65 write relays drawn from ingested kind:10002 data.
  2. Each relay carries a health score derived from observed behavior (connect failures, timeouts, rate-limit hits, response latency).
  3. The health score visibly drives routing decisions and per-relay concurrency, so a degraded relay receives less traffic than a healthy one.

## Progress

**Execution Order:**
Phases execute in numeric order: 1 → 2 → 3 → 4 → 5

| Phase | Plans Complete | Status | Completed |
|-------|----------------|--------|-----------|
| 1. Schema & Data Contract | 3/3 | Complete   | 2026-06-12 |
| 2. Relay Acquisition & Validation | 4/4 | Complete | 2026-06-12 |
| 3. Graph Writer & BFS Frontier | 0/TBD | Not started | - |
| 4. Daemon, Staleness Loop & Observability | 0/TBD | Not started | - |
| 5. NIP-65 Outbox Routing & Relay Health | 0/TBD | Not started | - |
