# Phase 4: Daemon, Staleness Loop & Observability - Pattern Map

**Mapped:** 2026-06-15
**Files analyzed:** 17 (8 new, 9 modified/extended) + 5 test binaries
**Analogs found:** 12 / 17 (5 are net-new patterns with style-only analogs)

> **Framing (carried from RESEARCH):** This is a *wiring* phase, not a domain-logic
> phase. The two pieces of genuinely new SQL/Rust logic — the staleness `UPDATE`
> and the gauge-sampler aggregate queries — are near-verbatim copies of existing
> code. Everything else (axum server, config load, tracing init, metrics recorder,
> CancellationToken shutdown) is new-to-this-repo plumbing whose only in-repo
> analogs are *style* (module-doc conventions, error plumbing, `DEFAULT_*` const
> idiom). Those are flagged explicitly in **No Analog Found**.

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match Quality |
|-------------------|------|-----------|----------------|---------------|
| `src/crawl/frontier.rs` (MODIFY: add `reclaim_stale_by_ttl`, `reclaim_in_progress_older_than`) | store/frontier | batch (DB UPDATE) | `frontier.rs::reclaim_stale_on_startup` (lines 99-113) | exact (near-verbatim) |
| `migrations/0003_staleness.sql` (NEW) | migration | DDL | `migrations/0002_frontier.sql` | exact (same idiom) |
| `src/crawl/mod.rs` (MODIFY: `join_worker` → `pub(crate)`) | crawl | n/a | self (line 226) | exact |
| `src/daemon/loop_.rs` (NEW: `run_daemon_loop`) | crawl/driver | event-driven (cancellation) | `crawl/mod.rs::run_crawl` (lines 117-221) | role-match (reuses primitives, new loop control) |
| `src/daemon/config.rs` (NEW) | config | transform (deserialize+validate) | `store/mod.rs` const idiom + RESEARCH §Config | style-only (no config struct exists yet) |
| `src/daemon/observe.rs` (NEW: tracing init, recorder install, axum router) | provider/server | request-response (HTTP) | `relay/rate_limit.rs` metric sites (source) | style-only (no HTTP/server code exists) |
| `src/daemon/sampler.rs` (NEW: gauge sampler + progress summary) | service | batch (DB aggregate → gauge) | `frontier.rs` query idiom + follows.rs UPDATE | role-match (query style) / new emit |
| `src/daemon/mod.rs` (NEW: orchestrator, JoinSet, token wiring) | service/orchestrator | event-driven | `crawl/mod.rs::run_crawl` task spawn/join | role-match |
| `src/main.rs` (NEW: bin entry) | config/bootstrap | request-response | none (library-only today) | none |
| `src/lib.rs` (MODIFY: `pub mod daemon;`) | config | n/a | self (lines 9-13) | exact |
| `Cargo.toml` (MODIFY: deps + `[[bin]]`) | config | n/a | self | exact |
| `config.example.toml` (NEW) | config | n/a | none | none |
| `ops/grafana-dashboard.json` (NEW) | config | n/a | none | none |
| `tests/daemon_config.rs` (NEW) | test (no DB) | transform | `tests/migrations.rs` (pure assertions) | role-match |
| `tests/staleness.rs` (NEW) | test (DB) | batch | `tests/frontier.rs::startup_reclaims_in_progress` (line 236) | exact |
| `tests/daemon_loop.rs` (NEW) | test (DB+mock) | event-driven | `tests/frontier.rs` (`ScriptedGraph` + `run_crawl` tests) | exact (reuse harness) |
| `tests/observe.rs` (NEW) | test (in-proc axum) | request-response | none (no HTTP tests exist) | none (style: tests/migrations.rs) |
| `tests/graph_writer.rs` (MODIFY: add churn-on-change assertion) | test (DB) | CRUD | self (`same_event_zero_touch` line 122) | exact |

## Shared Conventions (apply to every new file)

These are repo-wide idioms verified across the analogs; the planner should hold
every new file to them.

1. **Module doc-comment header** mapping the file to its phase requirement IDs —
   every existing module opens with `//!` lines citing `CRAWL-0x`/`FRESH-0x`/etc.
   (`crawl/mod.rs:1-27`, `frontier.rs:1-13`, `store/mod.rs:1-10`). New daemon
   modules should cite `OPS-0x`/`OBS-0x`/`FRESH-02`.
2. **`DEFAULT_*` `pub const` with a doc-comment** tagged for config sourcing —
   `crawl/mod.rs:33-59`, `relay/rate_limit.rs:37-46`, `relay/fetch.rs:32`,
   `store/mod.rs:20-24`. The config struct defaults reference these by name (do
   NOT re-literal the numbers).
