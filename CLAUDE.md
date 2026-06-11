<!-- GSD:project-start source:PROJECT.md -->

## Project

**Nostr Web-of-Trust Crawler & Data Layer**

A continuously running crawler and data layer that turns nostr's scattered, adversarial kind-3 follow data into a locally held, always-current picture of the social graph. Starting from a single trusted anchor pubkey, it discovers everyone reachable through follows, fetches each follow list from public relays roughly once, remembers how current that knowledge is, and re-checks relays only when the knowledge ages out. It is the foundation for a separate spam-scoring project that propagates trust over this graph.

**Core Value:** From one anchor pubkey, maintain a complete and continuously fresh follow graph of everyone reachable through follows — fetched from the wild efficiently (each list roughly once, refreshed only when stale) — so a downstream trust/spam layer can read it from a shared database at any time.

### Constraints

- **Tech stack**: Rust — performance and memory control for a large graph and thousands of concurrent relay connections
- **Architecture**: Shared database as the project boundary — the spam layer consumes the graph by reading the DB, not via an API or library
- **Scale**: Must handle low millions of pubkeys / hundreds of millions of edges — full reachable component, not hop-limited
- **Efficiency**: Each follow list fetched from relays roughly once; re-fetches only on staleness — relay goodwill and bandwidth are finite
- **Operations**: Single operator, self-hosted — favor simplicity over deployment polish

<!-- GSD:project-end -->

<!-- GSD:stack-start source:research/STACK.md -->

## Technology Stack

## Executive Recommendation

## Recommended Stack

### Core Technologies

| Technology | Version | Purpose | Why Recommended |
|------------|---------|---------|-----------------|
| **Rust** | 1.84+ (2021/2024 edition) | Implementation language | Decided. Memory control + zero-cost async for thousands of concurrent relay sockets at millions-of-nodes scale. |
| **nostr-sdk** | 0.44.1 | Nostr protocol: events, signatures (secp256k1), relay pool, websocket lifecycle, gossip/outbox routing | The canonical, actively-maintained (last release 2026-05-19, ~765k downloads) rust-nostr umbrella crate. Bundles `nostr`, `nostr-relay-pool`, `nostr-database`, gossip support. Handles kind-3 parsing, replaceable-event semantics, and NIP-65 natively. Reimplementing the websocket/event/sig layer by hand is the classic wasted-quarter mistake. |
| **tokio** | 1.52 | Async runtime | Required by nostr-sdk; the de-facto Rust async runtime. Multi-threaded scheduler handles thousands of concurrent relay connections with bounded worker threads. |
| **PostgreSQL** | 16 or 17 | Shared graph store (pubkeys, directed follow edges, freshness metadata) | FIRM recommendation. MVCC gives lock-free concurrent reads by the separate spam-layer process while the crawler writes; `COPY` gives the bulk-insert throughput needed for hundreds of millions of edges; the schema *is* the cross-project API and SQL is the most stable, tool-rich interface for that contract. See database section. |
| **sqlx** | 0.9.0 | Async Postgres driver + query layer | Pure-Rust, async, compile-time-checked queries against a live/offline schema (`sqlx::query!`), built-in connection pool. No ORM/DSL overhead — you write SQL, which matters because the schema is a public contract you want to read literally. Released 2026-05-21; massive adoption. |

