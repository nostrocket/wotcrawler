# Architecture Research

**Domain:** Nostr social-graph crawler + shared data layer (Rust daemon, BFS discovery over kind-3 follow lists)
**Researched:** 2026-06-11
**Confidence:** MEDIUM-HIGH (component shape and data-flow are well-grounded in prior art and the rust-nostr SDK; specific scale numbers and staleness policy are MEDIUM and flagged for phase-level research)

## Standard Architecture

The dominant pattern for "crawl a P2P/relay-federated social graph from a seed" splits into **two clean halves connected by a durable queue/store**: an *acquisition* half (relay I/O, validation, dedup) and a *graph* half (edge persistence, frontier, staleness). Prior art (vertex-lab `crawler_v2`, Go) literally splits these into separate binaries — a 24/7 crawler process and a sync/graph builder — sharing a database. For a single Rust daemon you keep them as cooperating async tasks inside one process, but the *boundary* between "what came off the wire" and "what is the canonical graph" stays sharp. That boundary is the most important architectural decision in the whole system: never let websocket plumbing reach into graph logic, and never let graph logic block on relay latency.

The acquisition half is fundamentally a **producer**: a scheduler pulls pubkeys off a frontier, asks a relay-connection pool for "newest kind 3 for these authors," and emits raw events. A validator/deduplicator turns the noisy, out-of-order, multi-copy event stream into at most one *accepted* kind-3 per pubkey (the one with the highest `created_at`, signature-valid). The graph half is a **consumer**: it diffs the accepted follow list against what's stored, writes edge inserts/deletes, records freshness, and — critically — enqueues newly-discovered followees back onto the frontier. That feedback loop (followees become new frontier entries) *is* the BFS.

The staleness-refresh loop is not a second system; it is the same consumer/producer loop with a different frontier source. During initial crawl the frontier is fed by "newly discovered pubkeys." Once a pubkey has been fetched, it gets a `next_refresh_at` timestamp; a staleness scanner periodically selects pubkeys whose timestamp has passed and re-enqueues them. Initial crawl and refresh therefore *coexist by sharing one frontier and one fetch path* — they differ only in priority and in what populates the queue. This is the key to not building the daemon twice.

At the target scale (low millions of pubkeys, hundreds of millions of directed edges) the frontier **cannot live in memory** as a naive `HashSet` + `VecDeque` if you want crash/resume — and you do, because a multi-day initial crawl will be interrupted. The frontier and the "seen" set must be durable. The pragmatic answer is to make the database the source of truth for both the graph *and* the frontier (a `crawl_queue` / status column on the pubkey table), with an in-memory working set hydrated from the DB on startup. This gives crash/resume for free and keeps memory bounded.

### System Overview

```
┌───────────────────────────────────────────────────────────────────────┐
│                       ACQUISITION HALF (relay I/O)                      │
├───────────────────────────────────────────────────────────────────────┤
│  ┌──────────────┐   leases    ┌──────────────────┐  REQ    ┌─────────┐ │
│  │   Fetch       │ ──────────▶ │  Relay Connection │ ──────▶ │ Public  │ │
│  │  Scheduler    │             │      Pool         │ ◀────── │ Relays  │ │
│  │ (frontier     │ ◀────────── │ (curated + NIP-65 │ events  │ (wss)   │ │
│  │  consumer)    │   raw evts  │  fallback)        │         └─────────┘ │
│  └──────┬───────┘             └─────────┬────────┘                      │
│         │ pubkeys to fetch              │ raw kind-3 / kind-10002       │
│         │                               ▼                               │
│         │                    ┌──────────────────────┐                  │
│         │                    │  Validator /          │                  │
│         │                    │  Deduplicator         │                  │
│         │                    │  (sig check, newest-  │                  │
│         │                    │   per-pubkey wins)    │                  │
│         │                    └──────────┬───────────┘                   │
│─────────┼───────────────────────────────┼───────────────────────────── │
│         │           accepted kind-3 (one per pubkey)                    │
│         │                               ▼                               │
│                          GRAPH HALF (canonical store)                   │
├───────────────────────────────────────────────────────────────────────┤
│  ┌──────────────┐   enqueue   ┌──────────────────────┐                 │
│  │  Staleness    │  refresh    │   Graph Store Writer  │                │
│  │  Scanner      │ ──────────▶ │   - edge-list DIFF    │                │
│  │ (next_refresh │   FRONTIER  │   - insert/del edges  │                │
│  │  _at expired) │ ◀────────── │   - update freshness  │                │
│  └──────────────┘  new pubkeys│   - enqueue followees │                 │
│                                └──────────┬───────────┘                 │
│                                           ▼                             │
│                          ┌─────────────────────────────┐               │
│                          │  Shared Database (Postgres)  │ ◀── read ──── │
│                          │  pubkeys | edges | freshness │   SPAM LAYER  │
│                          │  crawl_queue | relay_health  │  (separate)   │
│                          └─────────────────────────────┘               │
├───────────────────────────────────────────────────────────────────────┤
│  Metrics/Observability  (taps every component: coverage, staleness      │
│  distribution, relay health, queue depth, fetch latency)  → Prometheus  │
└───────────────────────────────────────────────────────────────────────┘
```

