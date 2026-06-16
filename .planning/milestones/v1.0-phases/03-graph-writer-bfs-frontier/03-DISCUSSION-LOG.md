# Phase 3: Graph Writer & BFS Frontier - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-06-13
**Phase:** 3-Graph Writer & BFS Frontier
**Areas discussed:** Frontier representation, Claim & crash recovery, Fetch unit / batching, Failure & retry policy

---

## Frontier representation

### Q1 — How should the DB-resident frontier be represented?

| Option | Description | Selected |
|--------|-------------|----------|
| Status-driven on pubkeys | No new table; frontier IS pubkeys.status='discovered' on the existing partial index. Claim transitions status. One additive migration for an 'in_progress' status/claimed_at. | ✓ |
| Dedicated frontier table | Separate frontier(pubkey_id, enqueued_at, lease_until,...); cleaner queue/graph separation but a second structure to keep consistent. | |
| Let me explain | User-described shape. | |

**User's choice:** Status-driven on pubkeys.

### Q2 — Does traversal order matter, or just full reachable coverage?

| Option | Description | Selected |
|--------|-------------|----------|
| Order-agnostic coverage | Reach every reachable pubkey once; order doesn't matter; claim any 'discovered' row; no sequence column. | ✓ |
| Strict breadth-first levels | Process hop-1 fully before hop-2; needs depth/ordering column + ORDER BY; costly at scale. | |
| Approximate BFS (enqueued_at) | Roughly FIFO by discovered timestamp; cheap middle ground, no strict levels. | |

**User's choice:** Order-agnostic coverage.

---

## Claim & crash recovery

### Q1 — How to claim, and what happens to in-flight work on crash?

| Option | Description | Selected |
|--------|-------------|----------|
| Persisted lease + reclaim | 'discovered'→'in_progress' + claimed_at in a short FOR UPDATE SKIP LOCKED txn; lock not held during fetch; startup reclaim of stale 'in_progress'; at-least-once on crash (writer idempotent). | ✓ |
| Lock-held-for-fetch | Claim+fetch inside one txn holding the row lock for the whole fetch; no new status/reclaim, but holds a long txn — fights pool sizing/bloat at high concurrency. | |
| Let me explain | User-described model. | |

**User's choice:** Persisted lease + reclaim.

### Q2 — When should stale 'in_progress' leases be reclaimed?

| Option | Description | Selected |
|--------|-------------|----------|
| Startup sweep only | Reclaim 'in_progress' on start; fully satisfies CRAWL-03 in Phase 3; continuous sweep is Phase 4 daemon territory. | ✓ |
| Startup + periodic sweep | Also periodic in-run reclaim for workers that die while process lives; more robust but Phase 4 territory. | |
| You decide | Planner chooses based on worker structure. | |

**User's choice:** Startup sweep only.

---

## Fetch unit / batching

### Q1 — Unit of work a worker claims and fetches at once?

| Option | Description | Selected |
|--------|-------------|----------|
| Batch of N authors | Claim N pubkeys, fetch as one author-chunked request set (existing machinery); N a config knob; fewer round-trips; partial-failure needs per-author status. | ✓ |
| One pubkey per fetch | Single pubkey at a time; simplest status logic but wastes author-chunking and multiplies round-trips. | |
| You decide | Planner picks based on per-author outcome mapping. | |

**User's choice:** Batch of N authors.

### Q2 — How to use the curated relay set for a claimed batch?

| Option | Description | Selected |
|--------|-------------|----------|
| Fan out to all curated relays, union, ingest once | Fetch batch from every relay, union raw events, ingest once; newest-wins across relays; ingest already cross-relay dedups. | ✓ |
| Sequential relays, stop when satisfied | Query one relay at a time, stop when batch satisfied; fewer requests but 'satisfied' ill-defined, risks stale list, weakens newest-wins. | |
| You decide | Planner chooses fan-out/union strategy. | |

**User's choice:** Fan out to all curated relays, union, then ingest once.

---

## Failure & retry policy

### Q1 — Retry within the crawl, or record terminal status and move on?

| Option | Description | Selected |
|--------|-------------|----------|
| Record terminal status, no in-crawl retry | not_found/failed recorded; Phase 4 staleness re-enqueues; forward-only, simple. | |
| Bounded in-crawl retry for errors only | N retries / bounded requeue for transient errors before 'failed'; not_found terminal; adds attempt counter. | |
| You decide | Planner decides retry depth. | |
| **(Other — user free text)** | **Both in-crawl retry for errors, AND then record terminal status for NIP-65 fallback attempt (Phase 5).** | ✓ |

**User's choice:** (Other) Both — bounded in-crawl retry for transient errors, then record a terminal status that the Phase 5 NIP-65 fallback (and Phase 4 staleness) can pick up.
**Notes:** not_found is the specific NIP-65 fallback target; failed is the errored-out terminal state. Both are re-enqueued by Phase 4 via the existing partial index.

### Q2 — How should the in-crawl retry be bounded and structured?

| Option | Description | Selected |
|--------|-------------|----------|
| Fixed attempt cap, requeue to 'discovered' | Transient error → return to 'discovered' + bump per-pubkey attempt counter; after configurable max (~3) → 'failed'; needs a fetch_attempts column; spreads retries out. | ✓ |
| Immediate in-batch retries | Retry failing relays a few times in the same fetch; no new column but clusters retries in time and blocks batch completion. | |
| You decide | Planner chooses requeue vs immediate. | |

**User's choice:** Fixed attempt cap, requeue to 'discovered'.

---

## Claude's Discretion

- Worker-pool / bounded-concurrency mechanism (CRAWL-04), batch size N default, retry-cap default, claim batch SQL, whether `in_progress` is exposed in `pubkey_freshness` (lean: not exposed), and GRAPH-02 verification test construction.

## Deferred Ideas

- Continuous (in-run) stale-lease reclaim sweep → Phase 4 daemon loop.
- Config-file sourcing of anchor / batch size / concurrency / retry caps → OPS-01, Phase 4.
- NIP-65 fallback for `not_found` pubkeys → Phase 5 (RELAY-05).
- Staleness/TTL re-enqueue of `failed`/`not_found` → Phase 4 (FRESH-02).
