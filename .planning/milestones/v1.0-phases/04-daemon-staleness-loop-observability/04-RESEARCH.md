# Phase 4: Daemon, Staleness Loop & Observability - Research

**Researched:** 2026-06-15
**Domain:** Rust async daemon lifecycle (tokio), Prometheus metrics + axum HTTP, tracing, config layering, Postgres staleness sweeps (sqlx)
**Confidence:** HIGH

<user_constraints>
## User Constraints (from CONTEXT.md)

> All four "grey areas" were ACCEPTED as recommended. The decisions below are LOCKED — research the HOW, do not relitigate the WHAT.

### Locked Decisions

**Daemon Runtime & Lifecycle**
- Single `src/main.rs` daemon binary named `crawler` (OPS-01). Library crate stays as-is; the binary wires existing modules.
- Unified continuous loop: run the initial crawl to frontier exhaustion, then keep running — the staleness scanner re-enqueues stale pubkeys into the *same* `pubkeys.status='discovered'` frontier `run_crawl` already drains. The loop no longer terminates on empty frontier while the daemon is live; it idles/polls when the frontier is empty.
- Graceful shutdown via `tokio::signal` (SIGTERM + SIGINT) driving a `CancellationToken` (add the tokio `signal` feature): stop claiming new work, drain in-flight workers, leave the DB consistent with no orphaned leases (OPS-02). In-progress rows left by a hard kill are still reclaimed at next startup.
- Keep the existing startup `reclaim_stale_on_startup` AND add a periodic in-run stale-lease reclaim sweep (deferred from Phase 3 at `frontier.rs:98`).

**Configuration**
- TOML config file loaded via the `config` crate (already a dependency, currently unused).
- Config file path supplied via a `--config` CLI flag (add `clap`); env-var overrides layered on top (`WOT__` prefix, double-underscore nesting).
- Full tunable set, each defaulting to the existing `DEFAULT_*` constants: anchor pubkey, curated relay set, staleness TTL, database URL, concurrency cap (`DEFAULT_CONCURRENCY=8`), batch size (`DEFAULT_BATCH_SIZE=64`), max attempts (`DEFAULT_MAX_ATTEMPTS=3`), fetch timeout (`DEFAULT_FETCH_TIMEOUT=30s`), per-relay rate limit (`DEFAULT_REQS_PER_SECOND=4`), metrics + health bind addresses, log level/format, progress-summary interval, staleness-scan interval, in-run reclaim interval.
- Fail-fast validation at startup: validate anchor pubkey (hex/bech32), require a non-empty relay set, require a parseable DB URL, require TTL > 0. On invalid config, exit non-zero with a clear, actionable error before any crawl work begins.

**Staleness / TTL Refresh**
- Single uniform, configurable TTL (humantime duration, e.g. `24h`) per FRESH-02. Per-status TTLs are explicitly NOT in scope.
- Staleness scanner: a periodic `UPDATE` flips `fetched`/`not_found`/`failed` rows whose `last_fetched_at` is older than TTL back to `status='discovered'`, resetting `claimed_at=NULL` and `fetch_attempts=0` (mirrors `reclaim_stale_on_startup`). Re-enqueued rows are picked up by the next `claim_batch` — no change to the claim/apply path.
- New migration `0003` adds an index supporting the staleness scan over `last_fetched_at` (the existing `pubkeys_status_idx` partial index deliberately excludes `fetched`). Migration is additive/idempotent per 0001/0002 conventions.
- FRESH-03 churn capture: `apply_follow_list` already returns a changed-bool; persist per-pubkey churn signal as new columns on `pubkeys`. Keep these bookkeeping columns out of the public `pubkey_freshness` contract view unless they belong there (follow the 0002 precedent).

**Observability**
- A single `axum` HTTP server exposes both `/metrics` (backed by `metrics-exporter-prometheus` `PrometheusBuilder`/handle) and the health endpoints, bound to a configurable address (OBS-01).
- Separate health endpoints per OBS-03: `/health/live` (process is up) and `/health/ready` (DB reachable AND the crawl loop is running).
- Structured logging via `tracing` + `tracing-subscriber` with an `EnvFilter` (config/`RUST_LOG`-driven levels, OBS-02). Human-readable by default; JSON selectable via config.
- Metrics surface (OBS-01): crawl coverage, staleness distribution, relay health (from the existing `RateLimiterRegistry` failure counts), frontier depth, fetch rate, and validation-failure counts. Add gauges/histograms where the current code only has counters.
- Periodic crawl-progress summaries (frontier size, fetch rate, coverage %) logged at a configurable interval (OBS-04).
- A Grafana dashboard JSON committed under `ops/` (OBS-05).

### Claude's Discretion
- Exact metric names, label cardinality, and histogram bucket boundaries.
- Internal layout of `src/main.rs` vs. a small `src/daemon/` submodule tree for the binary's wiring.
- Whether churn is one column-pair or a small set; the exact column names.
- Poll/idle interval when the frontier is empty.
- Exact Grafana panel set as long as it renders the OBS-01 metric series.

### Deferred Ideas (OUT OF SCOPE)
- Adaptive per-pubkey refresh intervals from churn (FRESH-04, v2) — Phase 4 only *accumulates* the FRESH-03 churn data.
- NIP-65 outbox routing and relay-health-driven routing / per-relay concurrency steering (Phase 5).
- Per-status or per-pubkey differentiated TTLs — Phase 4 ships a single uniform TTL only.
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| FRESH-02 | Staleness scanner enqueues pubkeys past a configurable uniform TTL into the same frontier the initial crawl uses | Periodic `UPDATE` mirroring `reclaim_stale_on_startup` (frontier.rs:99); migration 0003 `last_fetched_at` index; humantime TTL parse (Staleness Scanner section) |
| FRESH-03 | Each refresh records whether the follow list actually changed, accumulating per-pubkey churn data | `apply_follow_list` already bumps `change_count`/`last_changed_at` on changed (follows.rs:120-135) — **the columns already exist** (see Runtime State Inventory + Migration 0003 finding) |
| OBS-01 | Prometheus `/metrics` exposes coverage, staleness distribution, relay health, frontier depth, fetch rate, validation-failure counts | `PrometheusBuilder::install_recorder()` → `PrometheusHandle::render()` in an axum handler; gauge/histogram registration (Metrics Architecture section) |
| OBS-02 | Structured logging via `tracing` with configurable levels | `tracing-subscriber` `EnvFilter` + switchable fmt/JSON layer (Tracing section) |
| OBS-03 | HTTP health endpoint (liveness/readiness) for supervisors | `/health/live` + `/health/ready` axum handlers sharing a readiness `Arc<AtomicBool>` + DB ping (Health Endpoints section) |
| OBS-04 | Periodic crawl-progress summaries during the long crawl | A `tokio::select!` interval task reading shared crawl counters; logged via `tracing::info!` (Progress Summary section) |
| OBS-05 | Grafana dashboard JSON committed | Static JSON under `ops/` rendering the OBS-01 series (Grafana section) |
| OPS-01 | Single Rust daemon binary configured via config file | `src/main.rs` + `config` crate TOML + `clap --config` + `WOT__` env overlay (Config section) |
| OPS-02 | Graceful shutdown drains in-flight work, leaves DB consistent, no orphaned leases | `CancellationToken` + `tokio::signal`; stop claiming, drain workers (the existing two-phase join already drains); axum graceful shutdown (Shutdown section) |
</phase_requirements>

## Summary

This phase adds **no new domain logic** — it is a **wiring and lifecycle** phase. The proven Phase 3 `run_crawl` BFS loop, the frontier lease/reclaim primitives, the `apply_follow_list` churn writer, and the six existing `metrics::counter!` call sites are all already correct; Phase 4 turns them into a long-running daemon binary with a recorder installed, a config front-end, a staleness re-enqueue sweep, and an HTTP observability surface. The single most important framing: **the staleness scanner is a near-verbatim copy of `reclaim_stale_on_startup`** (a periodic `UPDATE … SET status='discovered', claimed_at=NULL, fetch_attempts=0` keyed on `last_fetched_at < now()-TTL` instead of `status='in_progress'`), and **FRESH-03 churn columns already exist in migration 0001 and are already written by `apply_follow_list`** — so migration 0003 only needs the `last_fetched_at` index, not new churn columns (this contradicts a literal reading of CONTEXT, see the Runtime State Inventory and Migration 0003 findings — flag to the planner).