3. **All SQL via `sqlx::query!`/`query_scalar!` with `$N` binds — never string-
   formatted** (`store/mod.rs:9-11`, every query in `frontier.rs`/`follows.rs`).
   `.sqlx/` offline metadata is committed; run `cargo sqlx prepare` after adding
   queries.
4. **Errors propagate as the typed `StoreError`/`RelayError` enums**; the binary
   layer uses `anyhow` (already a dep) — see RESEARCH config example.
5. **Multi-statement writes use `pool.begin()` / `.execute(&mut *tx)` /
   `tx.commit()`** (`follows.rs:94-139`). Single-statement sweeps run directly on
   the pool (`frontier.rs:99-113`) — do NOT wrap the staleness UPDATE in a txn.

## Pattern Assignments

### `src/crawl/frontier.rs` — add `reclaim_stale_by_ttl` + `reclaim_in_progress_older_than` (frontier, batch UPDATE)

**Analog:** `src/crawl/frontier.rs::reclaim_stale_on_startup` (lines 99-113) — the
staleness scanner is a *parametrized copy* of this function. Same return type
(`Result<u64, StoreError>` via `result.rows_affected()`), same single-statement-on-
pool shape, same `claimed_at = NULL, fetch_attempts = 0` reset rationale.

**Core pattern to copy** (lines 99-113):
```rust
pub async fn reclaim_stale_on_startup(pool: &PgPool) -> Result<u64, StoreError> {
    let result = sqlx::query!(
        "UPDATE pubkeys SET status = 'discovered', claimed_at = NULL, fetch_attempts = 0 \
         WHERE status = 'in_progress'"
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}
```

**New `reclaim_stale_by_ttl`** — same body, change only the WHERE clause to key on
age (FRESH-02). The `$1::interval` bind is the TTL (RESEARCH Pattern 3):
```rust
// WHERE status IN ('fetched','not_found','failed')
//   AND last_fetched_at < (now() - $1::interval)
```
The `claimed_at = NULL, fetch_attempts = 0` reset is **load-bearing** — the comment
at `frontier.rs:101-105` explains why a re-fetch cycle must not inherit prior
relay-failure retry counts; carry that comment forward.

**New `reclaim_in_progress_older_than`** — same shape as `reclaim_stale_on_startup`
but `WHERE status='in_progress' AND claimed_at < now() - $1::interval` so the in-run
sweep never resets freshly-claimed live leases (RESEARCH Open Question 2). Note the
`int2`/interval cast idiom from `requeue_or_fail` (lines 142-160) for typed binds.

---

### `migrations/0003_staleness.sql` (migration, DDL)

**Analog:** `migrations/0002_frontier.sql` (whole file) — copy the additive/
idempotent conventions exactly.

**Conventions to copy:**
- File-header `--` doc block: idempotent + strictly additive, scope statement,
  what is left untouched (0002 lines 1-19).
- `CREATE INDEX IF NOT EXISTS` (0001 lines 40-45 show the existing partial-index
  idiom; the new index follows it).
- **Why a new index is needed** (RESEARCH Pitfall 2): the existing
  `pubkeys_status_idx` (0001:44-45) is partial `WHERE status IN
  ('discovered','not_found','failed')` — it *excludes* `fetched`, the bulk of the
  staleness re-enqueue population. Add e.g.
  `CREATE INDEX IF NOT EXISTS pubkeys_last_fetched_idx ON pubkeys (last_fetched_at)`
  (or a partial index over the three terminal statuses).
- **DO NOT add churn columns.** `last_changed_at`, `fetch_count`, `change_count`
  already exist (0001:24-26) and are already written by `apply_follow_list`
  (`follows.rs:120-135`). Migration 0003 is **index-only** unless the planner
  decides FRESH-03 needs a `refresh_count` distinct from `fetch_count` (RESEARCH
  A3 / Open Question 1 — flag to planner). This contradicts CONTEXT's literal
  "add new columns" wording.
- If any new column *were* added, use the `ALTER TABLE ... ADD COLUMN IF NOT
  EXISTS` + `COMMENT ON COLUMN ... INTERNAL` idiom (0002:31-32, 55-58) to keep it
  out of the public `pubkey_freshness` contract view.

---

### `src/daemon/loop_.rs` — `run_daemon_loop` (crawl driver, event-driven/cancellation)

**Analog:** `src/crawl/mod.rs::run_crawl` (lines 117-221). RESEARCH Pattern 2
Option A: a NEW function that **reuses the primitives** rather than refactoring
`run_crawl` (keeps its 5 green tests untouched).

