# Phase 4: Daemon, Staleness Loop & Observability - Context

**Gathered:** 2026-06-15
**Status:** Ready for planning

<domain>
## Phase Boundary

Turn the library-only crawler (Phases 1–3: schema, relay acquisition, graph writer + DB-resident BFS frontier) into a single configurable, long-running daemon binary. The daemon runs the initial full crawl, then transitions to a continuous TTL-driven staleness refresh over the same frontier, shuts down gracefully on signal, and exposes the metrics, logs, and health signals an operator needs to trust it running unattended for days.

Requirements in scope: FRESH-02, FRESH-03, OBS-01, OBS-02, OBS-03, OBS-04, OBS-05, OPS-01, OPS-02.

Out of scope (later phases / v2): NIP-65 outbox routing & relay-health-driven routing (Phase 5); adaptive per-pubkey refresh intervals derived from churn (FRESH-04, v2 — Phase 4 only *accumulates* the churn data that grounds it).
</domain>

<decisions>
## Implementation Decisions

### Daemon Runtime & Lifecycle
- Single `src/main.rs` daemon binary named `crawler` (OPS-01: "single Rust daemon binary"). Library crate stays as-is; the binary wires the existing modules.
- Unified continuous loop: run the initial crawl to frontier exhaustion, then keep running — the staleness scanner re-enqueues stale pubkeys into the *same* `pubkeys.status='discovered'` frontier `run_crawl` already drains. The loop no longer terminates on empty frontier while the daemon is live; it idles/polls when the frontier is empty.
- Graceful shutdown via `tokio::signal` (SIGTERM + SIGINT) driving a `CancellationToken` (add the tokio `signal` feature): stop claiming new work, drain in-flight workers, leave the DB consistent with no orphaned leases (OPS-02). In-progress rows left by a hard kill are still reclaimed at next startup.
- Keep the existing startup `reclaim_stale_on_startup` AND add a periodic in-run stale-lease reclaim sweep (explicitly deferred from Phase 3 at `frontier.rs:98`) so long multi-day runs recover abandoned leases without a restart.

### Configuration
- TOML config file loaded via the `config` crate (already a dependency, currently unused).
- Config file path supplied via a `--config` CLI flag (add `clap`); env-var overrides layered on top (`WOT__*` prefix, double-underscore nesting).
- Full tunable set, each defaulting to the existing `DEFAULT_*` constants so config is optional where sensible: anchor pubkey, curated relay set, staleness TTL, database URL, concurrency cap (`DEFAULT_CONCURRENCY=8`), batch size (`DEFAULT_BATCH_SIZE=64`), max attempts (`DEFAULT_MAX_ATTEMPTS=3`), fetch timeout (`DEFAULT_FETCH_TIMEOUT=30s`), per-relay rate limit (`DEFAULT_REQS_PER_SECOND=4`), metrics + health bind addresses, log level/format, progress-summary interval, staleness-scan interval, in-run reclaim interval.
- Fail-fast validation at startup: validate the anchor pubkey (hex/bech32), require a non-empty relay set, require a parseable DB URL, require TTL > 0. On invalid config, exit non-zero with a clear, actionable error before any crawl work begins.

### Staleness / TTL Refresh
- Single uniform, configurable TTL (humantime duration, e.g. `24h`) per FRESH-02's "configurable uniform TTL". Per-status TTLs are explicitly NOT in scope.
- Staleness scanner: a periodic `UPDATE` flips `fetched` / `not_found` / `failed` rows whose `last_fetched_at` is older than the TTL back to `status='discovered'`, resetting `claimed_at=NULL` and `fetch_attempts=0` (mirrors `reclaim_stale_on_startup`; re-fetch cycles must not inherit prior relay-failure retry counts). Re-enqueued rows are picked up by the next `claim_batch` — no change to the claim/apply path.
- New migration `0003` adds an index supporting the staleness scan over `last_fetched_at` (the existing `pubkeys_status_idx` partial index deliberately excludes `fetched`, which the scanner must re-enqueue). Keep the migration additive/idempotent per the established 0001/0002 conventions.
- FRESH-03 churn capture: `apply_follow_list` already returns a changed-bool; persist per-pubkey churn signal as new columns on `pubkeys` — `last_changed_at TIMESTAMPTZ` and a `change_count` / `refresh_count` pair (cheap, no separate table) — so a future adaptive policy (FRESH-04, v2) has the data without reprocessing. Keep these bookkeeping columns out of the public `pubkey_freshness` contract view unless they belong there (follow the 0002 precedent of hiding internal columns).

### Observability
- A single `axum` HTTP server exposes both `/metrics` (backed by `metrics-exporter-prometheus` `PrometheusBuilder` / handle) and the health endpoints, bound to a configurable address (OBS-01).
- Separate health endpoints per OBS-03: `/health/live` (process is up) and `/health/ready` (DB reachable AND the crawl loop is running) for process supervisors.
- Structured logging via `tracing` + `tracing-subscriber` with an `EnvFilter` (config/`RUST_LOG`-driven levels, OBS-02). Human-readable format by default; JSON output selectable via config for log shipping.
- Metrics surface (OBS-01): crawl coverage, staleness distribution, relay health (from the existing `RateLimiterRegistry` failure counts), frontier depth, fetch rate, and validation-failure counts (the existing `metrics::counter!` call sites become real once the exporter is installed). Add gauges/histograms where the current code only has counters.
- Periodic crawl-progress summaries (frontier size, fetch rate, coverage %) logged at a configurable interval during the long initial crawl (OBS-04).
- A Grafana dashboard JSON covering the exported metrics is committed to the repo under `ops/` (OBS-05).
</decisions>