### Component Responsibilities

| Component | Responsibility | Typical Implementation |
|-----------|----------------|------------------------|
| **Fetch Scheduler / Frontier** | Decide *who* to fetch next and at what concurrency; lease batches of pubkeys from the durable queue; respect per-relay rate limits | Tokio task; pulls a bounded working set from DB (`SELECT … FOR UPDATE SKIP LOCKED` or a status flip), hands authors to the pool |
| **Relay Connection Pool** | Hold N persistent websocket connections; send REQ filters (`{authors:[…], kinds:[3]}`); aggregate EVENT/EOSE across relays; reconnect with backoff; route to NIP-65 relays on miss | `nostr-relay-pool` crate (`RelayPool`, `RelayStatus`, `RelayPoolNotification`) over Tokio |
| **Validator / Deduplicator** | Verify schnorr signature + event id; enforce "newest `created_at` per pubkey wins"; drop duplicates and stale copies arriving from other relays | Per-pubkey "best seen so far" map keyed during a fetch window; signature via `nostr` crate |
| **Graph Store Writer** | Diff accepted follow list vs stored edges; apply inserts/deletes in one transaction; write freshness; enqueue *new* followees onto frontier | `sqlx`/`tokio-postgres`; set-difference in SQL or in Rust against current adjacency |
| **Staleness Scanner / Tracker** | Periodically select pubkeys whose `next_refresh_at` ≤ now and re-enqueue; compute next refresh interval | Tokio interval task; indexed range scan on `next_refresh_at` |
| **Metrics / Observability** | Expose coverage, staleness histogram, queue depth, per-relay success/latency/error, events/sec | `metrics` + `metrics-exporter-prometheus`; `/metrics` endpoint |
| **Reachability gate** | Ensure crawl effort only ever follows edges *into* the reachable component (spam islands stay unexplored) | Implicit: only enqueue pubkeys discovered as followees of already-accepted pubkeys — never enqueue arbitrary keys |

## Recommended Project Structure

```
src/
├── main.rs               # daemon bootstrap: load config, hydrate frontier, spawn tasks, wire shutdown
├── config.rs             # curated relay set, anchor pubkey, concurrency, staleness policy knobs
├── relay/                # ACQUISITION HALF
│   ├── pool.rs           # wraps nostr-relay-pool: connect, REQ batches, EOSE aggregation, backoff
│   ├── fetch.rs          # fetch one author's newest kind-3 across curated relays
│   └── outbox.rs         # NIP-65 (kind 10002) lookup + fallback fetch when curated relays miss
├── ingest/
│   ├── validate.rs       # signature + id verification
│   └── dedup.rs          # newest-per-pubkey resolution within a fetch window
├── graph/                # GRAPH HALF
│   ├── writer.rs         # transactional edge diff + freshness write + followee enqueue
│   ├── diff.rs           # edge-list set difference (added / removed p-tags)
│   └── frontier.rs       # durable queue: lease, ack, requeue, hydrate-on-start
├── staleness/
│   └── scanner.rs        # select expired pubkeys, compute next_refresh_at, re-enqueue
├── store/                # shared-DB access layer (the PUBLIC API surface)
│   ├── schema.sql        # canonical schema — versioned, this is the contract with the spam layer
│   └── repo.rs           # typed queries (sqlx)
├── metrics/
│   └── mod.rs            # Prometheus registry + exporter
└── scheduler.rs          # the scheduler task: ties frontier → pool → ingest → writer
```