**Generic-bound + signature pattern to copy** (lines 116-132) — the injected
`fetch_union` closure shape is identical (`Clone + Send + Sync + 'static`):
```rust
where
    F: Fn(Vec<ClaimedAuthor>) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<Vec<Event>, RelayError>> + Send + 'static,
```

**Startup + semaphore + spawn pattern to copy** (lines 133-193):
```rust
seed_anchor(pool, anchor_pubkey).await?;
let reclaimed = reclaim_stale_on_startup(pool).await?;   // KEPT (CONTEXT)
let sem = Arc::new(Semaphore::new(concurrency));
let mut workers: Vec<JoinHandle<Result<(), StoreError>>> = Vec::new();
// ... claim_batch -> acquire_owned permit BEFORE spawn (backpressure) -> spawn process_batch
let permit = Arc::clone(&sem).acquire_owned().await.expect("...never closed");
```

**Two-phase drain pattern to copy** (lines 152-160, 205-217) — this IS the OPS-02
graceful drain. On empty claim, join all in-flight workers via `join_worker`. On
cancel, stop claiming then drain so every leased row reaches a terminal status (no
orphaned `in_progress`).

**Only NEW control logic** (RESEARCH Pattern 2 skeleton): replace the
`if workers.is_empty() { break; }` termination with a `tokio::select!` on
`token.cancelled()` vs `tokio::time::sleep(poll_interval)` so the loop idle-polls
instead of exiting. Cancel at the **claim boundary only** — never wrap
`process_batch` in `select!` (RESEARCH Pitfall 4 — would abort an in-flight
`apply_follow_list` txn).

**Reuse `join_worker`** (`crawl/mod.rs:226-240`) — make it `pub(crate)` (one-word
library change, RESEARCH Open Question 4 / Pattern 2 note).

---

### `src/daemon/sampler.rs` — gauge sampler + progress summary (service, DB aggregate → gauge)

**Analog (query style):** `frontier.rs` `sqlx::query!` idiom; **emit style:**
`relay/rate_limit.rs:206/213` (`metrics::counter!(...).increment(1)`).

**Aggregate query** — a single `SELECT status, COUNT(*) ... GROUP BY status` (new,
but trivially the `query!`/`query_scalar!` idiom). Coverage = `fetched / total`.
Sample on a **coarse interval** (15-60s, configurable) — RESEARCH Pitfall 6 warns
against running the aggregate every second.

**Emit pattern** — mirror the existing counter sites but use `gauge!`/`histogram!`:
```rust
// existing (relay/rate_limit.rs:206):
metrics::counter!("relay_rate_limited", "relay" => relay_url.to_string()).increment(1);
// new sampler emits (NO per-pubkey labels — RESEARCH Pitfall 7):
// metrics::gauge!("frontier_depth").set(discovered_count as f64);
```

**Relay-health metric source:** read `RateLimiterRegistry::failure_count` /
`active_relay_count` (`rate_limit.rs:238-254`) — already public, built for exactly
this (`/// Exposed ... for ... observability`).

