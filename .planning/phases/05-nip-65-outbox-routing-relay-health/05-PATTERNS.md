# Phase 5: NIP-65 Outbox Routing & Relay Health - Pattern Map

**Mapped:** 2026-06-15
**Files analyzed:** 18 new/modified
**Analogs found:** 16 with strong analog / 18 total (2 are genuinely-new patterns with style-only analogs)

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match Quality |
|-------------------|------|-----------|----------------|---------------|
| `migrations/0004_pubkey_relays.sql` | migration | CRUD | `migrations/0002_frontier.sql` | exact (additive/idempotent conventions) |
| `src/ingest/relay_list.rs` (NEW) | utility (transform) | transform | `src/ingest/mod.rs` (`ValidatedFollowList`, `timestamp_to_datetime`) | role-match (sibling type + extract fn) |
| `src/ingest/mod.rs` (modify: add `ValidatedRelayList`) | model | transform | `ValidatedFollowList` in same file (lines 45-87) | exact (same file, copy struct shape) |
| `src/store/relays.rs` (NEW: `apply_relay_list`, `lookup_write_relays`) | service (store) | CRUD | `src/store/follows.rs::apply_follow_list` | role-match (txn newest-wins replace) |
| `src/relay/health.rs` (NEW: `RelayHealthRegistry`) | service (registry) | event-driven | `src/relay/rate_limit.rs::RateLimiterRegistry` | role-match structure / **new pattern** (EWMA) |
| `src/relay/fetch.rs` (modify: health capture at Ok/Err arms) | service (relay) | request-response | self (`fetch_complete_with_timeout` lines 318-366) + `fetch_window_with_deadline` (Instant idiom, line 248) | exact (capture sites already present) |
| `src/relay/mod.rs` (modify: record rate-limit hit in `handle_relay_message`) | service (relay) | event-driven | self (`handle_relay_message` lines 252-263) | exact (one-line addition beside `record_notice`) |
| `src/crawl/apply.rs` (modify: `process_batch` + `fallback_fetch`, None arm) | controller | request-response | self (`process_batch` lines 107-200, None arm 193-195) | exact (extend existing seam) |
| `src/daemon/mod.rs` (modify: health-driven fan-out + per-relay Semaphore) | controller (daemon) | request-response | self (`fetch_union` closure lines 176-222) | exact / **new pattern** (routing + per-relay sem) |
| `src/daemon/loop_.rs` (read-only context: global Semaphore acquisition order) | controller (daemon) | request-response | self (global `Semaphore` line 98, permit acquire line 137) | exact (deadlock-order anchor) |
| `src/daemon/config.rs` (modify: 5 new fields + validate guards) | config | — | self (`Config` fields, default fns, `validate` lines 194-215) | exact (copy field + guard idiom) |
| `src/daemon/observe.rs` (modify: 3 new metric-name consts) | config | — | self (`METRIC_*` consts lines 53-73) | exact (add `pub const`s) |
| `src/daemon/sampler.rs` (modify: per-relay health gauge) | service (timer) | batch | self (`sample_gauges` lines 116-164, relay-health block 154-162) | exact (extend curated-relay loop) |
| `ops/grafana-dashboard.json` (modify: 3 panels) | config | — | self (relay-health timeseries panel; counter-rate exprs) | exact (copy panel shape) |
| `tests/common/mod.rs` (modify: relay-URL-aware + error-injecting `ScriptedGraph`) | test | event-driven | self (`ScriptedGraph` lines 150-188) | role-match / **new pattern** (URL-aware + Err injection) |
| `tests/relay_list.rs` (extend: r-tag extraction + `apply_relay_list` replace) | test | transform/CRUD | self (`newest_relay_list_wins` lines 14-43) | exact (extend existing file) |
| `tests/relay_health.rs` (NEW: EWMA/routing/permit) | test | event-driven | `tests/daemon_config.rs` (offline unit shape) + `rate_limit.rs` doc-tested invariants | role-match (offline unit) |
| `tests/nip65_fallback.rs` (NEW: recovery/miss/deadlock) | test | request-response | `tests/daemon_loop.rs` + `tests/common::fresh_db`/`ScriptedGraph` | role-match (testcontainers + scripted) |
| `tests/daemon_config.rs` (extend: 5 new field guards) | test | — | self (`*_zero_rejected` tests lines 153-193) | exact (copy the reject-test idiom) |