The metrics integration is the one place with a real design choice, and the clean answer is settled: install the recorder with `PrometheusBuilder::install_recorder()` (no `http-listener` feature, no second hyper server) which returns a `PrometheusHandle`; render `handle.render()` from an ordinary axum `GET /metrics` handler on the **same** axum server as the health endpoints. The existing fire-and-forget `metrics::counter!` calls become live the instant the global recorder is installed — no call-site changes needed. Gauges/histograms (frontier depth, coverage, staleness distribution, fetch latency) are added as new `metrics::gauge!`/`metrics::histogram!` emit points, mostly from a periodic sampler task that runs cheap DB aggregate queries.

The continuous loop is built by wrapping (not rewriting) `run_crawl`'s body in a `tokio::select!` against a `CancellationToken`, replacing the "break on empty frontier" with "idle-poll on empty frontier," and spawning two side timers (staleness scan, in-run reclaim). The crate's `run_crawl` currently *owns* its loop and termination; the cleanest path is a small refactor that extracts the claim→spawn→drain body so the daemon can drive it with cancellation, OR a new `run_daemon` entry that reuses `claim_batch`/`process_batch`/the semaphore pattern directly. Both are viable; see Architecture Patterns.

**Primary recommendation:** Add a thin `src/main.rs` + `src/daemon/` module tree that (1) loads+validates config, (2) installs tracing and the Prometheus recorder, (3) starts one axum server for `/metrics` + `/health/*` with graceful shutdown, (4) builds the `PgPool` and the live-relay `fetch_union` closure (fan out `acquire_validated_lists_client` per curated relay, concat raw events), and (5) runs a cancellation-aware continuous crawl loop with staleness-scan, in-run-reclaim, and progress-summary timers — reusing the Phase 3 frontier/apply primitives verbatim. Only five new dependencies (`axum`, `clap`, `tracing`, `tracing-subscriber`, `metrics-exporter-prometheus`) plus `tokio-util` (for `CancellationToken`) and `humantime`/`humantime-serde` (TTL parsing) — all in the locked stack table except `axum`/`tokio-util`/`humantime`, which CONTEXT already anticipated.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Config load + validate + env overlay | Process bootstrap (`main.rs`) | — | Must run before any subsystem; fail-fast (OPS-01). |
| Tracing init (fmt/JSON + EnvFilter) | Process bootstrap | — | Global subscriber must be set before first span/log (OBS-02). |
| Prometheus recorder install | Process bootstrap | metrics facade | Global recorder must be installed before any `counter!` fires for the value to be retained (OBS-01). |
| `/metrics` + `/health/*` HTTP | axum server task | metrics-exporter / DB ping | One server, two route groups; renders the recorder handle + reports readiness (OBS-01/OBS-03). |
| Signal → cancellation | Process bootstrap + signal task | `CancellationToken` | `tokio::signal` listens, cancels the shared token; every long task selects on it (OPS-02). |
| Continuous crawl loop | Daemon loop task | Phase 3 `claim_batch`/`process_batch`/semaphore | Reuses the proven BFS mechanics; only the termination/idle/cancel behavior is new. |
| Staleness scan (TTL re-enqueue) | Periodic timer task | DB `UPDATE` (mirrors reclaim) | Independent cadence from the crawl; feeds the same `discovered` frontier (FRESH-02). |
| In-run stale-lease reclaim | Periodic timer task | DB `UPDATE` (frontier.rs pattern) | Recovers leases orphaned mid-run without a restart (OPS-02 robustness). |
| Churn accumulation (FRESH-03) | Store writer (`apply_follow_list`) | **already implemented** | `change_count`/`last_changed_at` already bumped on change; no new write path. |
| Metric sampling (frontier depth, coverage, staleness dist.) | Periodic sampler task | DB aggregate queries → `gauge!`/`histogram!` | Gauges reflect point-in-time DB state; cheap `GROUP BY status`/age-bucket counts (OBS-01). |
| Progress summaries | Periodic timer task | shared counters + `tracing::info!` | Human-readable operator signal during the multi-day crawl (OBS-04). |

## Standard Stack

### Core (NEW dependencies this phase adds)
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `clap` | 4.6.1 | `--config <path>` CLI flag + daemon flags | [VERIFIED: crates.io] In CLAUDE.md stack table; the de-facto Rust arg parser. Use the derive feature. |
| `tracing` | 0.1.44 | Structured spans + events | [VERIFIED: crates.io] In CLAUDE.md table. Span-aware, async-correct; CLAUDE.md "What NOT to use" forbids the `log` crate for the daemon. |
| `tracing-subscriber` | 0.3.23 | Subscriber + `EnvFilter` + fmt/JSON layers | [VERIFIED: crates.io] In CLAUDE.md table. `env-filter` + `json` features needed (see Installation). |
| `metrics-exporter-prometheus` | 0.18.3 | `PrometheusBuilder`/`PrometheusHandle` recorder + render | [VERIFIED: crates.io] In CLAUDE.md table. Tracks the `metrics` 0.24 facade already in use. |
| `axum` | 0.8.9 | One HTTP server for `/metrics` + `/health/*` | [VERIFIED: crates.io] CONTEXT-anticipated new dep. tokio-rs project; built on hyper 1.x + tower 0.5. |
| `tokio-util` | 0.7.18 | `CancellationToken` for graceful shutdown | [VERIFIED: crates.io] CONTEXT-anticipated. The canonical structured-cancellation primitive for tokio. |
| `humantime-serde` | 1.1.1 | Deserialize `"24h"`/`"30s"` TOML strings into `Duration` | [VERIFIED: crates.io] Thin serde adapter over `humantime`; lets config fields be `#[serde(with = "humantime_serde")]` durations. |

### Supporting (feature flags toggled on existing deps)
| Library | Change | Purpose | When to Use |
|---------|--------|---------|-------------|
| `tokio` | add `signal` feature (and `time` if not transitively present) | SIGTERM/SIGINT handling; interval timers | OPS-02 graceful shutdown; periodic tasks. Current Cargo.toml has only `rt-multi-thread`, `macros`. |
| `humantime` | 2.3.0 (transitive via `humantime-serde`, or direct) | Parse/format `Duration` human strings | TTL/interval config fields. Pull direct only if you format durations in log output. |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| axum `GET /metrics` rendering `handle.render()` | `metrics-exporter-prometheus` `http-listener` feature (its own hyper server) | The built-in listener spins a *second* hyper server on a separate port and would need the `http-listener` feature (pulls `hyper`/`hyper-util`/`http-body-util`). Rendering from the shared axum server is simpler, single-port, and avoids the extra feature surface. **Recommended: axum handler.** [VERIFIED: crates.io deps] |
| `humantime-serde` on config struct | manual `String` → `humantime::parse_duration` in validation | `humantime-serde` is cleaner (durations deserialize directly); manual parse gives a custom error message. Either works; prefer `humantime-serde` + a validation pass for the friendly message. |
| `tokio-util::CancellationToken` | a `tokio::sync::watch::<bool>` or `broadcast` shutdown channel | `CancellationToken` is purpose-built (`cancelled()` future, `child_token()`, cheap clone) and reads cleanly in `select!`. watch/broadcast work but are lower-level. **Recommended: CancellationToken.** |
| `clap` derive | `clap` builder API | Derive is terser for a handful of flags. Either is fine. |