### Structure Rationale

- **`relay/` vs `graph/` is the hard boundary.** Relay code may never query the graph; graph code may never await a socket. The only thing crossing is `AcceptedFollowList { pubkey, created_at, followees: Vec<PublicKey> }`. This keeps relay flakiness from stalling DB writes and lets you test graph logic without a network.
- **`store/schema.sql` is treated as the public API**, kept under version control with explicit migrations, because the spam layer reads it directly. Schema changes are a contract change, not an internal refactor.
- **`frontier.rs` is its own module, not buried in the scheduler**, because frontier persistence/resume is the highest-risk part of the system and deserves isolated tests (crash mid-batch, duplicate enqueue, lease expiry).
- **`staleness/` is separate from `relay/`** to make obvious that refresh is just another frontier source, not a parallel pipeline.

## Architectural Patterns

### Pattern 1: Durable Frontier with In-Memory Working Set

**What:** The authoritative frontier lives in the DB as a status column / queue table on the pubkey row (`pending | in_flight | done`, plus `next_refresh_at`). The scheduler leases a bounded batch into memory, processes it, acks it. On startup it re-hydrates by re-selecting `pending` and `in_flight` (the latter were interrupted).
**When to use:** Any crawl whose total node count exceeds comfortable RAM-for-state and whose runtime exceeds "I can afford to restart from scratch." At millions of pubkeys over a multi-day crawl, both are true.
**Trade-offs:** + Free crash/resume; bounded memory; observable queue depth via SQL. − Every lease/ack is a DB write, so batch them (lease 1–10k at a time, use `SKIP LOCKED`). Don't do one row per query.

**Example:**
```rust
// Lease a batch atomically; SKIP LOCKED lets multiple workers coexist safely.
let batch: Vec<PubkeyRow> = sqlx::query_as(
    "UPDATE crawl_queue SET status='in_flight', leased_at=now()
     WHERE id IN (
       SELECT id FROM crawl_queue
       WHERE status='pending' ORDER BY priority, discovered_at
       LIMIT $1 FOR UPDATE SKIP LOCKED)
     RETURNING id, pubkey")
    .bind(batch_size).fetch_all(&pool).await?;
```

### Pattern 2: Newest-Per-Pubkey Reconciliation (replaceable-event correctness)

**What:** Kind 3 is replaceable: relays each hold *some* version, and you may receive several. Maintain a per-pubkey "best so far" keyed on `created_at` (tie-break on lexicographically lowest event id per NIP-01) during the fetch window, and only the winner reaches the writer. The writer *also* guards with `WHERE incoming.created_at > stored.created_at` so out-of-order arrivals across separate fetches can't regress the graph.
**When to use:** Always, for replaceable events. This is non-negotiable correctness, not optimization.
**Trade-offs:** + Correct under duplicates, reordering, conflicting relay copies. − Requires storing `created_at` per pubkey as part of freshness; cheap.

**Example:**
```rust
// Within one author's fetch window, keep only the newest valid event.
if event.verify().is_ok()
   && best.get(&event.pubkey).map_or(true, |b| event.created_at > b.created_at
        || (event.created_at == b.created_at && event.id < b.id)) {
    best.insert(event.pubkey, event);
}
```

### Pattern 3: Edge-List Diffing on Replacement

