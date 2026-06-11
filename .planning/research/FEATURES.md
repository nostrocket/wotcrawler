# Feature Research

**Domain:** Nostr social-graph crawler / web-of-trust data layer (kind-3 follow-graph builder feeding a downstream spam-scoring project)
**Researched:** 2026-06-11
**Confidence:** MEDIUM-HIGH

Closest comparable system is **Vertex `crawler_v2`** (Go: 24/7 kind-3 crawler, seeded relays + kind:10002 discovery, SQLite event store + Redis graph, backoff/retry, Monte Carlo PageRank). Other reference points: **Brainstorm/Grapevine** (personalized WoT scoring on top of a crawled graph), **strfry + NIP-77 negentropy** (bulk set reconciliation), the **NIP-65 outbox/gossip model** (relay routing), and generic nostr relay-pool libraries (reconnection/backoff). Our scope is narrower than all of these: build and maintain the *graph + freshness*, not the *scoring*.

## Feature Landscape

### Table Stakes (Get These Wrong = Garbage Graph or a Daemon That Falls Over)

These are the non-negotiables. Every comparable crawler implements them; skipping any one produces a corrupt graph, wasted relay goodwill, or a process that dies overnight.

| Feature | Why Expected | Complexity | Notes |
|---------|--------------|------------|-------|
| Replaceable-event resolution (newest kind-3 wins) | Kind 3 is replaceable (`created_at` + pubkey = identity); different relays hold different versions. Keeping anything but the newest produces a wrong graph. | MEDIUM | Compare `created_at`; tie-break by lowest event id per NIP-01. Must be enforced at write time, not just read time. Edges = the p-tags of the winning event *only*. |
| Event signature + structure validation | Relays serve unsigned/forged/malformed events. Trusting them poisons the graph an adversarial downstream depends on. | MEDIUM | secp256k1 schnorr verify on every kind-3 before it counts. Reject malformed p-tags. Rust crates exist (e.g. nostr-sdk / secp256k1). |
| Event dedup | Same event arrives from many relays simultaneously. Without dedup you re-verify and re-write constantly. | LOW | Dedup by event id (seen-set) and by (pubkey, kind) for replaceable resolution. |
| Frontier/queue management (BFS expansion) | Core mechanic: each fetched follow list yields new pubkeys to fetch. Must dedup the frontier so each pubkey is fetched ~once. | MEDIUM | "Discovered but not fetched" vs "fetched" state per pubkey. This is the engine of the crawl. |
| Reachability-gated expansion | PROJECT requirement: never spend effort on pubkeys nobody reachable points to. Spam islands must stay unexplored. | LOW-MEDIUM | Falls out of BFS naturally — only enqueue pubkeys that appear as a p-tag of an already-reached node. Document it as an explicit invariant. |
| Relay connection management: reconnect w/ exponential backoff | Relays drop connections constantly. A crawler that doesn't reconnect silently stalls. | MEDIUM | Standard: initial ~1s, cap ~60s, multiplier ~2. Per-relay connection state machine. Every relay-pool lib does this; Vertex notes "retries and backoff." |
| Subscription lifecycle (REQ/EOSE/CLOSE) handling | Must know when a relay has returned everything (EOSE) to advance, and close subs to free relay resources. | MEDIUM | Track per-subscription state; respect relay `CLOSED` messages and NIP-01 limits. |
| Backpressure / concurrency bounding | Thousands of concurrent websockets + millions of pubkeys will OOM or get the crawler rate-limited/banned if unbounded. | HIGH | Bounded worker pool, per-relay in-flight cap, channel-based queue with limits. Rust async (tokio) is well suited; this is a primary reason Rust was chosen. |
| Checkpoint / resume (durable crawl state) | A multi-day initial crawl of millions of pubkeys *will* be interrupted. Restarting from the anchor every time is unacceptable. | MEDIUM | Frontier + fetched-set + per-pubkey freshness must survive restart. Naturally falls out if state lives in the DB rather than memory. |
| Durable graph persistence (pubkeys, directed edges, freshness metadata) | This DB *is* the project's public API for the spam layer. | HIGH | Schema is load-bearing; edge volume (hundreds of millions) drives engine choice (see STACK research). Edges should be derived from winning kind-3 only. |
| Per-pubkey freshness metadata | PROJECT requirement: re-query only when knowledge ages out. Needs last-fetched-at, last-seen `created_at`, fetch outcome. | LOW-MEDIUM | The data model behind the whole staleness story. Must be queryable for the refresh scheduler and observability. |
| Staleness-driven refresh loop | After initial crawl, the daemon's steady-state job is re-checking stale pubkeys. | MEDIUM | Even a naive uniform-TTL implementation is table stakes; *adaptive* is the differentiator (below). |
| Seeded curated relay set | Vertex seeds from a relay list; you can't discover relays before you've fetched anything. Curated set is the workhorse. | LOW | Config-file list of high-coverage relays (PROJECT's hybrid strategy). |
| Graceful per-relay error handling | One bad/slow/hostile relay must not stall or crash the crawl. | MEDIUM | Timeouts, isolate failures per relay, continue with others. |
| Basic observability (logs + liveness) | Single operator must know it's alive and making progress. Minimum bar before "trust it unattended." | LOW | Structured logging at minimum; richer metrics are a differentiator. |

### Differentiators (Make It Efficient and Trustworthy Unattended)

These are where this project earns its keep. They align with Core Value: *each list fetched roughly once, refreshed only when stale, trustworthy unattended.*

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| NIP-65 outbox routing as fallback | PROJECT's explicit hybrid strategy: when a pubkey isn't on curated relays, use its kind:10002 write relays. Recovers the long tail without hammering every relay for every pubkey. | HIGH | Requires also crawling kind:10002, maintaining a pubkey→write-relays routing table, and an author-set-cover step to minimize relay connections. Vertex discovers relays this way. Biggest single differentiator. |
| Adaptive refresh / staleness policy | Uniform TTL either wastes fetches on dormant accounts or lets active ones go stale. Adaptive cadence (based on observed update frequency) directly serves "fetch roughly once, refresh only when stale." | HIGH | OPEN decision in PROJECT — must be grounded in real kind-3 churn (no public churn stats found; instrument and learn). Start with uniform TTL, evolve to per-pubkey adaptive. Depends on freshness metadata history. |
| Dead/degraded-relay scoring | Relays go offline, lie, rate-limit, or serve stale data. Scoring lets the crawler deprioritize bad relays and lean on good ones — protects relay goodwill and crawl speed. | MEDIUM | Track per-relay: connect success rate, EOSE latency, events/connection, error rate. Demote or pause low scorers. No comparable system documents this richly — genuine differentiator. |
| NIP-77 negentropy bulk sync | For relays that support it (~16% of reachable relays, incl. strfry), set reconciliation transfers only the *diff*, dramatically cheaper than re-REQ-ing millions of authors. Massive bandwidth/goodwill win on the initial crawl. | HIGH | NIP-77 / strfry negentropy. Optional fast path; must fall back to plain REQ for non-supporting relays. High payoff at this scale. |
| Rich observability: coverage, staleness distribution, relay health | PROJECT explicitly makes this part of "done." An unattended daemon you can't inspect can't be trusted. | MEDIUM | Metrics endpoint (Prometheus-style): pubkeys discovered/fetched/stale, staleness histogram, per-relay health, queue depth, fetch rate. This is what separates "trustworthy" from "hope it's fine." |
| Idempotent, crash-safe writes | Beyond basic checkpointing: ensure interrupted writes never corrupt the graph the spam layer reads live. | MEDIUM | Transactional upserts keyed on (pubkey); writers and the external reader share the DB concurrently. |
| Adaptive concurrency / rate control per relay | Push throughput up where relays tolerate it, back off where they don't — without manual tuning. | MEDIUM | Pairs with dead-relay scoring; turns backpressure from a safety floor into an efficiency lever. |
| Incremental graph availability | Spam layer can read a partially-crawled-but-honest graph rather than waiting for a full crawl. | LOW-MEDIUM | Mostly a consequence of writing edges as they're resolved + honest freshness metadata; document the read contract. |

### Anti-Features (Tempting, but Out of Scope or Actively Harmful)

| Feature | Why Requested | Why Problematic | Alternative |
|---------|---------------|-----------------|-------------|
| Trust propagation / spam scoring in the crawler | "We have the graph, just score it here." | Explicit PROJECT out-of-scope; couples two codebases, bloats the daemon, and conflates honest-data-collection with opinionated scoring. | Keep crawler scoring-free; spam layer reads the shared DB. |
| Multi-anchor / personalized graphs | Vertex/Brainstorm do personalized PageRank from many roots. | Explicit out-of-scope. Multiplies storage and crawl cost; the short trust walk lives in the spam layer. | Single configurable anchor; revisit only if spam layer demands it. |
| Hop-limited / depth-capped crawl | Seems cheaper and safer. | Contradicts requirement: full *reachable* set, not hop-limited. Depth caps silently drop legitimate distant accounts. | Reachability-gated BFS over the full component; bound cost via staleness + relay strategy, not depth. |
| Content / note fetching & analysis | "While we're connected, grab notes too." | Explicit out-of-scope; the system works from social structure only. Massively increases bandwidth, storage, and relay load. | Fetch only kind:3 and kind:10002. Nothing else. |
| Serving an API / DVM / acting as a relay | Vertex exposes NIP-90 DVMs; Brainstorm runs services. | Shared-DB *is* the boundary by design (PROJECT constraint). An API/relay is a separate service to maintain, secure, and scale. | Spam layer reads the DB directly. No API surface. |
| Storing full event history / every kind-3 version | "Audit trail / we might need old versions." | Replaceable semantics mean only newest counts; history is hundreds of millions of dead rows. | Store winning edges + freshness only; optionally keep last `created_at`, not full bodies. |
| Polished multi-tenant deployment (Docker images, install docs) | "Make it easy for others to run." | Explicit out-of-scope; single-operator infra. Premature packaging slows v1. | Config file + README. |
| Writing back to nostr / re-publishing | "Be a good network citizen / mirror data." | Read-only crawler is simpler, safer, and avoids spam/abuse concerns. | Read-only. Never publish. |
| Real-time streaming subscriptions for liveness | "Subscribe to all kind-3 forever for instant updates." | At millions of authors this is unbounded open subscriptions and relay-banning behavior; conflicts with bounded backpressure. | Staleness-driven polling; optional targeted live subs only for the anchor's close neighborhood if ever needed. |

## Feature Dependencies

```
Durable graph persistence + per-pubkey freshness metadata  (foundation)
    ├──requires──> Replaceable-event resolution (newest wins)
    │                   └──requires──> Signature/structure validation
    │                                       └──requires──> Event dedup
    ├──enables───> Checkpoint / resume        (state lives in DB)
    ├──enables───> Staleness-driven refresh loop
    │                   └──enhanced by──> Adaptive refresh policy
    └──enables───> Observability (coverage, staleness distribution)

Frontier/queue management (BFS)
    ├──requires──> Reachability-gated expansion (invariant)
    └──requires──> Backpressure / concurrency bounding

Relay connection mgmt (reconnect+backoff)
    ├──requires──> Subscription lifecycle (REQ/EOSE/CLOSE)
    ├──enhanced by──> Dead/degraded-relay scoring
    │                     └──enhances──> Adaptive concurrency per relay
    └──enables───> NIP-65 outbox routing (needs kind:10002 crawl + routing table)
                       └──enhances──> Coverage of the long tail

NIP-77 negentropy bulk sync ──enhances──> initial-crawl efficiency (optional fast path; falls back to REQ)

Content fetching ──conflicts──> bounded scope + relay goodwill   (anti-feature)
Real-time global subscriptions ──conflicts──> backpressure bounding   (anti-feature)
```

### Dependency Notes

- **Replaceable resolution requires validation requires dedup:** you can't pick "newest valid event" without verifying signatures and de-duplicating arrivals first. These form one ingest pipeline and should ship together.
- **Checkpoint/resume falls out of DB-resident state:** if frontier + fetched-set + freshness all live in the DB rather than memory, resume is nearly free. Designing state in-memory first and bolting on persistence later is the trap.
- **NIP-65 outbox routing depends on relay connection management AND a kind:10002 crawl:** it's a second discovery loop layered on the same connection machinery, plus a routing table and set-cover. It's the highest-value differentiator but also the heaviest — sequence it after a working curated-relay crawl.
- **Adaptive refresh depends on freshness-metadata history:** you cannot tune cadence without first recording fetch outcomes over time. Ship uniform TTL first, accumulate data, then make it adaptive.
- **Dead-relay scoring and adaptive concurrency reinforce each other:** scoring identifies good relays; adaptive concurrency exploits them. Both ride on per-relay health metrics that observability already needs.

## MVP Definition

### Launch With (v1)

The minimum that produces an *honest, complete-enough, resumable* graph the spam layer can read.

- [ ] Event dedup + signature/structure validation — or the graph is poisoned
- [ ] Replaceable-event resolution (newest kind-3 wins) — correctness of every edge
- [ ] Reachability-gated BFS frontier management — the crawl engine + spam-island avoidance
- [ ] Seeded curated relay set + connection management with backoff/reconnect — the workhorse fetch path
- [ ] Subscription lifecycle (REQ/EOSE/CLOSE) + per-relay error isolation — won't stall or crash
- [ ] Bounded concurrency / backpressure — won't OOM or get banned at scale
- [ ] Durable graph + per-pubkey freshness persistence (DB-resident state) — the public API + checkpoint/resume for free
- [ ] Staleness-driven refresh loop (uniform TTL to start) — steady-state behavior
- [ ] Basic observability: coverage, staleness distribution, relay health, queue depth — PROJECT makes this part of "done"

### Add After Validation (v1.x)

- [ ] NIP-65 outbox routing fallback — add once curated-relay coverage gaps are measured (observability reveals the long tail it recovers)
- [ ] Dead/degraded-relay scoring + adaptive per-relay concurrency — add once relay-health metrics show which relays are worth the connections
- [ ] Adaptive refresh policy — add once freshness history reveals real kind-3 churn patterns

### Future Consideration (v2+)

- [ ] NIP-77 negentropy bulk-sync fast path — defer until initial-crawl bandwidth/goodwill proves to be the bottleneck; only ~16% of relays support it, so it's an optimization, not a foundation
- [ ] Incremental/streaming live updates for the anchor's close neighborhood — only if the spam layer demands fresher-than-poll data for the inner graph

## Feature Prioritization Matrix

| Feature | User Value | Implementation Cost | Priority |
|---------|------------|---------------------|----------|
| Dedup + validation + replaceable resolution | HIGH | MEDIUM | P1 |
| Reachability-gated BFS frontier | HIGH | MEDIUM | P1 |
| Relay connection mgmt (reconnect/backoff/lifecycle) | HIGH | MEDIUM | P1 |
| Bounded concurrency / backpressure | HIGH | HIGH | P1 |
| Durable graph + freshness persistence | HIGH | HIGH | P1 |
| Checkpoint/resume (via DB-resident state) | HIGH | MEDIUM | P1 |
| Staleness refresh loop (uniform TTL) | HIGH | MEDIUM | P1 |
| Basic observability (coverage/staleness/relay health) | HIGH | MEDIUM | P1 |
| NIP-65 outbox routing fallback | HIGH | HIGH | P2 |
| Dead-relay scoring + adaptive concurrency | MEDIUM | MEDIUM | P2 |
| Adaptive refresh policy | MEDIUM | HIGH | P2 |
| NIP-77 negentropy bulk sync | MEDIUM | HIGH | P3 |
| Streaming live updates (inner graph) | LOW | MEDIUM | P3 |

**Priority key:** P1 = must have for launch · P2 = add when possible · P3 = future.

## Competitor Feature Analysis

| Feature | Vertex `crawler_v2` | Brainstorm/Grapevine | strfry / NIP-77 | Our Approach |
|---------|---------------------|----------------------|-----------------|--------------|
| Discovery | Seed pubkeys → BFS via kind:3 | Crawls graph from anchor(s) for scoring | n/a (relay) | Single anchor → reachability-gated BFS |
| Relay discovery | Seed list + kind:10002 dynamic | Uses relay sets | n/a | Curated set (workhorse) + NIP-65 fallback |
| Replaceable handling | Rank-filtered events, newest | Implicit in graph build | Stores replaceable per NIP-01 | Newest valid kind-3 wins, enforced at write |
| Storage | SQLite (events, FTS5) + Redis (graph) | Neo4j-style graph (scoring) | LMDB (relay store) | Shared DB tuned for hundreds of M edges (see STACK) |
| Refresh | Continuous 24/7, random-walk updates | Recompute on demand | Negentropy diff sync | Staleness-driven; uniform TTL → adaptive |
| Bulk sync | REQ-based | REQ-based | Negentropy set reconciliation | REQ first; NIP-77 fast path later |
| Scoring | Monte Carlo PageRank (in-crawler) | GrapeRank/influence | none | Out of scope — graph only |
| Observability | Limited (per docs) | Web UI / dashboards | Relay metrics | First-class: coverage, staleness dist, relay health |
| Interface | NIP-90 DVM API | Web services | Relay protocol | Shared DB only — no API |

The key insight: Vertex is the closest architectural analog but bundles scoring (PageRank) into the crawler and exposes a DVM API. Our project deliberately strips both — a pure, honest, observable graph+freshness layer behind a shared-DB boundary. That narrower scope is the differentiation: do the *data* job better (adaptive refresh, outbox routing, relay scoring, rich observability) precisely because we're not doing the scoring job.

## Sources

- [Vertex `crawler_v2` (closest analog)](https://github.com/vertex-lab/crawler_v2) — HIGH (primary source, curated repo)
- [Vertex FAQ / docs](https://vertexlab.io/docs/faq/) — HIGH
- [Brainstorm WoT](https://github.com/wds4/brainstorm) / [brainstorm.world](https://brainstorm.world/) — MEDIUM (scoring-focused, crawler internals not documented on landing page)
- [NIP-02 Follow List (kind 3, replaceable)](https://github.com/nostr-protocol/nips/blob/master/02.md) / [Nostrbook kind 3](https://nostrbook.dev/kinds/3) — HIGH
- [NIP-65 Relay List Metadata / outbox model](https://nips.nostr.com/65) / [Nostrify outbox model](https://nostrify.dev/relay/outbox) — HIGH
- [NIP-77 Negentropy syncing](https://nips.nostr.com/77) / [strfry negentropy docs](https://github.com/hoytech/strfry/blob/master/docs/negentropy.md) — HIGH
- [strfry relay (negentropy, ~23% relay share)](https://github.com/hoytech/strfry) — HIGH
- Reconnection/backoff patterns from nostr relay-pool libraries (rust-nostr, nostro2-relay) — MEDIUM

**Gaps:** No public data on real-world kind-3 *churn frequency* (how often follow lists actually change) — directly relevant to the adaptive-refresh decision and must be learned by instrumentation, not assumed. Brainstorm crawler internals (refresh cadence, storage) were not retrievable from public landing pages.

---
*Feature research for: nostr web-of-trust graph crawler / data layer*
*Researched: 2026-06-11*