**Installation:**
```toml
# Cargo.toml additions
[dependencies]
clap = { version = "4.6.1", features = ["derive"] }
tracing = "0.1.44"
tracing-subscriber = { version = "0.3.23", features = ["env-filter", "json", "fmt"] }
metrics-exporter-prometheus = { version = "0.18.3", default-features = false }  # NO http-listener
axum = "0.8.9"
tokio-util = { version = "0.7.18", features = ["rt"] }   # CancellationToken
humantime-serde = "1.1.1"
# bump tokio features:
tokio = { version = "1.52", features = ["rt-multi-thread", "macros", "signal", "time"] }

[[bin]]
name = "crawler"
path = "src/main.rs"
```

**Version verification (2026-06-15, crates.io API):** axum 0.8.9, tower-http 0.6.11, tokio-util 0.7.18, humantime 2.3.0, humantime-serde 1.1.1, tracing 0.1.44, tracing-subscriber 0.3.23, metrics-exporter-prometheus 0.18.3, clap 4.6.1, hyper 1.10.1. All current stable. [VERIFIED: crates.io]

> **Note on `tower-http`:** not required unless you want middleware (request logging, timeouts) on the observability server. The `/metrics` + `/health` endpoints need no middleware. **Recommendation: do NOT add `tower-http`** — keep the observability server dependency-minimal. Listed here only because CONTEXT's research questions mention it.

> **Note on `metrics-exporter-prometheus` features:** `default-features = false` drops the optional `hyper`/`tokio`/`tracing` listener machinery and keeps just the recorder + renderer. Confirm `set_buckets`/`set_buckets_for_metric`/`install_recorder` remain available with default features off (they are core builder methods, not gated by `http-listener`). [CITED: docs.rs/metrics-exporter-prometheus/0.18.3]

## Package Legitimacy Audit

> `gsd-tools query package-legitimacy check` was unavailable in this environment; verdicts derived from direct crates.io metadata (downloads, age, source repo) per the ecosystem-verification protocol.

| Package | Registry | Age | Recent Downloads | Source Repo | Verdict | Disposition |
|---------|----------|-----|------------------|-------------|---------|-------------|
| axum | crates.io | since 2021-07 | ~86M | github.com/tokio-rs/axum | OK | Approved |
| tower-http | crates.io | since 2017 | ~88M | github.com/tower-rs/tower-http | OK | Approved (but not recommended — see note) |
| tokio-util | crates.io | since 2018 | ~125M | github.com/tokio-rs/tokio | OK | Approved |
| humantime | crates.io | since 2016 | ~55M | github.com/chronotope/humantime | OK | Approved (transitive via humantime-serde) |
| humantime-serde | crates.io | since 2019 | ~11M | github.com/jean-airoldie/humantime-serde | OK | Approved |
| clap | crates.io | mature | ~180M | github.com/clap-rs/clap | OK | Approved (in CLAUDE.md table) |
| tracing | crates.io | mature | ~146M | github.com/tokio-rs/tracing | OK | Approved (in CLAUDE.md table) |
| tracing-subscriber | crates.io | mature | ~112M | github.com/tokio-rs/tracing | OK | Approved (in CLAUDE.md table) |
| metrics-exporter-prometheus | crates.io | mature | ~9M | github.com/metrics-rs/metrics | OK | Approved (in CLAUDE.md table) |

**Packages removed due to [SLOP] verdict:** none.
**Packages flagged as suspicious [SUS]:** none.

All packages are first-party crates of the tokio-rs / tower-rs / clap-rs / metrics-rs ecosystems with multi-year histories and tens-to-hundreds of millions of recent downloads. No new-package or cross-ecosystem-confusion risk.

## Architecture Patterns

### System Architecture Diagram

```
                          ┌─────────────────────── src/main.rs (bin "crawler") ───────────────────────┐
                          │                                                                            │
  CLI: --config path ───► │ 1. clap parse ──► 2. config crate: TOML File + WOT__ Env overlay           │
                          │                          │                                                 │
                          │                          ▼                                                 │
                          │                   3. validate (anchor pubkey, relays non-empty,            │
                          │                      DB URL parses, TTL>0) ── invalid ──► exit(non-zero)    │
                          │                          │ valid                                           │
                          │                          ▼                                                 │
                          │   4. init tracing (EnvFilter + fmt|json layer)                             │
                          │   5. PrometheusBuilder.set_buckets(...).install_recorder() ► PrometheusHandle
                          │   6. store::connect(db_url) ► PgPool ; run_migrations (0003)               │
                          │   7. CancellationToken (shared, cloned into every task)                    │
                          │   8. Arc<AtomicBool> ready-flag                                             │
                          │                          │                                                 │
   SIGTERM / SIGINT ─────►│   signal task: select(SIGTERM|SIGINT) ──► token.cancel()                   │
                          │                          │                                                 │
        spawns ───────────┼──────────────┬───────────────┬───────────────┬───────────────┐            │
                          │               ▼               ▼               ▼               ▼            │
                          │  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐ ┌──────────────┐       │
                          │  │ axum server  │ │ continuous   │ │ staleness    │ │ sampler +    │       │
                          │  │ /metrics     │ │ crawl loop   │ │ scan timer   │ │ progress     │       │
                          │  │ /health/live │ │ (claim→spawn │ │ (UPDATE TTL  │ │ summary      │       │
                          │  │ /health/ready│ │  →apply,     │ │  re-enqueue) │ │ timers       │       │
                          │  │  renders     │ │  idle-poll   │ │      +       │ │ DB aggregates│       │
                          │  │  handle +    │ │  on empty,   │ │ in-run       │ │  ► gauge!/   │       │
                          │  │  ready-flag  │ │  select on   │ │ reclaim      │ │  histogram!  │       │
                          │  │  + DB ping)  │ │  token)      │ │ sweep        │ │ ► tracing    │       │
                          │  └──────┬───────┘ └──────┬───────┘ └──────┬───────┘ └──────┬───────┘       │
                          │         │                │                │                │               │
                          └─────────┼────────────────┼────────────────┼────────────────┼───────────────┘
                                    │ all select! on token.cancelled()
                                    ▼                ▼                ▼                ▼
                            graceful shutdown:  stop claiming, drain in-flight workers (existing two-phase
                            join), axum with_graceful_shutdown completes, timers stop ─► clean exit, no orphan leases

  ── data flow ──►  PgPool  ◄── crawl loop writes (apply_follow_list: edges + churn) / staleness UPDATE / sampler reads
                    metrics global recorder ◄── counter!/gauge!/histogram! from all tasks ──► PrometheusHandle.render() ──► /metrics
                    live relays ◄── fetch_union closure (fan out acquire_validated_lists_client per curated relay) ── crawl loop
```

### Recommended Project Structure
```
src/
├── main.rs              # bin entry: parse → load → validate → init → spawn → await shutdown
├── daemon/
│   ├── mod.rs           # run() orchestrator; CancellationToken + task JoinSet wiring
│   ├── config.rs        # Config struct (serde), load_config(), validate(), DEFAULT_* defaults
│   ├── observe.rs       # tracing init, PrometheusBuilder setup, axum router (/metrics + /health/*)
│   ├── loop_.rs         # continuous cancellation-aware crawl loop (reuses claim_batch/process_batch)
│   └── sampler.rs       # periodic DB-aggregate gauge sampler + progress-summary logger
ops/
└── grafana-dashboard.json   # OBS-05 committed dashboard
config.example.toml      # documents every field (single-operator "config + README" constraint)
migrations/
└── 0003_staleness.sql   # last_fetched_at index (+ any FRESH-03 column only if not already present — see finding)
```
(Library `src/{store,relay,ingest,crawl}` is untouched except possibly a small `run_crawl` refactor — see Pattern 2.)