**What:** When a newer kind-3 replaces an older one, do **not** delete-all-then-insert-all (it churns indexes, breaks consumers reading mid-update, and inflates write volume). Compute the set difference between the new followee set and the stored adjacency: `added = new \ old`, `removed = old \ new`. Apply only those, in one transaction. Only members of `added` that are not already known pubkeys get enqueued to the frontier.
**When to use:** Every replacement of an existing pubkey's list. (First-ever fetch is all-insert.)
**Trade-offs:** + Minimal write amplification; consumers see a consistent transition; new-pubkey discovery falls out naturally. − Needs the current adjacency available; at hundreds of millions of edges, rely on an index on `(follower)` so loading one node's out-edges is cheap.

**Example:**
```sql
-- Inside one transaction, after computing added/removed in Rust (or via a temp table):
DELETE FROM edges WHERE follower=$1 AND followee = ANY($removed);
INSERT INTO edges (follower, followee) SELECT $1, x FROM unnest($added) AS x
  ON CONFLICT DO NOTHING;
UPDATE pubkeys SET last_kind3_at=$2, fetched_at=now(),
  next_refresh_at = now() + $interval WHERE pubkey=$1;
```

### Pattern 4: Unified Frontier, Two Sources (initial crawl ⊕ refresh coexist)

**What:** One queue, fed by two producers. The **discovery producer** is the writer enqueuing newly-seen followees (drives the initial BFS to completion). The **refresh producer** is the staleness scanner enqueuing expired pubkeys. The scheduler consumes both, with discovery typically higher priority until coverage plateaus, then refresh dominates steady state.
**When to use:** This is the core daemon loop — it is how the system transitions from "initial full crawl" to "continuous staleness-driven refresh" without a mode switch or a second pipeline.
**Trade-offs:** + One code path to test and observe; smooth handoff. − Need a priority/fairness rule so refresh of hot nodes doesn't starve initial coverage (and vice versa). A simple `priority` column + ratio scheduler suffices.

## Data Flow

### Acquisition → Persistence Flow

```
[anchor pubkey seeded into crawl_queue]
        ↓
[Scheduler leases batch] → [Relay Pool sends REQ {authors, kinds:[3]}]
        ↓                          ↓ (curated relays first)
        ↓                   [miss?] → [Outbox: fetch kind 10002, REQ author's write relays]
        ↓                          ↓
[Validator: verify sig/id] → [Dedup: newest created_at per pubkey wins]
        ↓
[AcceptedFollowList crosses the boundary]
        ↓
[Graph Writer] →(1) diff vs stored edges  →(2) apply added/removed in txn
        ↓        →(3) write freshness (last_kind3_at, fetched_at, next_refresh_at)
        ↓        →(4) enqueue NEW followees as 'pending' in crawl_queue
[ack leased batch as 'done']
        ↓
[Shared DB] ← read independently by spam layer (schema = contract)
```

### Staleness / Steady-State Flow

```
[Staleness Scanner ticks]
        ↓
SELECT pubkey FROM pubkeys WHERE next_refresh_at <= now() LIMIT batch
        ↓
[set crawl_queue.status='pending' for those pubkeys]  ← rejoins the SAME loop above
```

### Key Data Flows

1. **BFS expansion:** The graph writer enqueuing newly-discovered followees is the *only* mechanism that grows the frontier. Because the writer only ever sees followees of *accepted* (reachable) pubkeys, spam islands nobody in the reachable set points to are never enqueued — the reachability constraint is satisfied structurally, for free.
2. **Backpressure:** Relay latency must not stall DB writes and DB write latency must not stall relay reads. Connect the two halves with a bounded `tokio::mpsc` channel of `AcceptedFollowList`. A full channel naturally throttles the scheduler.
3. **Crash/resume:** `in_flight` rows on startup are re-leased; partial edge writes are atomic per pubkey (one transaction), so resume is at pubkey granularity with no torn graph state.

## Scaling Considerations

| Scale | Architecture Adjustments |
|-------|--------------------------|
| ~10k pubkeys (dev/test) | Single process, in-memory frontier fine, SQLite acceptable, one relay connection group. |
| ~100k–500k pubkeys | Durable frontier becomes worth it; Postgres; batch leases; tune relay concurrency; add metrics early. |
| Low millions / hundreds of millions of edges (target) | Postgres with a **row-per-edge** `edges(follower, followee)` table + composite PK and a `(followee)` index for reverse lookups; batch inserts via `COPY`/`unnest`; partition or at least cluster edges by follower; staleness scan must be index-driven, never a full table scan. |

