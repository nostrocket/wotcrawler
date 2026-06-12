# Requirements: Nostr Web-of-Trust Crawler & Data Layer

**Defined:** 2026-06-11
**Core Value:** From one anchor pubkey, maintain a complete and continuously fresh follow graph of everyone reachable through follows — fetched efficiently — so a downstream trust/spam layer can read it from a shared database at any time.

## v1 Requirements

Requirements for initial release. Each maps to roadmap phases.

### Ingest & Validation

- [ ] **INGEST-01**: Every event's signature is verified against its pubkey before anything is stored; invalid events are discarded and counted
- [ ] **INGEST-02**: Duplicate events (same id arriving from multiple relays) are processed at most once
- [ ] **INGEST-03**: For each pubkey, only the newest valid kind-3 event is applied — future-dated `created_at` is rejected (configurable clamp, e.g. >1h ahead), same-timestamp ties break to lowest event id
- [ ] **INGEST-04**: Malformed p-tags are skipped and oversized follow lists are bounded by a configurable cap without crashing the pipeline
- [ ] **INGEST-05**: kind:10002 (NIP-65) relay-list events are ingested and validated under the same replaceable-event rules

### Relay Acquisition

- [ ] **RELAY-01**: Crawler maintains connections to a configurable curated relay set with automatic reconnect and exponential backoff with jitter
- [ ] **RELAY-02**: Crawler reads each relay's NIP-11 document and respects advertised limits (max_subscriptions, max_limit, etc.)
- [ ] **RELAY-03**: Fetches paginate (`until` windows, author chunking) and never treat EOSE as proof of completeness
- [ ] **RELAY-04**: Per-relay rate limiting keeps request rates polite; rate-limited notices trigger backoff
- [ ] **RELAY-05**: When a pubkey's kind 3 isn't found on curated relays, the crawler falls back to that pubkey's NIP-65 write relays
- [ ] **RELAY-06**: Each relay carries a health score derived from observed behavior (connect failures, timeouts, rate-limit hits, response latency) that drives routing and per-relay concurrency

### Graph Storage

- [x] **GRAPH-01**: PostgreSQL schema stores pubkeys (surrogate bigint ids), directed follow edges, and per-pubkey freshness metadata, with versioned migrations
- [ ] **GRAPH-02**: A replacing kind-3 is applied as a transactional edge diff (insert added, delete removed); an unchanged list (same event id) touches zero edge rows
- [x] **GRAPH-03**: A separate process (the spam layer) can read the graph concurrently while the crawler writes, without coordination
- [x] **GRAPH-04**: The schema is documented as the public contract for downstream consumers

### Crawl & Frontier

- [ ] **CRAWL-01**: Crawl starts from a single configurable anchor pubkey and discovers pubkeys via BFS over follow edges
- [ ] **CRAWL-02**: Only pubkeys followed by someone already in the graph are ever enqueued — spam islands nobody legitimate points to are never crawled
- [ ] **CRAWL-03**: The frontier is DB-resident; after crash or restart the crawler resumes without refetching completed work
- [ ] **CRAWL-04**: In-flight fetch concurrency is bounded end-to-end (backpressure; no unbounded queues or memory growth)

### Refresh

- [ ] **FRESH-01**: Every pubkey records when its follow-list knowledge was last acquired or confirmed
- [ ] **FRESH-02**: A staleness scanner enqueues pubkeys whose knowledge exceeds a configurable uniform TTL into the same frontier the initial crawl uses
- [ ] **FRESH-03**: Each refresh records whether the follow list actually changed, accumulating per-pubkey churn data to ground a future adaptive policy

### Observability

- [ ] **OBS-01**: Prometheus `/metrics` endpoint exposes crawl coverage, staleness distribution, relay health, frontier depth, fetch rate, and validation-failure counts
- [ ] **OBS-02**: Structured logging via `tracing` with configurable levels
- [ ] **OBS-03**: HTTP health endpoint (liveness/readiness) for process supervisors
- [ ] **OBS-04**: Periodic crawl-progress summaries (frontier size, fetch rate, coverage %) are logged during the initial multi-day crawl
- [ ] **OBS-05**: A Grafana dashboard JSON covering the exported metrics is committed to the repo

### Operations

- [ ] **OPS-01**: Single Rust daemon binary configured via config file (anchor pubkey, relay set, TTL, DB URL, concurrency caps)
- [ ] **OPS-02**: Graceful shutdown drains in-flight work and leaves DB state consistent

## v2 Requirements

Deferred to future release. Tracked but not in current roadmap.

### Refresh

- **FRESH-04**: Adaptive per-pubkey refresh intervals derived from observed churn (requires weeks of FRESH-03 data)

### Relay Acquisition

- **RELAY-07**: NIP-77 negentropy bulk sync with supporting relays (~16% relay support today)
- **RELAY-08**: Streaming live kind-3 subscriptions for near-real-time graph updates

## Out of Scope

Explicitly excluded. Documented to prevent scope creep.

| Feature | Reason |
|---------|--------|
| Trust propagation / spam scoring | Separate project; this layer only builds and maintains the graph it consumes |
| Pubkey → spam-verdict lookup | Product of the spam layer, not the data layer |
| Content/note fetching or analysis | System works entirely from social structure (kind 3, kind 10002) |
| API service (HTTP/gRPC) | The shared DB schema is the consumer contract |
| Multi-anchor support | Single trusted anchor is the model; revisit only if the spam layer demands it |
| Deployment polish (Docker, install docs) | Single-operator infrastructure; config file + README suffices |

## Traceability

Which phases cover which requirements. Updated during roadmap creation.

| Requirement | Phase | Status |
|-------------|-------|--------|
| INGEST-01 | Phase 2 | Pending |
| INGEST-02 | Phase 2 | Pending |
| INGEST-03 | Phase 2 | Pending |
| INGEST-04 | Phase 2 | Pending |
| INGEST-05 | Phase 2 | Pending |
| RELAY-01 | Phase 2 | Pending |
| RELAY-02 | Phase 2 | Pending |
| RELAY-03 | Phase 2 | Pending |
| RELAY-04 | Phase 2 | Pending |
| RELAY-05 | Phase 5 | Pending |
| RELAY-06 | Phase 5 | Pending |
| GRAPH-01 | Phase 1 | Complete |
| GRAPH-02 | Phase 3 | Pending |
| GRAPH-03 | Phase 1 | Complete |
| GRAPH-04 | Phase 1 | Complete |
| CRAWL-01 | Phase 3 | Pending |
| CRAWL-02 | Phase 3 | Pending |
| CRAWL-03 | Phase 3 | Pending |
| CRAWL-04 | Phase 3 | Pending |
| FRESH-01 | Phase 3 | Pending |
| FRESH-02 | Phase 4 | Pending |
| FRESH-03 | Phase 4 | Pending |
| OBS-01 | Phase 4 | Pending |
| OBS-02 | Phase 4 | Pending |
| OBS-03 | Phase 4 | Pending |
| OBS-04 | Phase 4 | Pending |
| OBS-05 | Phase 4 | Pending |
| OPS-01 | Phase 4 | Pending |
| OPS-02 | Phase 4 | Pending |

**Coverage:**

- v1 requirements: 29 total
- Mapped to phases: 29
- Unmapped: 0 ✓

---
*Requirements defined: 2026-06-11*
*Last updated: 2026-06-11 after roadmap creation*