<code_context>
## Existing Code Insights

### Reusable Assets
- `src/crawl/mod.rs::run_crawl` (lines 117–221): the bounded worker-pool BFS loop — seeds anchor, startup-reclaims, claims batches, spawns `process_batch` under a `Semaphore`, two-phase termination on empty frontier. The daemon wraps/extends this for the continuous + signal-driven case. `CrawlStats { reclaimed_on_startup, authors_claimed, batches_processed }` is the metrics seed.
- `DEFAULT_*` tunables already isolated and tagged for Phase 4: `crawl/mod.rs:39/49/59` (batch/concurrency/max_attempts), `relay/rate_limit.rs:39` (reqs/sec), `relay/fetch.rs:31` (fetch timeout), `relay/mod.rs:39` (retry interval). `store/mod.rs:24` `MAX_CONNECTIONS=8` (private).
- `src/crawl/frontier.rs`: `seed_anchor`, `claim_batch`, `reclaim_stale_on_startup` (resets `in_progress→discovered`), `requeue_or_fail` — the staleness scanner mirrors the reclaim UPDATE pattern. Comment at `frontier.rs:98` explicitly defers the continuous in-run reclaim to Phase 4.
- `metrics::counter!` already wired at 6 sites (`ingest/replaceable.rs:39`, `ingest/verify.rs:34/41`, `ingest/follow_list.rs:57`, `relay/rate_limit.rs:206/213`) — currently sunk into a no-op recorder; installing the Prometheus exporter at startup makes them live.
- `RateLimiterRegistry` (`relay/rate_limit.rs`): per-relay consecutive-failure counts (`failure_count`, `active_relay_count`, `reset` on success) — the relay-health metric source. `spawn_notice_consumer` (`relay/mod.rs:280–311`) routes rate-limited/blocked NOTICEs.
- Schema is ready: `pubkeys.last_fetched_at TIMESTAMPTZ` stamped on every terminal transition (FRESH-01); `status` CHECK domain includes `discovered/in_progress/fetched/not_found/failed`.

### Established Patterns
- Raw SQL via `sqlx::query!` / `query_scalar!` macros with `$N` binds; bytea pubkeys as `Vec<u8>`/`&[u8]`; `.sqlx/` offline metadata committed and regenerated with `cargo sqlx prepare` after query changes (build with `SQLX_OFFLINE=true`).
- Migrations additive + idempotent (`IF NOT EXISTS`, `CREATE OR REPLACE VIEW`, named DROP+ADD for CHECK widen, `COMMENT ON ... INTERNAL` to keep bookkeeping columns out of contract views) — see 0001/0002.
- Multi-statement atomic writes via `pool.begin()` / `.execute(&mut *tx)` / `tx.commit()`.
- Integration tests use the testcontainers Postgres fixture + deterministic seeds + the offline `ScriptedRelay`/`ScriptedGraph` mocks — never live relays. KNOWN FLAKE: testcontainers container-creation race under full-suite load; run per-binary with `-- --test-threads=2` and re-run once on a container/port timeout.

### Integration Points
- New `src/main.rs` binary: load config → init tracing → install metrics exporter → start axum metrics/health server → build PgPool → construct relay client/fetch closure (reuse Phase 2 `acquire_validated_lists_client` wiring) → run the daemon loop (initial crawl + staleness scanner + in-run reclaim) under a `CancellationToken`.
- `Cargo.toml`: add `clap`, `tracing`, `tracing-subscriber`, `metrics-exporter-prometheus`, `axum`, and the tokio `signal` feature (versions per CLAUDE.md stack table).
- `tokio` currently has only `rt-multi-thread`, `macros` — add `signal` (and any needed for axum/time).
</code_context>

<specifics>
## Specific Ideas

- Config file format TOML; example/committed `config.example.toml` documenting every field is desirable (single-operator, "config file + README is enough" constraint).
- Grafana dashboard JSON lives under `ops/` and renders the OBS-01 metric set (coverage, staleness distribution, relay health, frontier depth, fetch rate, validation failures).
- The two STATE.md concerns this phase informs: curated-relay-set coverage % (gates Phase 5 scope) should be one of the exported coverage metrics; the multi-day full-scale resource profile should be observable via the progress summaries + metrics.
</specifics>

<deferred>
## Deferred Ideas

- Adaptive per-pubkey refresh intervals derived from observed churn (FRESH-04, v2) — Phase 4 only accumulates the FRESH-03 churn data.
- NIP-65 outbox routing and relay-health-driven routing / per-relay concurrency steering (Phase 5).
- Per-status or per-pubkey differentiated TTLs — Phase 4 ships a single uniform TTL only.
</deferred>