### Supporting Libraries

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| **nostr-relay-pool** | 0.44.1 | Lower-level relay pool (managed for you by nostr-sdk) | Reach for it directly only if you need finer control over per-relay connection policy than `Client` exposes. Default: let `nostr-sdk` manage it. |
| **nostr-gossip** / **nostr-gossip-sqlite** | 0.44 | NIP-65 outbox routing tables; maps pubkeys → their advertised relays | Backs the "NIP-65 fallback" half of the hybrid relay strategy. Enable via `ClientOptions::gossip(true)` so `fetch_events` auto-routes to a pubkey's write relays when the curated set misses. |
| **tracing** | 0.1.44 | Structured, async-aware logging + spans | Primary instrumentation. Spans per crawl batch / per relay connection give you the crawl-coverage and relay-health visibility the daemon needs. |
| **tracing-subscriber** | 0.3.23 | tracing output formatting / filtering / env-filter | Always paired with `tracing`. JSON output for log shipping, `EnvFilter` for runtime log-level control. |
| **metrics** | 0.24.6 | Lightweight metrics facade (counters/gauges/histograms) | Record crawl-coverage counters, staleness-distribution histograms, relay-success/failure rates, queue depth. Facade pattern lets you swap exporters. |
| **metrics-exporter-prometheus** | 0.18.3 | Exposes `metrics` over an HTTP `/metrics` endpoint | The observability requirement ("operator can see crawl coverage, staleness distribution, relay health unattended") → scrape with Prometheus + Grafana. |
| **governor** | 0.10.4 | Rate limiting (GCRA / token bucket) | Enforce "each list fetched roughly once" politeness and per-relay request caps so you don't burn relay goodwill or get IP-banned. |
| **petgraph** | 0.8.3 | In-memory graph data structures / algorithms | Optional, for the in-memory BFS frontier / reachability bookkeeping if you want a structured graph rather than hand-rolled `HashSet`/`VecDeque`. Evaluate vs. a plain visited-set + queue; for pure BFS the hand-rolled version is often leaner at these scales. |
| **serde** | 1.0.228 | (De)serialization | Transitive via nostr-sdk; used directly for config. |
| **config** | 0.15.23 | Layered config file loading | Single-operator daemon config (anchor pubkey, curated relay set, staleness TTLs) per the "config file + README is enough" constraint. |
| **clap** | 4.6.1 | CLI argument parsing | Daemon flags, one-shot maintenance subcommands (re-crawl, stats dump). |
| **anyhow** | 1.0.102 | Application-level error handling | Crawler binary error plumbing. |
| **thiserror** | 2.0.18 | Typed library errors | For any internal crate boundaries / the store module. |

### Development Tools