### Pattern 1: Install the global Prometheus recorder, render from axum
**What:** Install the recorder once at startup; the `metrics::counter!`/`gauge!`/`histogram!` macros (the facade) then route to it globally. Render the Prometheus text exposition from an axum handler.
**When to use:** Always — this is the OBS-01 backbone.
**Example:**
```rust
// Source: docs.rs/metrics-exporter-prometheus/0.18.3 (PrometheusBuilder) [CITED]
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle, Matcher};
use axum::{routing::get, Router, extract::State, http::StatusCode};

fn install_metrics() -> PrometheusHandle {
    PrometheusBuilder::new()
        // Turn fetch-latency / staleness-age histograms into real Prometheus
        // buckets (otherwise they export as summaries). Discretion: pick buckets.
        .set_buckets_for_metric(
            Matcher::Full("fetch_duration_seconds".to_string()),
            &[0.1, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0],
        ).expect("valid buckets")
        .install_recorder()          // installs the GLOBAL recorder, no HTTP listener
        .expect("recorder installs once")
}

async fn metrics_handler(State(handle): State<PrometheusHandle>) -> String {
    handle.render()                  // Prometheus text exposition
}

// Router shares the handle + readiness flag as axum state.
let app = Router::new()
    .route("/metrics", get(metrics_handler))
    .route("/health/live", get(|| async { StatusCode::OK }))
    .route("/health/ready", get(ready_handler))
    .with_state(/* (handle, ready_flag, pool) bundled in an AppState */);
```
**Critical ordering:** `install_recorder()` MUST run before any code path fires a `metrics::counter!`. The six existing call sites (`ingest/replaceable.rs:39`, `ingest/verify.rs:34/41`, `ingest/follow_list.rs:57`, `relay/rate_limit.rs:206/213`) are currently sunk into a no-op recorder; they become live automatically once installed — **no edits to those files** (VERIFIED by reading the call sites).

### Pattern 2: Continuous, cancellation-aware crawl loop (reuse, don't rewrite)
**What:** The Phase 3 `run_crawl` loop *breaks* on a drained frontier. The daemon instead idles-and-polls on a drained frontier and exits only on cancellation. Two clean options:

**Option A (recommended): new `run_daemon_loop` in `src/daemon/loop_.rs` that reuses the primitives.** Reuse `seed_anchor`, `reclaim_stale_on_startup`, `claim_batch`, `process_batch`, and the exact semaphore-permit-before-spawn pattern from `run_crawl` (mod.rs:142-218). Change only the termination: an empty claim → `tokio::select!` on `token.cancelled()` vs a short `tokio::time::sleep(poll_interval)`, then continue. Keep the two-phase drain (join in-flight workers) as the shutdown drain.

**Option B: refactor `run_crawl` to take a `CancellationToken` + `LoopMode { RunToExhaustion, Continuous }`.** Touches library code and the Phase 3 tests; more invasive. Prefer A to keep the proven library function and its 5 green tests untouched.

**When to use:** Option A unless the planner wants a single unified entry.
**Example (Option A skeleton):**
```rust
// Reuses crate::crawl primitives verbatim; only loop control is new.
pub async fn run_daemon_loop(
    pool: &PgPool, anchor: &[u8], cfg: &CrawlCfg,
    fetch_union: F, token: CancellationToken, poll: Duration,
) -> Result<(), StoreError> {
    seed_anchor(pool, anchor).await?;
    reclaim_stale_on_startup(pool).await?;          // startup orphan reclaim (kept)
    let sem = Arc::new(Semaphore::new(cfg.concurrency));
    let mut workers = Vec::new();
    loop {
        if token.is_cancelled() { break; }          // stop CLAIMING new work (OPS-02)
        let batch = claim_batch(pool, cfg.batch_size).await?;
        if batch.is_empty() {
            // drain in-flight (they may discover followees), then idle-poll or cancel.
            for h in workers.drain(..) { join_worker(h).await?; }
            tokio::select! {
                _ = token.cancelled() => break,
                _ = tokio::time::sleep(poll) => continue,   // frontier empty: wait, re-check
            }
        }
        let permit = Arc::clone(&sem).acquire_owned().await.unwrap();  // backpressure (CRAWL-04)
        // ... spawn process_batch exactly as run_crawl does (mod.rs:175-217) ...
    }
    // DRAIN: join every in-flight worker so claimed leases reach a terminal state
    // (fetched/not_found/failed) — leaves DB consistent, no orphaned in_progress (OPS-02).
    for h in workers.drain(..) { join_worker(h).await?; }
    Ok(())
}
```
**Note on `join_worker`:** it is currently a private fn in `crawl/mod.rs`. To reuse it in `daemon/loop_.rs`, either make it `pub(crate)` (trivial library change) or duplicate the ~12-line flatten. Prefer `pub(crate)`.

### Pattern 3: Staleness scanner = parametrized reclaim
**What:** A periodic `UPDATE` that mirrors `reclaim_stale_on_startup` (frontier.rs:99-113) but keys on age rather than lease state.
**When to use:** FRESH-02. Runs on its own `staleness_scan_interval` timer, independent of the crawl cadence.
**Example SQL:**
```sql
-- Re-enqueue everything past TTL. Mirrors reclaim_stale_on_startup's reset of
-- claimed_at + fetch_attempts (a re-fetch cycle must NOT inherit prior retry counts).
UPDATE pubkeys
SET status = 'discovered', claimed_at = NULL, fetch_attempts = 0
WHERE status IN ('fetched','not_found','failed')
  AND last_fetched_at < (now() - $1::interval);   -- $1 = TTL as interval/duration
```
**Rust seam:** add `frontier::reclaim_stale_by_ttl(pool, ttl) -> Result<u64, StoreError>` alongside `reclaim_stale_on_startup`, returning `rows_affected()` for a `staleness_reenqueued_total` counter. **Index dependency:** the existing `pubkeys_status_idx` is partial `WHERE status IN ('discovered','not_found','failed')` — it deliberately **excludes `fetched`**, which is the bulk of the rows the scanner must re-enqueue. Migration 0003 must add an index supporting this scan (see Migration 0003 finding).