**Progress summary:** a `tokio::select!` interval task reading the same DB counts
and logging via `tracing::info!(...)` (OBS-04, RESEARCH Code Example "staleness +
in-run reclaim timers" shows the timer shape).

---

### `src/daemon/config.rs` (config, transform)

**Analog:** style-only. No config struct exists yet; `config` crate is a dep but
unused. **Follow RESEARCH §"Config struct + load + validate" verbatim** (the
`config::Config::builder().add_source(File).add_source(Environment.prefix("WOT").separator("__"))`
+ `humantime_serde` duration fields + `validate()` fail-fast pattern).

**Repo-idiom to apply:** every `#[serde(default = "...")]` default fn returns the
existing `DEFAULT_*` const (`crawl/mod.rs:39/49/59`, `rate_limit.rs:40`,
`fetch.rs:32`, `relay/mod.rs:39`) — do not re-literal. `store::MAX_CONNECTIONS`
(`store/mod.rs:24`) is private; if the pool size becomes configurable, that const
must be made `pub` or the value threaded into `connect`.

**Security:** never log `database_url` (`store/mod.rs:28-29` documents the
convention; RESEARCH Security Domain T-03-04).

---

### `tests/staleness.rs` (test, DB)

**Analog:** `tests/frontier.rs::startup_reclaims_in_progress` (line 236) — same
shape: `fresh_db()` helper (`frontier.rs:103-111`: `start_postgres` +
`store::connect` + `run_migrations`), seed rows at various `last_fetched_at`, call
`reclaim_stale_by_ttl`, assert only past-TTL rows flip and `fetch_attempts`/
`claimed_at` reset, fresh rows untouched.

**Harness to copy verbatim:** `fresh_db` (frontier.rs:103-111), `status_of`
helper (frontier.rs:114-120), `pk(seed)` (frontier.rs:94-99). Use
`-- --test-threads=2` (KNOWN FLAKE: testcontainers race).

---

### `tests/daemon_loop.rs` (test, DB + mock fetch)

**Analog:** `tests/frontier.rs` end-to-end tests (`bfs_reaches_full_component`
line 442, `crash_resume_no_redo` line 556). **Reuse the `ScriptedGraph` mock**
(frontier.rs:42-79) and its `fetch_fn` closure — the injected-`fetch_union` seam
is exactly what `run_daemon_loop` takes. `ScriptedGraph` is currently private to
`tests/frontier.rs`; make it shareable (move to `tests/common/mod.rs`) or
duplicate (RESEARCH Wave 0 note).

**Test seam (RESEARCH §Test Seams):** inject a `CancellationToken`, run the loop
with the mock + testcontainers pool, `token.cancel()` after a tick, assert clean
drain + zero `in_progress`. Do NOT test real signals.

---

### `tests/graph_writer.rs` — add churn-on-change assertion (test, CRUD)

**Analog:** `tests/graph_writer.rs::same_event_zero_touch` (line 122) — the
unchanged case (asserts `change_count` not bumped) already exists. ADD a
`churn_recorded_on_change` test that applies a *changed* follow list and asserts
`change_count`/`last_changed_at` bumped (FRESH-03). Reuse `fresh_db`
(graph_writer.rs:22), `validate_one` (line 37), `edge_count` (line 56).

---

### `src/main.rs`, `src/daemon/mod.rs`, `src/daemon/observe.rs`, `tests/observe.rs`

See **No Analog Found** — these are net-new (HTTP server, signal handling,
recorder install, in-process axum tests). Style anchors only: module-doc header
convention, `crawl/mod.rs` task spawn/`JoinHandle` join idiom for `mod.rs`'s
JoinSet, `tests/migrations.rs` for pure-assertion test structure.

## No Analog Found

Files / sub-patterns with no role+data-flow match in the codebase (the crate is
library-only with zero binary, HTTP, signal, or config-loading code today). The
planner should use **RESEARCH.md** patterns/code-examples as the source of truth
for these, not a codebase analog.

| File / Pattern | Role | Data Flow | Reason | Use Instead |
|----------------|------|-----------|--------|-------------|
| `src/main.rs` | bootstrap | request-response | No binary exists (`[[bin]]` is new); no `main` anywhere | RESEARCH §Architecture (bootstrap order) + Code Examples (signal→token) |
| `src/daemon/observe.rs` (axum router, `/metrics`, `/health/*`) | server | request-response | No HTTP server, no axum, no route handlers anywhere | RESEARCH Pattern 1 + Pattern 4 |
| tracing init (EnvFilter + fmt/JSON) | provider | n/a | No `tracing` subscriber init exists (crate emits no logs today) | RESEARCH Pattern 5 |
| Prometheus recorder install | provider | n/a | The 6 `counter!` sites emit into a no-op recorder; nothing installs one | RESEARCH Pattern 1 (`install_recorder()` ordering) |
| `CancellationToken` + signal wiring | service | event-driven | No graceful-shutdown / signal code exists | RESEARCH Code Example "Graceful shutdown wiring" |
| `config.example.toml` | config | n/a | No config file exists | CONTEXT field list + RESEARCH config struct |
| `ops/grafana-dashboard.json` | config | n/a | No `ops/` dir; static artifact | RESEARCH §Grafana / OBS-01 metric series |
| `tests/observe.rs` | test | request-response | No HTTP/axum tests exist | RESEARCH §Test Seams (`build_recorder()` not global install; `oneshot` via `tower::ServiceExt`) |

## Metadata

**Analog search scope:** `src/{crawl,store,relay,ingest}/`, `migrations/`,
`tests/`, `Cargo.toml`, `src/lib.rs`.
**Files scanned:** 43 source/test/sql/toml files (full repo, excl. `target/`).
**Files read in full or targeted:** `crawl/mod.rs`, `crawl/frontier.rs`,
`store/mod.rs`, `store/follows.rs` (90-142), `relay/rate_limit.rs`, `relay/mod.rs`
(100-260), migrations 0001/0002, `lib.rs`, `Cargo.toml`, `tests/common/mod.rs`,
`tests/frontier.rs` (42-120), ingest metric sites.
**Pattern extraction date:** 2026-06-15