### Scaling Priorities

1. **First bottleneck — relay throughput & goodwill, not the DB.** You are fetching from other people's relays; the binding constraints are connection limits, rate limits, and "fetch each list roughly once." Batch authors per REQ, cap concurrency per relay, and lean on the curated set so NIP-65 fallback is the exception. The DB at hundreds of millions of *rows* (not array-columns) is comfortably within Postgres's range — note the widely-cited Postgres ~1 GB TOAST limit concerns *array/large-value columns*, which a normalized edge table deliberately avoids, so it does **not** apply here.
2. **Second bottleneck — write amplification on replacement.** Without edge-diffing (Pattern 3), every refresh of a 1000-follow account rewrites 1000 rows. With diffing, churn drops to actual deltas. This is why diffing is in v1, not later.
3. **Third — staleness scan cost.** A naive `WHERE next_refresh_at <= now()` over millions of rows needs a btree index on `next_refresh_at` and a `LIMIT`; otherwise the scanner degrades as the graph grows.

## Anti-Patterns

### Anti-Pattern 1: In-Memory-Only Frontier

**What people do:** Keep the BFS queue and visited-set in RAM (`VecDeque` + `HashSet`).
**Why it's wrong:** A multi-day crawl of millions of pubkeys *will* be interrupted; you lose all progress and re-hammer relays from scratch, burning the goodwill the project explicitly must conserve.
**Do this instead:** Make the DB authoritative for frontier state; hydrate a bounded working set on start (Pattern 1).

### Anti-Pattern 2: Delete-All-Then-Insert-All on Replacement

**What people do:** On a newer kind-3, `DELETE FROM edges WHERE follower=$1` then re-insert the full list.
**Why it's wrong:** Massive write amplification, index churn, and the spam layer can observe an empty follow list mid-update.
**Do this instead:** Edge-list diffing in a single transaction (Pattern 3).

### Anti-Pattern 3: Trusting created_at Ordering of Arrival

**What people do:** Take the last event received as the current one.
**Why it's wrong:** Events arrive out of order and duplicated across relays; the last *received* is often not the newest *created*. You'd regress the graph to an older list.
**Do this instead:** Newest-`created_at`-wins with a guard on the write (Pattern 2).

### Anti-Pattern 4: Letting the Graph Half Block on the Network

**What people do:** Writer awaits a relay fetch inline, or the scheduler holds a DB transaction open across a websocket round-trip.
**Why it's wrong:** One slow relay stalls graph writes; long transactions block the shared DB the spam layer reads.
**Do this instead:** Decouple with a bounded channel; keep DB transactions short and network-free.

### Anti-Pattern 5: Enqueuing Arbitrary / Discovered-Anywhere Pubkeys

**What people do:** Add any pubkey seen in any event to the frontier.
**Why it's wrong:** Pulls in spam islands and unreachable keys, violating the "never crawl who nobody points to" requirement and exploding scope.
**Do this instead:** Only enqueue followees of already-accepted pubkeys (reachability is structural).

## Integration Points

### External Services

| Service | Integration Pattern | Notes |
|---------|---------------------|-------|
| Public nostr relays (curated set) | Persistent wss via `nostr-relay-pool`; batched REQ on `{authors, kinds:[3]}`; aggregate EOSE across pool | Workhorse path; cap concurrency, backoff on disconnect; track per-relay health for metrics |
| NIP-65 relays (kind 10002) | Fallback only — fetch author's relay list, then REQ their write relays | Use sparingly (on curated-set miss); cache discovered relay lists to avoid re-lookup |

### Internal Boundaries