### Pattern 4: Readiness as shared state
**What:** `/health/live` returns 200 unconditionally (the process answers HTTP → it's alive). `/health/ready` returns 200 only when (a) the DB is reachable AND (b) the crawl loop is running. Share an `Arc<AtomicBool>` the loop sets `true` after `seed_anchor` succeeds and the axum handler also does a cheap `SELECT 1` ping.
**When to use:** OBS-03.
**Example:**
```rust
async fn ready_handler(State(st): State<AppState>) -> StatusCode {
    if !st.loop_alive.load(Ordering::Relaxed) { return StatusCode::SERVICE_UNAVAILABLE; }
    match sqlx::query_scalar!("SELECT 1").fetch_one(&st.pool).await {
        Ok(_) => StatusCode::OK,
        Err(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}
```

### Pattern 5: Switchable fmt vs JSON tracing layer
**What:** One `EnvFilter` (from config level or `RUST_LOG`), one fmt layer chosen at runtime between human and JSON.
**When to use:** OBS-02.
**Example:**
```rust
// Source: docs.rs/tracing-subscriber/0.3.23 (layered Registry) [CITED]
use tracing_subscriber::{fmt, EnvFilter, prelude::*};
let filter = EnvFilter::try_from_default_env()
    .unwrap_or_else(|_| EnvFilter::new(cfg.log_level.clone()));   // e.g. "info"
let registry = tracing_subscriber::registry().with(filter);
match cfg.log_format {
    LogFormat::Json  => registry.with(fmt::layer().json()).init(),
    LogFormat::Human => registry.with(fmt::layer()).init(),
}
```
(`.json()` requires the `json` feature; `EnvFilter` requires `env-filter`.)

### Anti-Patterns to Avoid
- **Installing the recorder after firing metrics.** Any `counter!` before `install_recorder()` is dropped. Install first thing after tracing.
- **A second hyper server via the `http-listener` feature.** Redundant; render from the shared axum server (Pattern 1).
- **Holding the DB row lock across the relay fetch.** Already correctly avoided by `claim_batch`'s short-txn (frontier.rs:58-79). The staleness `UPDATE` is also a single short statement — do not wrap it in a long transaction.
- **Rewriting `run_crawl` / the apply path.** The proven loop, semaphore backpressure, two-phase drain, and `process_batch` resolution are correct. Reuse (Pattern 2 Option A).
- **Recursive CTE / reachability predicate.** Forbidden (CLAUDE.md "What NOT to use", Phase 3 Pitfall 4). The frontier is purely structural; the staleness scan is a flat age `UPDATE`.
- **High-cardinality metric labels.** The existing `relay_rate_limited{relay=…}` labels are per-relay (bounded by the curated set — fine). Do NOT add per-pubkey labels — that explodes cardinality at millions of pubkeys. Keep coverage/staleness as aggregate gauges, not labelled-per-pubkey.
- **Logging the DB URL.** `store::connect` doc-comment already warns (T-03-04). Keep it out of tracing fields and config-dump logs.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Prometheus text exposition format | A custom `/metrics` string builder | `metrics-exporter-prometheus` `PrometheusHandle::render()` | Escaping, HELP/TYPE lines, histogram bucket cumulation, summary quantiles — all subtle and standardized. |
| Graceful structured cancellation | A hand-rolled `AtomicBool` polled everywhere | `tokio_util::sync::CancellationToken` | Gives a `.cancelled()` future usable in `select!`, child tokens, cheap clones. |
| Signal handling | raw `libc`/`signal_hook` | `tokio::signal::unix::{signal, SignalKind}` + `tokio::signal::ctrl_c` | Async-integrated, cross-platform, in the locked tokio dep (add `signal` feature). |
| Human duration parsing | regex over `"24h"` | `humantime` / `humantime-serde` | Handles `30s`, `5m`, `24h`, `1d` etc.; serde integration. |
| Layered config (file + env) | manual env var reads + TOML parse | `config` crate `File` + `Environment` sources | Already a dep; precedence + nested `WOT__A__B` mapping handled (Pattern in Config section). |
| HTTP routing for 3 endpoints | raw hyper service | `axum` Router | Ergonomic routing, shared state, `with_graceful_shutdown`. |
| EnvFilter level parsing | parse `RUST_LOG` by hand | `tracing_subscriber::EnvFilter` | Directive grammar (per-module levels) is non-trivial. |

**Key insight:** This entire phase is "wire well-known building blocks"; the only bespoke logic is the staleness `UPDATE` (a one-line variant of an existing query) and the gauge-sampling aggregate queries.

## Runtime State Inventory

> This is a wiring phase over an existing library, so the relevant inventory is **what schema/state already exists** that Phase 4 assumes or must change.

| Category | Items Found | Action Required |
|----------|-------------|------------------|
| Stored data (schema) | `pubkeys` already has `last_fetched_at`, `last_changed_at`, `fetch_count`, `change_count`, `applied_event_id`, `claimed_at`, `fetch_attempts` (migrations 0001 + 0002). `status` CHECK domain = `discovered/in_progress/fetched/not_found/failed`. | **FRESH-03 churn columns ALREADY EXIST and are ALREADY written** by `apply_follow_list` (follows.rs:120-135 bumps `change_count`/`last_changed_at` on `changed`). Migration 0003 does **NOT** need new churn columns — only the `last_fetched_at` index. (See Migration 0003 finding — flag the CONTEXT/reality delta to the planner.) |
| Live service config | None — single self-hosted daemon; no external service stores crawler state. Relay set, anchor, TTL all become config (this phase). | Author `config.example.toml`; no external migration. |
| OS-registered state | None today (crate is library-only, no binary, no service unit). The operator will likely run `crawler` under systemd/supervisor, but that's deployment polish (explicitly out of scope per REQUIREMENTS "Out of Scope"). | Health endpoints (OBS-03) are designed for a future supervisor; no OS registration shipped this phase. |
| Secrets / env vars | DB URL may contain a password; loaded via config/`WOT__DATABASE__URL`. `store::connect` already documents "never logged". | Ensure config-dump logging (if any) redacts the DB URL; never put it in a tracing field. |
| Build artifacts | `.sqlx/` offline metadata is committed and built with `SQLX_OFFLINE=true`. New queries (staleness `UPDATE`, `SELECT 1` ping, gauge aggregates, any churn read) require `cargo sqlx prepare` regeneration. | Run `cargo sqlx prepare` after adding queries; commit `.sqlx/` drift. Migration 0003 must be applied to the prepare DB first. |

**Verified by:** reading migrations 0001/0002, `store/follows.rs`, `store/pubkeys.rs`, `crawl/frontier.rs`, `Cargo.toml`, and the Phase 3 summary.

## Common Pitfalls

### Pitfall 1: Recorder installed after first metric fires
**What goes wrong:** Metrics emitted before `install_recorder()` vanish; counters appear to "start late" or read zero.
**Why it happens:** The `metrics` facade routes to whatever global recorder is installed *at emit time*; before install it's a no-op.
**How to avoid:** Install the recorder in `main` immediately after tracing init, before building the pool/spawning tasks.
**Warning signs:** `/metrics` missing a series you know fired; counters lower than expected after startup.

### Pitfall 2: Staleness scan can't use the existing partial index (full-table scan)
**What goes wrong:** The TTL `UPDATE` filters `status IN ('fetched','not_found','failed') AND last_fetched_at < cutoff`, but `pubkeys_status_idx` is partial `WHERE status IN ('discovered','not_found','failed')` — it **excludes `fetched`**, the dominant re-enqueue population. At millions of rows the scan is a full seq scan every interval.
**Why it happens:** The Phase 1 partial index was built for the *claim* scan (`discovered`), not the *staleness* scan (`fetched`).
**How to avoid:** Migration 0003 adds an index on `last_fetched_at` (e.g. `CREATE INDEX IF NOT EXISTS pubkeys_last_fetched_idx ON pubkeys (last_fetched_at)` — or a partial index covering the three terminal statuses). Run `EXPLAIN` against a migrated DB to confirm the planner uses it. (See Migration 0003 finding for the exact shape.)
**Warning signs:** Staleness-scan latency grows linearly with table size; `EXPLAIN` shows `Seq Scan on pubkeys`.

### Pitfall 3: Shutdown leaves orphaned `in_progress` leases
**What goes wrong:** On SIGTERM, tasks are aborted mid-fetch; claimed rows stay `in_progress` with no worker, invisible to the `discovered`-only claim scan until next startup reclaim.
**Why it happens:** Dropping (aborting) a worker task does not run `requeue_or_fail`/terminal write.
**How to avoid:** On cancel, **stop claiming** then **drain** (await) in-flight workers so each reaches a terminal status — exactly the two-phase join `run_crawl` already does on empty-frontier (mod.rs:152-160). The startup `reclaim_stale_on_startup` is the backstop for a *hard* kill (SIGKILL), which can't be drained. OPS-02 success = a clean SIGTERM leaves zero `in_progress`.
**Warning signs:** `in_progress` count > 0 after a graceful stop; a test asserting "no orphaned leases after cancel" fails.

### Pitfall 4: `select!` cancels a worker mid-DB-write, corrupting an edge diff
**What goes wrong:** Wrapping `process_batch` itself in `select! { _ = token.cancelled() => ... }` could abort an in-flight `apply_follow_list` transaction.
**Why it happens:** Cancelling at the wrong granularity drops a future holding an open `tx`.
**How to avoid:** Cancel at the **claim** boundary, not inside a spawned worker. Let already-spawned workers run to completion during drain. `apply_follow_list` is a single atomic transaction (follows.rs:94-139); a dropped tx rolls back cleanly, but the lease would then orphan — so prefer draining over aborting.
**Warning signs:** Partial edge sets; `in_progress` rows after shutdown.

### Pitfall 5: humantime in config — wrong serde wiring
**What goes wrong:** A TOML `ttl = "24h"` fails to deserialize into a `Duration` field, or silently parses as seconds.
**Why it happens:** `std::time::Duration` has no human-string serde impl by default.
**How to avoid:** `#[serde(with = "humantime_serde")]` on each duration field, or store as `String` and `humantime::parse_duration` in `validate()` (gives a friendlier error). Validate `ttl > 0` explicitly.
**Warning signs:** Config load error on a valid-looking duration; TTL behaving as if in the wrong unit.

### Pitfall 6: Gauge sampler runs an expensive aggregate too often
**What goes wrong:** Sampling `COUNT(*) GROUP BY status` or staleness-age buckets every second is a repeated full aggregate over millions of rows.
**Why it happens:** Treating gauges like cheap counters.
**How to avoid:** Sample on a coarse interval (e.g. every 15–60s, configurable). The claim scan and edge writes are the hot path; the sampler is observability and should be cheap and infrequent. Coverage = `fetched / total` is a single grouped count.
**Warning signs:** DB CPU spikes correlated with the metrics interval; pool contention with the crawl.

### Pitfall 7: High-cardinality labels from per-pubkey or per-author metrics
**What goes wrong:** A `metrics::counter!("...", "pubkey" => hex)` explodes Prometheus series at millions of pubkeys, OOMing the recorder and scraper.
**Why it happens:** Treating labels as free-form dimensions.
**How to avoid:** Aggregate. Coverage/staleness are gauges with NO per-entity label; relay metrics label by relay url (bounded curated set, already done). Validation-failure counters label by failure *kind* at most.
**Warning signs:** `/metrics` body megabytes large; recorder memory growth.

### Pitfall 8: axum graceful shutdown never completes (held connection)
**What goes wrong:** `axum::serve(...).with_graceful_shutdown(token.cancelled())` waits for in-flight requests; a hung `/metrics` scrape (or a slow DB ping in `/health/ready`) blocks shutdown.
**Why it happens:** Graceful shutdown drains connections; a stuck handler stalls it.
**How to avoid:** Keep handlers fast; put a short timeout on the `/health/ready` DB ping. Optionally bound total shutdown time with a `tokio::time::timeout` around the join of all tasks.
**Warning signs:** Daemon hangs on SIGTERM instead of exiting promptly.

## Code Examples

### Graceful shutdown wiring (signal → token → axum + tasks)
```rust
// Source: tokio::signal + tokio_util::sync::CancellationToken docs [CITED docs.rs/tokio, docs.rs/tokio-util]
use tokio_util::sync::CancellationToken;

let token = CancellationToken::new();

// Signal listener: cancel on SIGTERM or SIGINT.
{
    let token = token.clone();
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
            let mut int  = signal(SignalKind::interrupt()).expect("SIGINT handler");
            tokio::select! {
                _ = term.recv() => {},
                _ = int.recv()  => {},
            }
        }
        #[cfg(not(unix))]
        { let _ = tokio::signal::ctrl_c().await; }
        token.cancel();
    });
}

// axum server with graceful shutdown bound to the same token.
let listener = tokio::net::TcpListener::bind(cfg.metrics_addr).await?;
let server = {
    let token = token.clone();
    axum::serve(listener, app).with_graceful_shutdown(async move { token.cancelled().await; })
};
```

### Staleness scanner + in-run reclaim timers
```rust
// Both are independent interval tasks selecting on the same token.
async fn staleness_timer(pool: PgPool, ttl: Duration, every: Duration, token: CancellationToken) {
    let mut tick = tokio::time::interval(every);
    loop {
        tokio::select! {
            _ = token.cancelled() => break,
            _ = tick.tick() => {
                match crate::crawl::frontier::reclaim_stale_by_ttl(&pool, ttl).await {
                    Ok(n) => { metrics::counter!("staleness_reenqueued_total").increment(n);
                               tracing::info!(reenqueued = n, "staleness scan"); }
                    Err(e) => tracing::warn!(error = %e, "staleness scan failed"),
                }
            }
        }
    }
}
// in-run reclaim is the same shape calling reclaim_stale_on_startup-with-age
// (add an age threshold so it only reclaims leases older than e.g. 2× fetch_timeout,
//  NOT freshly-claimed in-flight rows — see Open Question 2).
```

### Config struct + load + validate
```rust
// Source: docs.rs/config/0.15 (File + Environment) [CITED]
use serde::Deserialize;
use std::time::Duration;

#[derive(Debug, Deserialize)]
struct Config {
    anchor_pubkey: String,
    relays: Vec<String>,
    database_url: String,
    #[serde(with = "humantime_serde")] ttl: Duration,
    #[serde(default = "default_concurrency")] concurrency: usize,         // DEFAULT_CONCURRENCY=8
    #[serde(default = "default_batch_size")]  batch_size: i64,            // DEFAULT_BATCH_SIZE=64
    #[serde(default = "default_max_attempts")] max_attempts: i16,         // DEFAULT_MAX_ATTEMPTS=3
    #[serde(with = "humantime_serde", default = "default_fetch_timeout")] fetch_timeout: Duration,
    #[serde(default = "default_reqs_per_second")] reqs_per_second: u32,   // DEFAULT_REQS_PER_SECOND=4
    metrics_addr: std::net::SocketAddr,
    #[serde(default = "default_log_level")] log_level: String,
    #[serde(default)] log_format: LogFormat,                             // Human | Json
    #[serde(with = "humantime_serde", default = "default_summary_interval")] progress_interval: Duration,
    #[serde(with = "humantime_serde", default = "default_scan_interval")]   staleness_scan_interval: Duration,
    #[serde(with = "humantime_serde", default = "default_reclaim_interval")] reclaim_interval: Duration,
}

fn load(path: &str) -> anyhow::Result<Config> {
    let cfg: Config = config::Config::builder()
        .add_source(config::File::with_name(path))
        .add_source(config::Environment::default().prefix("WOT").separator("__"))
        .build()?
        .try_deserialize()?;
    Ok(cfg)
}

fn validate(c: &Config) -> anyhow::Result<()> {
    // anchor: accept hex or bech32 npub via nostr_sdk::PublicKey::parse
    nostr_sdk::PublicKey::parse(&c.anchor_pubkey)
        .map_err(|_| anyhow::anyhow!("invalid anchor_pubkey: {}", c.anchor_pubkey))?;
    anyhow::ensure!(!c.relays.is_empty(), "relays must be non-empty");
    anyhow::ensure!(c.ttl > Duration::ZERO, "ttl must be > 0");
    // database_url parse: PgPool connect at startup is the authoritative check.
    Ok(())
}
```
(`nostr_sdk::PublicKey::parse` accepts both hex and bech32 `npub` — verify the exact method name against nostr-sdk 0.44; `from_bech32`/`from_hex`/`parse` exist in the rust-nostr API. [ASSUMED — confirm method name].)

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| `metrics-exporter-prometheus` built-in `http-listener` on its own port | Install recorder, render `PrometheusHandle::render()` from your app's HTTP server | Stable since 0.12+ | One server, one port, no extra feature; the standard axum+metrics pattern. |
| hyper 0.14 + axum 0.6 | hyper 1.x + axum 0.8 | hyper 1.0 (late 2023), axum 0.7→0.8 | axum 0.8 uses hyper 1.x; metrics-exporter-prometheus 0.18 optional listener also uses hyper 1.x → no version split if you ever enabled it. |
| `signal_hook` / raw libc | `tokio::signal` | tokio 1.x | First-party async signal handling; just enable the `signal` feature. |
| ad-hoc `AtomicBool` shutdown flags | `tokio_util::sync::CancellationToken` | tokio-util 0.7 | Composable `select!`-friendly cancellation. |

**Deprecated/outdated:**
- The `log` crate for this daemon — forbidden by CLAUDE.md; use `tracing`.
- axum 0.6/0.7 patterns (e.g. `Server::bind`) — axum 0.8 uses `axum::serve(listener, app)`.

## Validation Architecture

> `workflow.nyquist_validation` is not disabled; this section is REQUIRED. Phase 3 established the testcontainers Postgres fixture + offline scripted-relay pattern; Phase 4 extends it. KNOWN FLAKE: testcontainers container-creation race — run per-binary with `-- --test-threads=2`, re-run once on a container/port timeout.

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in `#[test]`/`#[tokio::test]` + `testcontainers` 0.27 / `testcontainers-modules` 0.15 (postgres) |
| Config file | `Cargo.toml` `[dev-dependencies]`; `.sqlx/` offline metadata committed |
| Quick run command | `SQLX_OFFLINE=true cargo test --test daemon_config` (pure-unit config tests, no DB) |
| Full suite command | `cargo test -- --test-threads=2` (DB integration; re-run once on container race) |

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| OPS-01 | Config loads from TOML; `WOT__*` env overrides win; defaults apply when omitted; invalid config fails validation | unit (no DB) | `cargo test --test daemon_config -- override_precedence default_fill invalid_anchor_rejected ttl_zero_rejected` | ❌ Wave 0 (`tests/daemon_config.rs`) |
| FRESH-02 | `reclaim_stale_by_ttl` re-enqueues ONLY rows past TTL, resets `claimed_at`+`fetch_attempts`, leaves fresh rows untouched | integration (DB) | `cargo test --test staleness -- reenqueues_only_stale resets_attempts fresh_untouched -- --test-threads=2` | ❌ Wave 0 (`tests/staleness.rs`) |
| FRESH-03 | A changed follow list bumps `change_count`/`last_changed_at`; an unchanged re-fetch bumps `fetch_count` only | integration (DB) | `cargo test --test graph_writer -- churn_recorded_on_change churn_not_bumped_unchanged -- --test-threads=2` | ⚠️ partially exists — `same_event_zero_touch` already asserts `change_count` unchanged (03-03 SUMMARY); add the changed-case assertion |
| OPS-02 | Graceful cancel: loop stops claiming, drains in-flight workers, zero `in_progress` rows remain | integration (DB + mock fetch) | `cargo test --test daemon_loop -- graceful_drain_no_orphan_leases -- --test-threads=2` | ❌ Wave 0 (`tests/daemon_loop.rs`) |
| OPS-02 | Continuous loop idles on empty frontier and resumes when staleness re-enqueues a row | integration (DB + mock fetch + injected token) | `cargo test --test daemon_loop -- idle_then_resume_after_reenqueue -- --test-threads=2` | ❌ Wave 0 |
| OBS-01 | `/metrics` exposes the required series after a crawl tick; `GET /metrics` returns 200 + non-empty body containing expected metric names | integration (in-process axum) | `cargo test --test observe -- metrics_endpoint_exposes_series` | ❌ Wave 0 (`tests/observe.rs`) |
| OBS-03 | `/health/live` 200 always; `/health/ready` 200 only when loop_alive AND DB reachable, 503 otherwise | integration (in-process axum + DB) | `cargo test --test observe -- live_always_ok ready_requires_db_and_loop -- --test-threads=2` | ❌ Wave 0 |
| OBS-02 | `log_format=json` produces JSON lines; configured level honored | unit | `cargo test --test observe -- json_format_selected` (or assert subscriber builds for both formats) | ❌ Wave 0 |
| OBS-04 | Progress summary computes frontier size / coverage % from DB counts | integration (DB) | `cargo test --test observe -- progress_summary_counts` | ❌ Wave 0 |
| OBS-05 | Grafana JSON is valid JSON and references the exported metric names | unit (parse + grep) | `cargo test --test observe -- dashboard_json_valid` (parse `ops/grafana-dashboard.json`, assert each OBS-01 series name present) | ❌ Wave 0 |

### Test Seams (how to test daemon/HTTP/signal code without flakiness)
- **Inject the `CancellationToken`.** `run_daemon_loop` takes a `CancellationToken` parameter (don't read signals inside it). Tests construct a token, run the loop with a mock `fetch_union` + testcontainers pool, `token.cancel()` after asserting a tick, and assert clean drain + zero `in_progress`. (Mirrors Phase 3's injected-`fetch_union` seam — reuse the `ScriptedGraph` Send helper from `tests/frontier.rs`.)
- **Hit axum handlers in-process.** Build the `Router` with test `AppState` and call handlers directly (e.g. `router.oneshot(Request::get("/health/ready"))` via `tower::ServiceExt`, or just call the handler fns with a `State`). No real TCP bind needed for handler logic; the `metrics_endpoint_exposes_series` test fires a few `counter!`/`gauge!` then asserts `handle.render()` contains the names.
- **Don't test real signals.** Signal delivery is OS-level and flaky in tests; the `token.cancel()` path IS the unit under test. Trust `tokio::signal` (locked dep) to fire the cancel; assert on the cancellation→drain behavior, not on SIGTERM delivery.
- **Recorder is global** — installing it in multiple `#[test]`s in one process conflicts (install-once). Either gate metric-render tests behind a single test that owns the install, use `PrometheusBuilder::build_recorder()` (local handle, no global install) for assertion-only tests, or run them in a dedicated test binary. **Recommendation:** use `build_recorder()` in tests to get a `PrometheusHandle` without touching the global recorder.
- **humantime-serde** is pure-unit: assert `"24h"` → `Duration::from_secs(86400)`, `"30s"` → 30s, and that `ttl=0`/`""` is rejected by `validate()`.

### Sampling Rate
- **Per task commit:** `SQLX_OFFLINE=true cargo test --test daemon_config` (fast, no DB) for config tasks; the relevant single integration test for DB tasks.
- **Per wave merge:** `cargo test -- --test-threads=2` (full suite, DB).
- **Phase gate:** full suite green + `cargo clippy --all-targets` + `cargo sqlx prepare` zero drift before `/gsd-verify-work`.

### Wave 0 Gaps
- [ ] `tests/daemon_config.rs` — config load/override/default/validation (OPS-01). No DB.
- [ ] `tests/staleness.rs` — `reclaim_stale_by_ttl` behavior (FRESH-02). Needs migration 0003 + testcontainers.
- [ ] `tests/daemon_loop.rs` — cancellation-aware loop drain + idle/resume (OPS-02). Reuse `ScriptedGraph` from `tests/frontier.rs` (make it shareable or duplicate).
- [ ] `tests/observe.rs` — metrics render, health endpoints, json format, progress counts, dashboard JSON valid (OBS-01..05). Needs `tower` dev-dep (`ServiceExt::oneshot`) if testing via the Router; otherwise call handlers directly.
- [ ] `tests/graph_writer.rs` — ADD a `churn_recorded_on_change` assertion (FRESH-03 changed case); the unchanged case already exists.
- [ ] Migration `migrations/0003_staleness.sql` must exist and be applied before `cargo sqlx prepare` regenerates `.sqlx/`.
- [ ] Possible dev-dep additions: `tower` (for `oneshot`), already-present `testcontainers`. Confirm before Wave 0.

## Security Domain

> `security_enforcement` is not disabled; this section applies. This phase adds a network-listening HTTP server, which is the primary new attack surface.

### Applicable ASVS Categories
| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | The `/metrics` + `/health` server is an internal operator/Prometheus surface; no auth in scope (bind to localhost or a trusted network — see threat table). |
| V3 Session Management | no | Stateless HTTP, no sessions. |
| V4 Access Control | partial | The observability server should NOT be exposed to the public internet — bind to a private/loopback address by default (config `metrics_addr`). |
| V5 Input Validation | yes | Config validation (anchor pubkey parse, relay set, TTL>0) is fail-fast (OPS-01). The HTTP endpoints take no untrusted input (no query params driving DB). |
| V6 Cryptography | no | No new crypto; signature verification stays in the (unchanged) ingest layer. |
| V7 Error Handling / Logging | yes | Structured tracing; MUST NOT log the DB URL / credentials (T-03-04 carried forward). |

### Known Threat Patterns for {Rust async daemon + HTTP observability}
| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| `/metrics` exposes internal topology (relay urls, coverage) to the public internet | Information Disclosure | Default-bind `metrics_addr` to `127.0.0.1` (or a private subnet); document that exposure requires an explicit operator choice. No auth needed on a trusted bind. |
| DB credentials leak via logs / config dump | Information Disclosure | Never log `database_url`; redact in any config-echo. Already a project convention (store/mod.rs). |
| High-cardinality labels → recorder/scraper OOM (DoS) | DoS | Aggregate metrics; no per-pubkey labels (Pitfall 7). |
| Orphaned leases on shutdown → stuck rows (integrity) | Tampering/Repudiation | Drain in-flight workers on cancel (OPS-02, Pitfall 3); startup reclaim backstops hard kills. |
| Staleness scan full-table-scan under load → DB starvation | DoS (self-inflicted) | Migration 0003 index + coarse scan interval (Pitfall 2, Pitfall 6). |
| Slow `/health/ready` DB ping stalls graceful shutdown | DoS (self-inflicted) | Short timeout on the ping; bound total shutdown time (Pitfall 8). |

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | `PrometheusHandle::render()` is the method name returning the exposition string in 0.18.3 (the builder page didn't list handle methods) | Pattern 1 | Low — `render()` is the long-stable method; verify against docs.rs/metrics-exporter-prometheus/0.18.3 `PrometheusHandle` page during planning. |
| A2 | `nostr_sdk::PublicKey::parse` accepts both hex and bech32 npub in 0.44 | Config example | Low–Med — rust-nostr exposes `from_hex`/`from_bech32`/`parse`; confirm the exact accepting method for the anchor validation. |
| A3 | FRESH-03 churn columns already satisfy the requirement; migration 0003 needs only the index | Runtime State Inventory, Migration 0003 finding | **Med** — if the planner/operator wants a distinct `refresh_count` separate from `fetch_count`, 0003 must add it. The existing `fetch_count`/`change_count`/`last_changed_at` cover the FRESH-03 intent; CONTEXT's "new columns" wording predates the discovery they already exist. **Flag to planner.** |
| A4 | `metrics-exporter-prometheus` with `default-features = false` still exposes `install_recorder`/`set_buckets`/`render` | Installation | Low — these are core builder/handle methods, not gated by `http-listener`; confirm during planning. |
| A5 | Rendering metrics from the shared axum server (vs the built-in listener) needs no extra crate beyond axum | Pattern 1, Alternatives | Low — verified by deps inspection; `render()` is sync and needs no hyper. |

## Open Questions (RESOLVED)
1. **Exact migration 0003 shape (index only vs index + `refresh_count`).**
   - What we know: `last_fetched_at`, `last_changed_at`, `fetch_count`, `change_count` already exist (0001) and `apply_follow_list` writes them. The staleness scan needs a `last_fetched_at` index that covers `fetched` rows (the existing partial index excludes them).
   - What's unclear: whether FRESH-03 wants a `refresh_count` semantically distinct from `fetch_count` (re-fetches after staleness vs. all fetches). The current `fetch_count` counts every fetch including the initial one.
   - Recommendation: Migration 0003 = `CREATE INDEX IF NOT EXISTS pubkeys_last_fetched_idx ON pubkeys (last_fetched_at)` (or a partial index `WHERE status IN ('fetched','not_found','failed')`). Treat churn columns as already present; only add `refresh_count` if the planner/operator explicitly wants the initial-vs-refresh distinction. Confirm in discuss-phase (A3).

2. **In-run reclaim age threshold.**
   - What we know: `reclaim_stale_on_startup` resets ALL `in_progress` (safe at startup — nothing is live). An in-run sweep must NOT reset rows freshly claimed by currently-running workers.
   - What's unclear: the threshold. `claimed_at` exists; a lease older than e.g. `2× fetch_timeout` is plausibly orphaned (its worker died).
   - Recommendation: in-run reclaim = `UPDATE … WHERE status='in_progress' AND claimed_at < now() - $threshold`, with `$threshold` a config value defaulting to a few × `fetch_timeout`. Add `reclaim_in_progress_older_than(pool, age)` alongside the startup variant.

3. **Idle-poll interval when the frontier is empty.**
   - What we know: after the initial crawl, the frontier sits empty until the staleness scan re-enqueues rows.
   - Recommendation: a config `idle_poll_interval` (e.g. 5–30s). The loop should also be promptly woken by cancellation (it is, via `select!`). Tighter coupling (notify on re-enqueue) is unnecessary — polling at the staleness cadence is fine.

4. **`run_crawl` reuse strategy (Option A new fn vs Option B refactor).**
   - Recommendation: Option A (new `run_daemon_loop` reusing primitives) to keep the proven `run_crawl` + its 5 tests untouched; make `join_worker` `pub(crate)`. Confirm with planner.

## Environment Availability
| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| PostgreSQL (running) | crawl + staleness + tests | assumed (Phases 1–3 used it via testcontainers/local) | 16/17 target | — (hard dep) |
| Docker (testcontainers) | DB integration tests | assumed (Phase 3 ran them) | — | — |
| Rust toolchain 1.94 | build (sqlx 0.9 MSRV) | assumed (pinned in Cargo.toml) | 1.94 | — |
| `cargo sqlx prepare` (sqlx-cli) | regenerate `.sqlx/` for new queries | likely (Phase 1–3 used it) | matches sqlx 0.9 | run against a 0003-migrated DB |
| Prometheus + Grafana | scrape `/metrics`, render dashboard | operator's infra (out of band) | — | dashboard JSON is static; not needed to build/test the daemon |

**Missing dependencies with no fallback:** none identified — all are already in use from Phases 1–3.
**Missing dependencies with fallback:** Prometheus/Grafana are operator infrastructure, not build/test deps; OBS-05 ships a static JSON regardless.

## Sources

### Primary (HIGH confidence)
- crates.io API (crates.io/api/v1/crates/...) — verified 2026-06-15: axum 0.8.9, tower-http 0.6.11, tokio-util 0.7.18, humantime 2.3.0, humantime-serde 1.1.1, tracing 0.1.44, tracing-subscriber 0.3.23, metrics-exporter-prometheus 0.18.3, clap 4.6.1, hyper 1.10.1; dependency-requirement inspection (axum→hyper ^1.1/tower ^0.5; metrics-exporter-prometheus optional listener→hyper ^1.8) confirming no hyper version split.
- Codebase (read this session): `src/crawl/{mod,frontier,apply}.rs`, `src/relay/{mod,rate_limit}.rs`, `src/store/{mod,pubkeys,follows}.rs`, `src/lib.rs`, `migrations/0001_graph_schema.sql`, `migrations/0002_frontier.sql`, `Cargo.toml`, `.planning/phases/03-graph-writer-bfs-frontier/03-03-SUMMARY.md`, CONTEXT/REQUIREMENTS/STATE/ROADMAP — the existing `metrics::counter!` sites, the reclaim/claim queries, the churn writer, the `run_crawl` loop.

### Secondary (MEDIUM confidence)
- docs.rs/metrics-exporter-prometheus/0.18.3 (PrometheusBuilder: `install_recorder`, `set_buckets`, `set_buckets_for_metric`, `idle_timeout`, `upkeep_timeout`).
- docs.rs/config/0.15.23 (Environment: `with_prefix`/`prefix`, `separator`, `prefix_separator`; File + Environment layering with env overriding file).
- docs.rs/tracing-subscriber/0.3 (layered Registry + EnvFilter + fmt `.json()`), docs.rs/tokio (signal), docs.rs/tokio-util (CancellationToken) — standard, stable APIs.

### Tertiary (LOW confidence)
- `nostr_sdk::PublicKey::parse` hex+bech32 acceptance (A2) — training knowledge of the rust-nostr API; confirm exact method in planning.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — every version verified against crates.io 2026-06-15; all in CLAUDE.md table or CONTEXT-anticipated.
- Architecture: HIGH — derived from reading the actual Phase 3 code; the daemon reuses proven primitives, the metrics/axum/config/signal patterns are standard and doc-confirmed.
- Pitfalls: HIGH — Pitfalls 2/3/7 grounded directly in the existing schema/index and the existing two-phase drain; the rest are well-known tokio/metrics gotchas.
- FRESH-03 / migration 0003 finding: MEDIUM — the churn-columns-already-exist discovery is verified in code, but the CONTEXT/reality delta needs planner confirmation (A3).

**Research date:** 2026-06-15
**Valid until:** 2026-07-15 (stable Rust ecosystem; re-verify versions if planning slips past a month).