| Tool | Purpose | Notes |
|------|---------|-------|
| **sqlx-cli** | Migrations + offline query metadata | `sqlx migrate` manages schema versions (critical: the schema is the spam-layer's API — version it deliberately). `cargo sqlx prepare` generates `.sqlx/` so compile-time query checks work in CI without a live DB. |
| **cargo-nextest** | Faster test runner | Optional but standard in 2026 Rust projects. |
| **Prometheus + Grafana** | Metrics scraping + dashboards | Self-hosted, pairs with `metrics-exporter-prometheus`. Single dashboard: coverage, staleness histogram, relay health. |

## Installation

# Cargo.toml (key dependencies)

# Optional, evaluate during build:

# petgraph = "0.8"

# cargo install sqlx-cli   (not a Cargo dep)

# Tooling

# Database (self-hosted)

# Install PostgreSQL 17, then:

## Database Decision (FIRM): PostgreSQL

### Why PostgreSQL wins for *this* workload

### Scale sanity check

| Concern | At low-millions pubkeys / 100s-of-millions edges |
|---------|--------------------------------------------------|
| `follows` table size | ~hundreds of millions of rows × (2 bigint FKs + edge metadata) ≈ low tens of GB. Comfortable for a single Postgres instance with adequate RAM/SSD. |
| Edge representation | Store pubkeys once in `pubkeys` table (32-byte key → `bytea` or surrogate `bigint id`); `follows(follower_id bigint, followee_id bigint)` keeps the big table to two bigints + metadata. Surrogate bigint IDs shrink the index and join cost dramatically vs. storing 32-byte keys in the edge table. |
| Hot indexes | `follows(follower_id)` for "who does X follow", `follows(followee_id)` for in-degree / reachability checks, partial index on staleness. |
| Bulk load | Initial crawl: `COPY` + deferred index build. Continuous refresh: batched upserts (`INSERT ... ON CONFLICT` or delete+COPY per refreshed pubkey since kind-3 is replaceable). |

### Alternatives weighed (and why not)

| Candidate | Strength | Why NOT the primary choice here |
|-----------|----------|----------------------------------|
| **SQLite (rusqlite/sqlx-sqlite)** | Zero-ops embedded, great for single-process; WAL mode allows concurrent readers | Global *write* lock and single-writer model fight the continuous high-rate crawl writes; cross-process concurrent access while writing is fragile (lock contention, no MVCC for writers). Best for read-mostly single-process; this is write-heavy + multi-process. Keep as a fallback only if "single binary, no DB server" becomes a hard constraint. |
| **RocksDB** | Best raw write throughput (LSM); embeddable | Single-process embedded by design; no built-in multi-process concurrent access, no SQL, no ad-hoc query for the spam layer. Forces a hand-rolled binary schema as the cross-project contract — exactly the brittle coupling to avoid. Write throughput isn't the binding constraint; the cross-process read boundary is. |
| **LMDB (heed) / redb** | Extremely fast memory-mapped reads; redb is pure-Rust | Single-writer MVCC, embedded, no SQL, no rich cross-language client story for the spam layer. Same coupling problem as RocksDB. redb 4.1 is solid but solves a different problem. |
| **Embedded graph DBs (IndraDB, etc.) / Neo4j** | Native graph traversal | The crawler doesn't traverse in-DB (it BFSes in memory), and the spam walk is short and shallow — a dedicated graph engine is operational overhead and another moving part for no payoff at this scale. Postgres adjacency lists cover it. |
| **SurrealDB / newer multi-model** | Trendy, graph + doc + SQL-ish | Less mature operationally for a "trust it unattended" single-operator daemon; smaller battle-tested track record at 100M+ edges. Stability > novelty here. |

## Alternatives Considered

| Recommended | Alternative | When to Use Alternative |
|-------------|-------------|-------------------------|
| sqlx 0.9 (Postgres) | tokio-postgres + deadpool-postgres | If you need query *pipelining* (sqlx lacks it; tokio-postgres has it) for extreme insert throughput. Start with sqlx; drop to tokio-postgres only if profiling shows the driver is the bottleneck. |
| sqlx 0.9 | diesel 2.3 / diesel-async | If you want a typed query DSL + migrations as Rust. We prefer raw-SQL-as-contract since the schema is a public API; diesel's DSL adds indirection. |
| sqlx 0.9 | sea-orm 1.1 | If you want a full async ORM with entities. Overkill — this is a few tables, not a domain model; ORM hides the schema you're trying to expose cleanly. |
| nostr-sdk Client (gossip on) | nostr-relay-pool directly | If `Client` abstractions get in the way of bespoke per-relay scheduling/backpressure at thousands of connections. Profile first; default to `Client`. |
| metrics + Prometheus exporter | OpenTelemetry (opentelemetry 0.32) | If you later want traces+metrics+logs unified into an OTLP pipeline. For a single self-hosted daemon, Prometheus scrape is simpler and sufficient. |
| PostgreSQL | SQLite (WAL) | Only if "no DB server, single static binary" becomes a hard deployment constraint AND the spam layer can tolerate read-only-while-paused access. |

## What NOT to Use

| Avoid | Why | Use Instead |
|-------|-----|-------------|
| Hand-rolled websocket + secp256k1 + event parsing | Reimplements the hard, security-sensitive parts of nostr (sig verification, replaceable-event rules, NIP-65) — weeks of work and a bug surface | nostr-sdk 0.44 |
| RocksDB / LMDB / redb as the *shared* store | Embedded, single-process, no SQL, no cross-language client — breaks the "spam layer reads the DB directly" boundary | PostgreSQL |
| SQLite as the primary store for a continuous high-write multi-process crawl | Global write lock + weak multi-process write story under continuous crawl writes | PostgreSQL (SQLite only as a constrained fallback) |
| Storing 32-byte pubkeys directly in the 100M-row edge table | Bloats the largest table + every index; slows joins | Surrogate `bigint` ids in a `pubkeys` table; edges reference ids |
| Recursive CTE BFS in Postgres for the crawl frontier | Postgres recursive executor can't hold visited-state efficiently; seconds-to-minutes on large graphs | In-memory BFS frontier in Rust; DB only for indexed adjacency lookups |
| `log` crate for the daemon | Not span/async-aware; weak structured output for an observability-critical daemon | tracing + tracing-subscriber |
| lmdb 0.8 (the old `lmdb` crate, last updated 2018) | Unmaintained | heed 0.22 *if* you ever needed LMDB (you don't here) |
| Diesel/SeaORM ORM layer | Hides the schema that is your public API; adds DSL indirection | sqlx raw SQL + sqlx-cli migrations |

## Stack Patterns by Variant

- Drive fetches through `nostr-sdk` `Client` against the curated relay list as the workhorse.
- Enable `ClientOptions::gossip(true)` so misses auto-fall-back to a pubkey's NIP-65 write relays.
- Rate-limit per relay with `governor`.
- Move from `Client` to direct `nostr-relay-pool` management for explicit per-relay connection budgets and backpressure.
- Cap concurrent in-flight subscriptions; batch pubkey requests per relay.
- Fall back to SQLite via `sqlx` (sqlite feature) in WAL mode, accept reduced write concurrency, and have the spam layer open read-only connections. Document the concurrency caveat as part of the schema contract.
- Use Postgres `COPY` binary ingest for edges, defer index creation until after the initial full crawl, then `CREATE INDEX CONCURRENTLY`.
- If still bound by the driver, switch the ingest path to tokio-postgres for pipelining.

## Version Compatibility

| Package | Compatible With | Notes |
|---------|-----------------|-------|
| nostr-sdk 0.44.1 | nostr 0.44.3, nostr-relay-pool 0.44.1, nostr-database 0.44.0, nostr-gossip 0.44 | rust-nostr releases the family in lockstep on the same minor (0.44.x, 2026-05-19). Pin to `0.44` and upgrade the family together; the project breaks API across minors. |
| sqlx 0.9.0 | tokio 1.52, PostgreSQL 12–17, rustls TLS | Use `runtime-tokio` + `tls-rustls` features. Note: mixing `sqlx` (sqlite feature) and `rusqlite` in one build can conflict on `libsqlite3-sys` versions — avoid combining; with Postgres this is a non-issue. |
| metrics 0.24.6 | metrics-exporter-prometheus 0.18.3 | Exporter tracks the `metrics` facade version; upgrade together. |
| tokio 1.52 | nostr-sdk 0.44, sqlx 0.9 | All target the stable tokio 1.x line. |
| governor 0.10.4 | tokio 1.x | Async-compatible rate limiting. |

## Sources

- crates.io API (https://crates.io/api/v1/crates/...) — verified current stable versions on 2026-06-11: nostr-sdk 0.44.1, nostr 0.44.3, nostr-relay-pool 0.44.1, nostr-database 0.44.0, nostr-gossip 0.44.0, sqlx 0.9.0, rusqlite 0.40.1, tokio-postgres 0.7.17, tokio 1.52.3, tracing 0.1.44, metrics 0.24.6, metrics-exporter-prometheus 0.18.3, rocksdb 0.24.0, redb 4.1.0, heed 0.22.1, governor 0.10.4, petgraph 0.8.3, config 0.15.23, clap 4.6.1, thiserror 2.0.18 — **HIGH confidence**
- rust-nostr GitHub + rust-nostr.org NIP-65 docs — gossip/outbox routing (`ClientOptions::gossip`), nostr-gossip / nostr-gossip-sqlite crates, NIP-65 kind:10002 read/write relay semantics — **HIGH confidence**
- lib.rs / SQLx README + GitHub — sqlx async pure-Rust Postgres driver, compile-time checked queries, no pipelining vs tokio-postgres pipelining; libsqlite3-sys conflict note — **HIGH confidence**
- explainextended.com, cybertec-postgresql.com, dev.to (Postgres-as-graph-engine), eulerai.au DB benchmark, StackShare Postgres-vs-RocksDB — adjacency-list modeling, recursive-CTE traversal weakness, MVCC concurrency vs RocksDB single-node/LSM, SQLite write-lock limits — **MEDIUM-HIGH confidence** (general/blog sources, cross-checked across multiple)

<!-- GSD:stack-end -->

<!-- GSD:conventions-start source:CONVENTIONS.md -->

## Conventions

Conventions not yet established. Will populate as patterns emerge during development.
<!-- GSD:conventions-end -->

<!-- GSD:architecture-start source:ARCHITECTURE.md -->

## Architecture

Architecture not yet mapped. Follow existing patterns found in the codebase.
<!-- GSD:architecture-end -->

<!-- GSD:skills-start source:skills/ -->

## Project Skills

No project skills found. Add skills to any of: `.claude/skills/`, `.agents/skills/`, `.cursor/skills/`, `.github/skills/`, or `.codex/skills/` with a `SKILL.md` index file.
<!-- GSD:skills-end -->

<!-- GSD:workflow-start source:GSD defaults -->

## GSD Workflow Enforcement

Before using Edit, Write, or other file-changing tools, start work through a GSD command so planning artifacts and execution context stay in sync.

Use these entry points:

- `/gsd-quick` for small fixes, doc updates, and ad-hoc tasks
- `/gsd-debug` for investigation and bug fixing
- `/gsd-execute-phase` for planned phase work

Do not make direct repo edits outside a GSD workflow unless the user explicitly asks to bypass it.
<!-- GSD:workflow-end -->

<!-- GSD:profile-start -->

## Developer Profile

> Profile not yet configured. Run `/gsd-profile-user` to generate your developer profile.
> This section is managed by `generate-claude-profile` -- do not edit manually.
<!-- GSD:profile-end -->