| Boundary | Communication | Notes |
|----------|---------------|-------|
| Relay pool ↔ Validator | in-process stream of raw events | one-directional, lossy-tolerant (duplicates expected) |
| Validator/Dedup ↔ Graph Writer | bounded `mpsc<AcceptedFollowList>` | the hard acquisition/graph boundary; provides backpressure |
| Graph Writer ↔ Frontier | enqueue new followees (DB write) | feedback edge that drives BFS |
| Staleness Scanner ↔ Frontier | enqueue expired pubkeys (DB write) | second frontier source; same queue |
| Daemon ↔ Spam layer | **shared DB schema (read-only for consumer)** | the project's public API; version migrations explicitly |

## Suggested Build Order

Dependencies flow acquisition → ingest → graph → loops → observability. Build the data contract early because it gates everything downstream and is the public API.

1. **Schema + store layer** (`store/`) — define `pubkeys`, `edges`, freshness columns, `crawl_queue`; this is the contract and unblocks every other component. Get the edge table and indexes right first.
2. **Relay pool + single-author fetch** (`relay/pool.rs`, `relay/fetch.rs`) — connect to the curated set, REQ one author's kind-3, handle EOSE/backoff. Verifiable in isolation against real relays.
3. **Validator + dedup** (`ingest/`) — signature check, newest-per-pubkey. Pure logic, unit-testable with fixtures.
4. **Graph writer + edge diff** (`graph/writer.rs`, `graph/diff.rs`) — diff and persist; testable against a local DB with no network. At this point a *single manual fetch → store* works end to end.
5. **Durable frontier + scheduler** (`graph/frontier.rs`, `scheduler.rs`) — lease/ack/resume + the followee-enqueue feedback loop. This turns step 4 into an actual BFS crawl from the anchor.
6. **Staleness scanner** (`staleness/`) — adds the second frontier source; converts the finite crawl into a continuous daemon. (Depends on freshness columns from step 1 and the loop from step 5.)
7. **NIP-65 outbox fallback** (`relay/outbox.rs`) — recover pubkeys the curated set misses. Deliberately late: it's an optimization on coverage, not a prerequisite for a working crawl.
8. **Observability** (`metrics/`) — coverage, staleness histogram, queue depth, per-relay health. Taps all prior components; a v1 requirement, so not skippable, but built once there are real signals to expose.

**Ordering rationale:** Steps 1–4 produce a vertical slice (fetch one list, store it correctly) that de-risks the two hardest correctness problems (replaceable-event dedup, edge diffing) before any concurrency. Steps 5–6 add the loops that make it a daemon. Steps 7–8 harden coverage and trust. The frontier (step 5) is the single highest-risk component and should get the most test attention (crash mid-batch, duplicate enqueue, resume).

## Sources

- [NIP-02 Follow List (kind 3)](https://github.com/nostr-protocol/nips/blob/master/02.md) — HIGH (canonical spec)
- [NIP-65 / Outbox model](https://nostrify.dev/relay/outbox) and [whynostr: Outbox Model](https://www.whynostr.org/post/8yjqxm4sky-tauwjoflxs/) — HIGH/MEDIUM (curated docs)
- [vertex-lab crawler_v2](https://github.com/vertex-lab/crawler_v2) — MEDIUM (direct prior art: 24/7 crawler + sync/graph split, dynamic relay discovery via kind:10002, separate event store and graph store)
- [optout21/nostr-crawler0](https://github.com/optout21/nostr-crawler0) — LOW/MEDIUM (simpler prior art for relay-walking)
- [nostr-relay-pool crate docs](https://docs.rs/nostr-relay-pool) — HIGH (RelayPool, RelayStatus, reconnection, runtime add/remove, EOSE aggregation)
- [Kind reference, rust-nostr book](https://rust-nostr.org/sdk/event/kind.html) — HIGH (SDK confirms kind-3/EventBuilder support)
- [Postgres as graph / edge storage at scale (HN discussion)](https://brianlovin.com/hn/35386948), [OneSparse: edges in Postgres](https://onesparse.com/blog/2025/07/01/billions-of-edges-per-second-with-postgres.html) — MEDIUM (TOAST ~1 GB limit applies to array/large columns, not normalized edge rows; row-per-edge avoids it)

---
*Architecture research for: nostr social-graph crawler + shared data layer (Rust)*
*Researched: 2026-06-11*