> **Migration-filename correction (verify against disk):** RESEARCH/CONTEXT refer to `0002_frontier.sql` / `0003_staleness.sql` — both correct on disk. But the init migration is `0001_graph_schema.sql` (NOT `0001_init.sql`). The new file is `migrations/0004_pubkey_relays.sql`.

---

## Pattern Assignments

### `migrations/0004_pubkey_relays.sql` (migration, additive/idempotent)

**Analog:** `migrations/0002_frontier.sql` (named-CHECK + `IF NOT EXISTS` + `COMMENT ON` conventions), `migrations/0003_staleness.sql` (index-only additive header).

**Idempotent additive header + CREATE IF NOT EXISTS** (0003 lines 1-23 / 0002 lines 1-9):
```sql
-- This migration is idempotent and strictly ADDITIVE: re-running it against an
-- already-migrated database is a no-op. ... sqlx also wraps each migration in a
-- transaction by default.
CREATE INDEX IF NOT EXISTS pubkeys_last_fetched_idx ON pubkeys (last_fetched_at);
```

**Named CHECK constraint** (copy the `marker` CHECK shape from 0002:25-27's explicit-name idiom):
```sql
ALTER TABLE pubkeys ADD CONSTRAINT pubkeys_status_check
    CHECK (status IN ('discovered','in_progress','fetched','not_found','failed'));
```
Apply as an inline named CHECK on the new table: `CONSTRAINT pubkey_relays_marker_check CHECK (marker IN ('read','write','both'))`.

**INTERNAL (not contract) COMMENT ON** (0002:55-58 — internal columns are documented `INTERNAL:`, never added to a contract view):
```sql
COMMENT ON COLUMN pubkeys.claimed_at IS
    'INTERNAL: timestamp this pubkey was leased into the in_progress frontier state; ...';
```
`pubkey_relays` is routing bookkeeping → `COMMENT ON TABLE pubkey_relays IS 'INTERNAL: ... NOT part of the public contract.'`; do NOT touch `pubkey_freshness` view (anti-pattern, RESEARCH Pitfall/Pattern 1). Final table/index/PK per RESEARCH Pattern 1 (`PRIMARY KEY (pubkey_id, url)` + `pubkey_relays_pubkey_idx`).

**`.sqlx` regen reminder:** after adding the 3 new queries, `cargo sqlx prepare -- --all-targets` and commit `.sqlx/` (RESEARCH Pitfall 6).

---

### `src/ingest/mod.rs` + `src/ingest/relay_list.rs` (model + transform)

**Analog:** `ValidatedFollowList` (mod.rs lines 45-87) for the companion struct; `nostr_sdk::nip65::extract_relay_list` for the r-tag transform (NEVER hand-roll — CLAUDE.md / RESEARCH Pattern 2).

**Companion struct shape** (mod.rs lines 45-87 — copy field doc style + `from_event` constructor; reuse `timestamp_to_datetime`):
```rust
#[derive(Debug, Clone)]
pub struct ValidatedFollowList {
    pub follower_pubkey: PublicKey,
    pub event_id: EventId,
    pub created_at: DateTime<Utc>,
    pub followee_pubkeys: Vec<PublicKey>,
}
// ...
pub fn from_event(event: &Event, followee_pubkeys: Vec<PublicKey>) -> Self {
    Self {
        follower_pubkey: event.pubkey,
        event_id: event.id,
        created_at: timestamp_to_datetime(event.created_at),
        followee_pubkeys,
    }
}
```
`ValidatedRelayList` mirrors this: `{ pubkey: PublicKey, event_id: EventId, created_at: DateTime<Utc>, relays: Vec<(String, &'static str)> }` (url + marker). Reuse `timestamp_to_datetime` (mod.rs lines 65-69) — do not re-derive the conversion.

**Where r-tags are currently dropped** (mod.rs lines 109-111 — the exact extension boundary):
```rust
/// kind:3 and kind:10002 identically — for kind:10002 the returned
/// [`ValidatedFollowList::followee_pubkeys`] are the relay-list p-tag pubkeys;
/// callers wanting the relay urls re-parse the winning event.
```
This is the documented seam: the winning kind:10002 event reaches the caller; the new `relay_list.rs` re-parses it via `nip65::extract_relay_list` (RESEARCH Pattern 2 — map `None → "both"`, `Read → "read"`, `Write → "write"`; normalize with `RelayUrl::as_str_without_trailing_slash()`).

**`pick_winner` is reused unchanged** (mod.rs lines 152-156 — kind-agnostic resolver already proven on kind:10002 by `tests/relay_list.rs`):
```rust
let Some(winner) = replaceable::pick_winner(group.iter(), now, future_clamp_secs) else {
    continue;
};
```

---

### `src/store/relays.rs` (NEW — service, CRUD)

**Analog:** `src/store/follows.rs::apply_follow_list` (transactional newest-wins write).

**Module header convention** (follows.rs lines 1-10 — phase-ID + requirement-tag doc header):
```rust
//! Transactional edge-diff writer (D-15).
//!
//! [`apply_follow_list`] applies a replacing kind-3 follow list as the diff ...
//! all in ONE transaction (RESEARCH Pattern 3, Pitfall 4 — a crash mid-diff must
//! never leave a half-applied follow list).
```

**Transactional DELETE-then-INSERT in one txn** (follows.rs lines 94-139 — the shape to copy; relay list uses full-replace not diff per RESEARCH Pattern 3 A2):
```rust
let mut tx = pool.begin().await?;
for &followee_id in &removed {
    sqlx::query!(
        "DELETE FROM follows WHERE follower_id = $1 AND followee_id = $2",
        follower_id, followee_id
    ).execute(&mut *tx).await?;
}
for &followee_id in &added {
    sqlx::query!(
        "INSERT INTO follows (follower_id, followee_id) VALUES ($1, $2) \
         ON CONFLICT DO NOTHING",
        follower_id, followee_id
    ).execute(&mut *tx).await?;
}
// ... UPDATE ...
tx.commit().await?;
```
`apply_relay_list`: `DELETE FROM pubkey_relays WHERE pubkey_id = $1` then loop-`INSERT ... ON CONFLICT (pubkey_id, url) DO NOTHING` then `tx.commit()` (RESEARCH Pattern 3). `lookup_write_relays`: `query_scalar!("SELECT url FROM pubkey_relays WHERE pubkey_id = $1 AND marker IN ('write','both')")` (Pitfall 2 — bare-r-tag-is-both).

**Conventions enforced:** `sqlx::query!`/`query_scalar!` with `$N` binds (follows.rs throughout); `Result<_, StoreError>` (error.rs:15 — `Sqlx(#[from] sqlx::Error)` gives `?` on `pool.begin()`/`execute`); `chrono::{DateTime, Utc}` for `seen_at` (follows.rs:14). Register `pub mod relays;` in `src/store/mod.rs` (currently lines 12-13: `pub mod follows; pub mod pubkeys;`).

---

### `src/relay/health.rs` (NEW — service registry) — **NEW PATTERN, style-only analog**

**Analog (structure only):** `src/relay/rate_limit.rs::RateLimiterRegistry`. The EWMA scoring math itself is genuinely new (RESEARCH Patterns 5-6); only the `Mutex<HashMap<String, _>>`-behind-`Arc` shape, the `record_*`/introspection method style, and the module header convention are copied.

**Module header + DEFAULT_* const convention** (rate_limit.rs lines 1-46):
```rust
//! Per-relay rate limiting + rate-limited-notice backoff (RELAY-04, RELAY-01).
//! ...
/// Default sustained per-relay outbound REQ rate (requests/second).
pub const DEFAULT_REQS_PER_SECOND: u32 = 4;
```
Add `pub const DEFAULT_HEALTH_ALPHA: f64`, etc. (so `config.rs` defaults reference the const by name, never re-literal — see config pattern below).

**`Mutex<HashMap>`-behind-`Arc` struct + clone-out-drop-lock discipline** (rate_limit.rs lines 131-187):
```rust
pub struct RateLimiterRegistry {
    // ...
    limiters: Mutex<HashMap<String, Arc<DirectLimiter>>>,
    failures: Mutex<HashMap<String, u32>>,
}
// acquire(): lock map, clone the Arc, DROP the lock, THEN await (NEVER hold the
// lock across .await):
let limiter = {
    let mut map = self.limiters.lock().expect("rate-limiter map not poisoned");
    Arc::clone(map.entry(relay_url.to_string()).or_insert_with(|| { /* ... */ }))
};
limiter.until_ready().await;
```
`RelayHealthRegistry` is **parallel, not an extension** (RESEARCH Pitfall 3 / discretion → parallel). Health updates are synchronous so no await-under-lock arises — but keep the discipline. Per-relay introspection methods mirror `failure_count`/`active_relay_count`/`has_limiter` (rate_limit.rs lines 238-264) for the sampler + tests: `score(relay) -> f64` (unknown = 1.0), `in_use(relay) -> u32`.

**Metric inside the registry** (rate_limit.rs lines 206-207 — labeled counter, NO manual `_total`):
```rust
metrics::counter!("relay_rate_limited", "relay" => relay_url.to_string()).increment(1);
```

---

### `src/relay/fetch.rs` (modify — health capture at Ok/Err arms)

**Analog:** self — `fetch_complete_with_timeout` (lines 318-366) is where the per-relay Ok/Err outcome lands; `fetch_window_with_deadline` (lines 238-254) already uses the `Instant::now()` latency idiom.

**Latency idiom already present** (fetch.rs lines 248-253):
```rust
let started = Instant::now();
let events = fetch(filter).await?;
if started.elapsed() >= timeout {
    return Err(RelayError::FetchTimeout(relay_url.to_string()));
}
```

**The Err/Ok classification site** — wrap the call (RESEARCH Code Examples) using the `RelayError` variants (error.rs:38-62):
```rust
match fetch_complete_with_timeout(/* ... */).await {
    Ok(events) => { health.record_success(relay_url, t0.elapsed()); Ok(events) }
    Err(RelayError::FetchTimeout(_)) => { health.record_timeout(relay_url); Err(/* .. */) }
    Err(RelayError::Client(_))       => { health.record_connect_failure(relay_url); Err(/* .. */) }
    Err(e)                           => { health.record_connect_failure(relay_url); Err(e) }
}
```
`RelayError::FetchTimeout(String)` (error.rs:61) is the explicit timeout; `RelayError::Client(#[from] nostr_sdk::client::Error)` (error.rs:42) wraps connect/subscribe/fetch → map to connect-failure (RESEARCH A4). Recommended threading: do the classification at the daemon fan-out call site (where `&health` is in scope) rather than widening `fetch_complete_with_timeout`'s signature, OR thread `Option<&RelayHealthRegistry>` like `registry` is threaded today.

---

### `src/relay/mod.rs` (modify — record rate-limit hit beside `record_notice`)

**Analog:** self — `handle_relay_message` (lines 252-263), the existing NOTICE routing site.

```rust
pub fn handle_relay_message(registry: &RateLimiterRegistry, relay_url: &str, message: &str) {
    match classify_notice(message) {
        NoticeKind::RateLimited | NoticeKind::Blocked => {
            let _ = registry.record_notice(relay_url, message);
        }
        NoticeKind::Other => {}
    }
}
```
Add `health.record_rate_limited(relay_url)` in the `RateLimited` arm. `spawn_notice_consumer` (lines 280-311) gains an `Arc<RelayHealthRegistry>` param threaded exactly as `registry` is (RESEARCH "Threading the new registry" — clone the `Arc` into the consumer). **Touch this file ONLY for this one line + the threaded param** (Pitfall 3 — do not entangle health into the limiter).

---

### `src/crawl/apply.rs` (modify — `process_batch` gains `fallback_fetch`; None arm)

**Analog:** self — `process_batch` (lines 107-200); the `None` arm (lines 193-195) is the exact RELAY-05 insertion point.

**The None arm today** (lines 191-195):
```rust
// Relays answered, no kind-3 for this author -> terminal not_found
// (D-10; set_fetch_status stamps last_fetched_at, FRESH-01).
None => {
    set_fetch_status(pool, claimed.id, "not_found", stamp).await?;
}
```

**Injected-closure seam to copy** (the existing `union_fetch` generic param, lines 107-119):
```rust
#[allow(clippy::too_many_arguments)]
pub async fn process_batch<F, Fut>(/* ... */ union_fetch: F) -> Result<usize, StoreError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Vec<Event>, crate::error::RelayError>>,
```
Add a second injected closure `fallback_fetch: Fn(PublicKey, Vec<String>) -> Fut2` (RESEARCH Pattern 4) so `apply.rs` never imports the live `Client` (no circular dep — same discipline as `union_fetch`). In the None arm: lookup write relays (`lookup_write_relays`), on-demand-resolve if empty (plain curated kind:10002 fetch — NOT routed back through fallback, Pitfall 4), sort by `health.score` desc + truncate to `nip65_max_write_relays`, call `fallback_fetch`, route the raw events through the SAME single-author `acquire_validated_lists` pass.

**Reuse `acquire_validated_lists` for single-author re-resolution** (already used at lines 136-144; same fn in relay/mod.rs lines 149-177) — write relays are adversarial, so verify/dedup/clamp must still run:
```rust
let result = acquire_validated_lists(
    &requested, want_kind, now_ts, future_clamp_secs, follow_cap, union_fetch,
).await;
```

**Counter on recovery** (sampler.rs:244 shows the no-manual-`_total` idiom):
```rust
metrics::counter!("nip65_recovered").increment(1); // exports as nip65_recovered_total (WR-02)
```

---

### `src/daemon/mod.rs` (modify — health-driven fan-out + per-relay Semaphore) — **NEW PATTERN**

**Analog:** self — the `fetch_union` closure (lines 176-222) is the only routing site; per-relay `Semaphore` admission + skip-below-threshold routing are genuinely new (RESEARCH Patterns 7-8).

**The current static uniform fan-out** (lines 201-218 — the loop to modify):
```rust
let mut union: Vec<nostr_sdk::Event> = Vec::new();
for relay_url in &relays {
    let max_limit = limit_cache.get_or_fetch(relay_url).await.max_limit;
    let events = fetch::fetch_complete_with_timeout(
        &client, relay_url, &authors, WANT_KIND, max_limit,
        MAX_AUTHORS_PER_REQ, fetch_timeout, &registry,
    ).await?;
    registry.reset(relay_url);
    union.extend(events);
}
```
Modify: before each relay, `if health.score(relay_url) < cfg.relay_health_threshold && !probe_due(relay_url) { continue; }` (Pattern 7); acquire the per-relay `Semaphore` permit between the global permit and the GCRA token (Pattern 8). On Ok → `health.record_success`; on Err → `health.record_timeout/connect_failure` (see fetch.rs pattern). NOTE: `?` on per-relay error currently aborts the whole union — preserve that requeue semantic, but record health first.

**Registry threading to copy** (lines 163-168 — `RateLimiterRegistry` built once, `Arc`, cloned into closure + notice consumer + sampler):
```rust
let registry = Arc::new(RateLimiterRegistry::with_params(
    reqs_per_second, DEFAULT_BACKOFF_BASE, DEFAULT_BACKOFF_CAP,
));
let _notice_consumer = spawn_notice_consumer(client.clone(), Arc::clone(&registry));
```
Build `let health = Arc::new(RelayHealthRegistry::new(cfg.health_alpha));` the same way; clone into the `fetch_union` closure, the notice consumer (rate-limit hit), and the sampler.

**Deadlock-safe acquisition order** (RESEARCH Pitfall 1) — anchor in `src/daemon/loop_.rs`:
- Global crawl `Semaphore::new(concurrency)` (loop_.rs line 98), permit acquired BEFORE spawn (loop_.rs lines 137-140):
```rust
let permit = Arc::clone(&sem).acquire_owned().await
    .expect("daemon crawl semaphore is never closed");
```
Fixed order EVERYWHERE: **global crawl permit (loop_.rs) → per-relay permit (fan-out) → GCRA token (`registry.acquire`) → fetch.** Never acquire a global permit while holding a per-relay permit. `tokio::Semaphore` cannot shrink (Pitfall 5): fixed `Semaphore::new(per_relay_concurrency)` per relay + an `in_use`-count gate against `max(1, round(per_relay_concurrency * score))` (Pattern 8).

---

### `src/daemon/config.rs` (modify — 5 new fields + validate guards)

**Analog:** self — `Config` fields (lines 48-99), the `default_*` fns referencing library consts by name (lines 101-137), the hand-impl `Debug` (lines 143-165), and `validate` (lines 194-215).

**Field + serde-default idiom** (lines 71-73):
```rust
/// Sustained per-relay outbound REQ rate (requests/second).
#[serde(default = "default_reqs_per_second")]
pub reqs_per_second: u32,
// ...
fn default_reqs_per_second() -> u32 { DEFAULT_REQS_PER_SECOND }
```
Add `nip65_fallback_enabled: bool`, `nip65_max_write_relays: usize`, `relay_health_threshold: f64`, `per_relay_concurrency: usize`, `health_alpha: f64` — each with a `#[serde(default = "...")]` fn referencing a `relay::health::DEFAULT_*` const by name (never re-literal — lines 101-102). **Add each new field to the hand-impl `Debug`** (lines 143-164) or the redaction test drifts.

**Fail-fast guards to copy** (lines 209-213):
```rust
anyhow::ensure!(c.concurrency > 0, "concurrency must be > 0");
anyhow::ensure!(c.batch_size > 0, "batch_size must be > 0");
anyhow::ensure!(c.reqs_per_second > 0, "reqs_per_second must be > 0");
```
Add: `nip65_max_write_relays > 0`, `per_relay_concurrency > 0`, `relay_health_threshold` in `[0,1]`, `health_alpha` in `(0,1]`. Document new fields in `config.example.toml` (the `example_config_is_valid` test in daemon_config.rs:133 will fail otherwise).

---

### `src/daemon/observe.rs` (modify — 3 new metric-name consts)

**Analog:** self — the `METRIC_*` `pub const` block (lines 53-73).

```rust
/// Gauge: max consecutive-failure count across the curated relay set (relay health).
pub const METRIC_RELAY_FAILURES: &str = "relay_consecutive_failures";
/// Gauge: number of relays with a live limiter (active relay count).
pub const METRIC_RELAY_ACTIVE: &str = "relay_active_count";
```
Add `METRIC_RELAY_HEALTH` (labeled gauge), `METRIC_NIP65_RECOVERED` (note: the counter itself is fired with the un-suffixed name `"nip65_recovered"` at the apply.rs site — the const is for the gauge/dashboard reference convention), `METRIC_RELAY_CONCURRENCY` (per-relay in-use gauge). The doc-comment block (lines 44-51) explicitly warns: aggregate gauges, NO per-pubkey labels (Pitfall 7) — see sampler caveat below.

---

### `src/daemon/sampler.rs` (modify — per-relay health gauge for curated relays only)

**Analog:** self — `sample_gauges` (lines 116-164); the relay-health block (lines 154-162) iterates the bounded curated relay set.

**Curated-relay iteration (bounded cardinality)** (lines 156-162):
```rust
metrics::gauge!(METRIC_RELAY_ACTIVE).set(registry.active_relay_count() as f64);
let max_failures = relays.iter().map(|r| registry.failure_count(r)).max().unwrap_or(0);
metrics::gauge!(METRIC_RELAY_FAILURES).set(max_failures as f64);
```
Add a labeled per-relay gauge ONLY over `relays` (the curated set, bounded ~tens) — NOT over transient write relays (Pitfall 7 / cardinality DoS):
```rust
for r in &relays {
    metrics::gauge!(METRIC_RELAY_HEALTH, "relay" => r.clone()).set(health.score(r));
}
```
`sample_gauges` gains an `Arc<RelayHealthRegistry>` param threaded exactly as `registry: Arc<RateLimiterRegistry>` is (mod.rs lines 270-279 spawn site). Coarse interval already enforced (Pitfall 6).

---

### `ops/grafana-dashboard.json` (modify — 3 panels)

**Analog:** self — the relay-health timeseries panel (the `relay_consecutive_failures`/`relay_active_count` panel) and the counter-rate exprs.

**Gauge panel expr** (no `_total`, matches the gauge code name): `"expr": "frontier_depth"`, `"expr": "relay_active_count"`.
**Counter panel expr** (DOES use `_total` — exporter appends it in exposition): `"expr": "sum(rate(relay_rate_limited_total[5m]))"`, `"expr": "sum(rate(ingest_unsolicited_total[5m]))"`.
Add panels: per-relay health gauge `relay_health` (labeled by `relay`); `nip65_recovered` counter as `sum(rate(nip65_recovered_total[5m]))` (the `_total` suffix in PromQL is correct; the code site fires `nip65_recovered`); per-relay concurrency-in-use gauge. Panel skeleton (timeseries):
```json
{ "type": "timeseries", "datasource": { "type": "prometheus", "uid": "${DS_PROMETHEUS}" },
  "gridPos": { "h": 8, "w": 12, "x": 0, "y": 0 },
  "targets": [ { "datasource": { "type": "prometheus", "uid": "${DS_PROMETHEUS}" },
    "expr": "relay_health", "legendFormat": "{{relay}}", "refId": "A" } ] }
```

---

### `tests/common/mod.rs` (modify — relay-URL-aware + error-injecting ScriptedGraph) — **NEW PATTERN**

**Analog:** self — `ScriptedGraph` (lines 150-188). The relay-URL-awareness + `Err` injection are genuinely new; the `Arc`-backed `Send + Clone` closure factory shape is copied.

**Current `Send`/`Clone`/`Arc` shape** (lines 150-188):
```rust
#[derive(Clone)]
pub struct ScriptedGraph { events: Arc<HashMap<Vec<u8>, Event>> }
// ...
pub fn fetch_fn(&self) -> impl Fn(Vec<ClaimedAuthor>) -> std::future::Ready<Result<Vec<Event>, RelayError>>
    + Clone + Send + Sync + 'static {
    let me = self.clone();
    move |batch: Vec<ClaimedAuthor>| std::future::ready(Ok(me.union_for(&batch)))
}
```
Extend (RESEARCH Wave 0): inner map `(relay_url, author) → Vec<Event>` (or `author → HashMap<relay_url, Vec<Event>>`) so a test models "author absent on curated relay A, present on write relay B"; add an error-injection variant returning `Err(RelayError::FetchTimeout(url))` / `Err(RelayError::Client(..))` for a designated relay. Keep `Send`/`Clone`/`Arc`-backed (it crosses `tokio::spawn`). `RelayError` is `web_of_trust::error::RelayError` (already imported, line 26).

**Add a fixture** `relay_list_event(author_seed, &[(url, marker)])` (sibling of `follows_event` lines 193-197), building a kind:10002 event with real r-tags via `EventBuilder::new(Kind::RelayList, "").tags([...])` (`signed_event` at lines 66-78 shows the EventBuilder+sign idiom).

---

### `tests/relay_list.rs` (extend) / `tests/relay_health.rs` (NEW) / `tests/nip65_fallback.rs` (NEW) / `tests/daemon_config.rs` (extend)

**`tests/relay_list.rs`** — analog: self (`newest_relay_list_wins` lines 14-43, offline `#[test]`). Extend with r-tag-extraction asserts (bare→both/read/write via `nip65::extract_relay_list`) + an `apply_relay_list` newest-wins-replace integration test (use `common::fresh_db`).

**`tests/relay_health.rs`** (NEW) — analog: `tests/daemon_config.rs` offline-unit shape + rate_limit.rs's pure-fn tests (`backoff_delay_unjittered`). Offline `#[test]` (no DB): EWMA moves with signals, skip-then-probe, permits scale with health.

**`tests/nip65_fallback.rs`** (NEW) — analog: `tests/daemon_loop.rs` + `common::{fresh_db, ScriptedGraph}`. testcontainers Postgres: not_found-on-curated→recovered-on-write-relay (`nip65_recovered_total`++), miss→terminal not_found, deadlock-free at `per_relay_concurrency=1`. Per-binary run `-- --test-threads=2`, re-run once on container flake.

**`tests/daemon_config.rs`** (extend) — analog: self, the `*_zero_rejected` idiom (lines 153-193):
```rust
let body = format!("{}\nconcurrency = 0\n", minimal_toml());
let cfg = load_config(&tmp.stem).expect("...deserializes");
let err = validate(&cfg).expect_err("concurrency = 0 must fail validation");
assert!(err.to_string().contains("concurrency"), "...got: {err}");
```
Add: `nip65_max_write_relays = 0`, `per_relay_concurrency = 0`, `relay_health_threshold` out of `[0,1]`, `health_alpha` out of `(0,1]`. NOTE: this suite mutates env so it runs `--test-threads=1`.

---

## Shared Patterns

### Registry threading (`Arc<...Registry>` cloned into fan-out + notice consumer + sampler)
**Source:** `src/daemon/mod.rs` lines 163-168, 270-279.
**Apply to:** `RelayHealthRegistry` — built once at daemon start behind `Arc`, cloned into the `fetch_union` closure (record + read), `spawn_notice_consumer` (rate-limit hit), and `sample_gauges` (labeled gauge for curated relays only). Exactly mirrors how `RateLimiterRegistry` is threaded.
```rust
let registry = Arc::new(RateLimiterRegistry::with_params(/* ... */));
let _notice_consumer = spawn_notice_consumer(client.clone(), Arc::clone(&registry));
```

### Mutex-map discipline (clone out, drop lock, then await; never hold lock across `.await`)
**Source:** `src/relay/rate_limit.rs::acquire` lines 166-187 (and the doc warning lines 167-176).
**Apply to:** `RelayHealthRegistry` methods. Health updates are synchronous so no await arises under the lock — but keep `*.lock().expect("health map")` scoped to the smallest block (RESEARCH anti-pattern: "Holding the health-map Mutex across an .await").

### Metric-name convention (NO manual `_total`; exporter appends it)
**Source:** `src/daemon/sampler.rs` lines 242-244; `src/relay/rate_limit.rs` lines 206-207; `src/daemon/observe.rs` doc block lines 44-51.
**Apply to:** `metrics::counter!("nip65_recovered")` (code) exports as `nip65_recovered_total` (Grafana PromQL uses `_total`). Gauges have no suffix (`relay_health`, `relay_active_count`). Labeled metrics ONLY for the bounded curated relay set (Pitfall 7).
```rust
metrics::counter!("staleness_reenqueued").increment(n); // -> staleness_reenqueued_total in exposition
```

### Injected-closure seam (keeps crawl layer free of the live `Client`; testable)
**Source:** `src/crawl/apply.rs::process_batch` `union_fetch: F where F: FnOnce() -> Fut` (lines 107-119).
**Apply to:** the new `fallback_fetch` param on `process_batch` — same `Fn`/`Future` injection so `ScriptedGraph` can supply scripted (or error-injecting) events and `apply.rs` never imports the relay `Client` (no circular dep).

### Additive/idempotent migration + INTERNAL-not-contract documentation
**Source:** `migrations/0002_frontier.sql` (named CHECK lines 25-27, `INTERNAL:` COMMENT lines 55-58), `migrations/0003_staleness.sql` (additive header lines 1-23).
**Apply to:** `0004_pubkey_relays.sql` — `CREATE TABLE/INDEX IF NOT EXISTS`, named `CHECK`, `COMMENT ON TABLE ... 'INTERNAL: ... NOT part of the public contract.'`; never add it to the `pubkey_freshness` contract view.

### Config field + library-const-backed default + fail-fast guard
**Source:** `src/daemon/config.rs` lines 71-73, 101-137 (default fns by name), 209-213 (numeric `ensure!`).
**Apply to:** all 5 new fields — `#[serde(default = "...")]` referencing a `relay::health::DEFAULT_*` const, add to hand-impl `Debug`, add an `anyhow::ensure!` guard in `validate`.

---

## No Analog Found

No file is fully analog-less. Two are **new patterns with style-only analogs** (flagged inline above):

| File | Role | Data Flow | New pattern (analog covers only style) |
|------|------|-----------|----------------------------------------|
| `src/relay/health.rs` | service registry | event-driven | EWMA scoring math (RESEARCH Patterns 5-6); `RateLimiterRegistry` supplies only the struct/method shape |
| `src/daemon/mod.rs` (fan-out changes) | controller | request-response | health-driven skip + periodic probe + per-relay `Semaphore` admission gate (Patterns 7-8); existing `fetch_union` is the host seam, not the pattern |
| `tests/common/mod.rs` (`ScriptedGraph` ext) | test | event-driven | relay-URL-aware map + `Err` injection; existing `ScriptedGraph` supplies only the `Send/Clone/Arc` closure shape |

For these, the planner should use RESEARCH Patterns 5-8 (EWMA math, routing, permit scaling) and the deadlock-ordering Pitfall 1 directly — the analogs only constrain *shape and conventions*, not the new logic.

## Metadata

**Analog search scope:** `src/store/`, `src/ingest/`, `src/relay/`, `src/crawl/`, `src/daemon/`, `migrations/`, `tests/`, `ops/`.
**Files scanned (read in full or targeted):** `src/store/follows.rs`, `src/ingest/mod.rs`, `src/relay/rate_limit.rs`, `src/relay/fetch.rs`, `src/relay/mod.rs` (NOTICE consumer + `acquire_validated_lists`), `src/crawl/apply.rs`, `src/daemon/mod.rs`, `src/daemon/loop_.rs`, `src/daemon/config.rs`, `src/daemon/observe.rs`, `src/daemon/sampler.rs`, `src/error.rs`, `migrations/0002_frontier.sql`, `migrations/0003_staleness.sql`, `tests/common/mod.rs`, `tests/relay_list.rs`, `tests/daemon_config.rs`, `ops/grafana-dashboard.json`.
**Pattern extraction date:** 2026-06-15
