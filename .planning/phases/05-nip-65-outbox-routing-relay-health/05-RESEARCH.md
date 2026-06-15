# Phase 5: NIP-65 Outbox Routing & Relay Health - Research

**Researched:** 2026-06-15
**Domain:** Nostr NIP-65 (kind:10002) relay-list extraction + outbox-style fallback routing, in-memory EWMA relay-health scoring, health-driven per-relay concurrency (Rust / nostr-sdk 0.44 / sqlx 0.9 / governor / metrics)
**Confidence:** HIGH (all APIs verified against the locked, already-resolved dependency tree on disk; all integration seams read from current source)

## Summary

Phase 5 is entirely *additive composition* over machinery Phases 2–4 already built and verified. Nothing in the locked stack is new: `nostr 0.44.3`, `nostr-sdk 0.44.1`, `sqlx 0.9.0`, `governor 0.10.4`, `metrics 0.24.6`, `tokio 1.52.3` are all already in `Cargo.lock` `[VERIFIED: Cargo.lock]`. There are **no new crates** to add and therefore **no package-legitimacy risk** in this phase.

The four locked work-streams map cleanly onto existing seams: (1) r-tag extraction reuses the unchanged INGEST-05 `pick_winner` winner and reads its `r`-tags via the *built-in* nostr NIP-65 helper `nostr::nip65::extract_relay_list` `[VERIFIED: nostr-0.44.3/src/nips/nip65.rs]` — there is no hand-rolled tag parsing to write; (2) the manual fallback inserts at the `crawl/apply.rs` `not_found` `None` arm (lines 186–194) behind an *injected* `fallback_fetch` closure so it stays ScriptedGraph-testable and avoids a circular dep; (3) `RelayHealthRegistry` parallels the existing `RateLimiterRegistry` (same `Mutex<HashMap<String,_>>`-behind-`Arc` shape) and is fed at the exact `Err`/`Ok` arms of `fetch_complete_with_timeout` plus the existing NOTICE consumer; (4) per-relay `Semaphore` and skip-below-threshold routing slot into the `daemon/mod.rs` fan-out, with a strict acquisition order (global → per-relay → GCRA) to stay deadlock-free.

**Primary recommendation:** Add migration `0004_pubkey_relays.sql` (additive, `IF NOT EXISTS`, named CHECK, mirroring 0002/0003 conventions) + `ValidatedRelayList` + `apply_relay_list` (transactional delete-not-in-set, copying `apply_follow_list`'s shape); use `nostr::nip65::extract_relay_list` for r-tags (never hand-parse); inject a `fallback_fetch: Fn(PublicKey, &[String]) -> Fut<Result<Vec<Event>, RelayError>>` into `process_batch` and resolve the single author via the existing `acquire_validated_lists`; build `RelayHealthRegistry` as a parallel in-memory registry with a continuous EWMA score in [0,1]; gate the fan-out on `score >= relay_health_threshold` with a periodic probe escape hatch; and scale per-relay `Semaphore` permits as `max(1, round(per_relay_concurrency * score))`. Make `ScriptedGraph` relay-URL-aware and error-injecting for the new tests.

## User Constraints (from CONTEXT.md)

### Locked Decisions

**kind:10002 Storage & Extraction (RELAY-05 prerequisite)**
- New migration `0004` adds a `pubkey_relays` table: `(pubkey_id BIGINT REFERENCES pubkeys(id), url TEXT, marker TEXT CHECK (marker IN ('read','write','both')), seen_at TIMESTAMPTZ)`, with newest-wins replacement per pubkey (a fresh winning kind:10002 replaces that pubkey's prior relay rows — delete-not-in-set, mirroring the follows edge-diff pattern). Additive/idempotent per the 0001/0002/0003 conventions; keep internal bookkeeping out of the public contract views unless it belongs there.
- Extend the ingest path: after `pick_winner` resolves a kind:10002 event, extract the `r`-tag relay URLs + read/write markers into a new `ValidatedRelayList` type (companion to `ValidatedFollowList`), and persist via a new store fn `apply_relay_list(pool, pubkey_id, &[(url, marker)])`. INGEST-05's replaceable/verify/dedup pipeline is reused unchanged; only the winning-event r-tag extraction + persistence is new.
- Acquisition timing: persist relay lists whenever a kind:10002 winner is seen. At the `not_found` hook, if a pubkey's write relays are not yet known, fetch its kind:10002 **on-demand** from the curated set first, extract+persist, then use the write relays. (Avoids a blanket dual-kind fetch on every batch; write relays are acquired exactly when needed and cached for reuse.)

**NIP-65 Fallback Routing (RELAY-05)**
- **Manual** fallback fetch using the existing `fetch_complete_with_timeout` path against the pubkey's NIP-65 write relays — NOT `nostr-sdk` `ClientOptions::gossip(true)`. Rationale: the whole acquisition stack (explicit pagination, NIP-11 limits, per-relay rate limiting, deterministic ScriptedGraph testing) is manual; gossip(true) would bypass that controllable, testable machinery.
- Trigger point: the `not_found` decision in `src/crawl/apply.rs` (the `None` arm, ~lines 186-194). Before stamping `not_found`, attempt the write-relay fallback; only stamp `not_found` if the fallback also yields nothing.
- Fan-out cap: configurable max write-relays tried per pubkey (default ~3) to bound fan-out; prefer healthier relays (per the health score) in selection; honor the existing per-relay rate limiting.
- Outcome: a fallback hit routes through `apply_validated` (status → `fetched`); still nothing → terminal `not_found`. Export a `nip65_recovered_total` counter so the operator sees fallback effectiveness (and it informs the curated-coverage concern carried from earlier phases).

**Relay Health Score (RELAY-06)**
- Track all four signals named in the success criterion, per relay: connect failures, timeouts, rate-limit hits, and response latency. Add per-relay fetch success counts too (the registry today only tracks NOTICE-driven failures).
- Score model: a continuous EWMA-based health score in [0,1] per relay — success rate penalized by latency and rate-limit hits (configurable EWMA alpha). Continuous (not discrete tiers) so routing/concurrency can scale smoothly.
- State: an in-memory `RelayHealthRegistry` (extends or parallels `RateLimiterRegistry`), rebuilt from live observation each daemon run. Not persisted across restarts — a multi-day daemon re-learns health quickly, and a `relay_health` table would add migration + write load for marginal benefit.
- Capture sites: the `Err` arms of `fetch_complete_with_timeout` (timeout / connect failure / client error), the success path (record latency + success), and the existing NOTICE consumer (rate-limit hits). These feed the registry.

**Health-Driven Routing & Per-Relay Concurrency (RELAY-06)**
- Routing effect: in the fan-out, skip relays whose health score is below a configurable threshold — BUT periodically probe skipped relays (a low-rate health probe) so a recovered relay can climb back into rotation. Avoids permanently blacklisting a relay after a transient outage.
- Per-relay concurrency: add a per-relay `Semaphore` (`HashMap<url, Arc<Semaphore>>`) whose permit count scales with the relay's health (healthy → more permits, degraded → fewer). Supplements the existing global crawl Semaphore + per-relay GCRA rate limiting.
- Config additions (with fail-fast validation in `validate()`): `nip65_fallback_enabled: bool`, `nip65_max_write_relays: usize` (>0), `relay_health_threshold: f64` (in [0,1]), `per_relay_concurrency: usize` (>0), and the health EWMA `alpha` (in (0,1]). Defaults chosen conservative/relay-polite.
- Observability: a per-relay health gauge (labeled by relay URL), the `nip65_recovered_total` counter, and per-relay concurrency-in-use; add corresponding Grafana panels to `ops/grafana-dashboard.json` (mind the exporter's `_total` counter suffix convention fixed in Phase 4).

### Claude's Discretion
- EWMA score formula details (the exact penalty weighting of latency/rate-limit vs success) — within the locked "success rate penalized by latency and rate-limit hits, configurable alpha" frame.
- Default values for the new config fields (locked as "conservative/relay-polite").
- Permit-count rounding rule and the probe cadence.
- Whether `RelayHealthRegistry` is a new struct or an extension of `RateLimiterRegistry` (locked as "extends or parallels"; this research recommends *parallel*, see Pitfall 3).
- Internal table/index naming for migration 0004; whether `pubkey_relays` appears in any contract view (locked: "unless it belongs there" — this research recommends NOT, see Pattern 1).
- Metric names for the new gauges/counter (consistent with the Phase 4 discretion that named the existing constants).

### Deferred Ideas (OUT OF SCOPE)
- NIP-77 negentropy bulk sync (RELAY-07, v2 — ~16% relay support today).
- Streaming live kind-3 subscriptions for near-real-time updates (RELAY-08, v2).
- Persisting relay health across daemon restarts (a `relay_health` table) — in-memory observation is sufficient for a multi-day daemon; revisit only if restart warm-up proves costly.
- nostr-sdk `gossip(true)` outbox routing — explicitly rejected in favor of the manual, testable fetch path.

## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| RELAY-05 | When a pubkey's kind 3 isn't found on curated relays, the crawler falls back to that pubkey's NIP-65 write relays | Migration 0004 + `pubkey_relays` (Pattern 1); `ValidatedRelayList` + `nip65::extract_relay_list` r-tag extraction (Pattern 2); `apply_relay_list` delete-not-in-set (Pattern 3); injected `fallback_fetch` closure at the `not_found` None arm reusing `acquire_validated_lists` (Pattern 4); `nip65_recovered_total` counter |
| RELAY-06 | Each relay carries a health score derived from connect failures, timeouts, rate-limit hits, response latency that drives routing and per-relay concurrency | `RelayHealthRegistry` parallel to `RateLimiterRegistry` (Pattern 5); EWMA score math (Pattern 6); capture sites at `fetch_complete_with_timeout` Err/Ok arms + NOTICE consumer; skip-below-threshold + periodic probe routing (Pattern 7); per-relay `Semaphore` permits scaled by score (Pattern 8); deadlock-safe acquisition ordering (Pitfall 1); per-relay health gauge + concurrency-in-use metrics |

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| kind:10002 r-tag extraction | Ingest (validation) | — | Reuses INGEST-05 winner; r-tag parsing is a per-event transform belonging with `ValidatedFollowList`'s sibling `ValidatedRelayList` |
| `pubkey_relays` persistence | Store (Postgres) | — | Newest-wins replace is a transactional write, identical responsibility class to `apply_follow_list` |
| NIP-65 fallback decision | Crawl (apply.rs) | Relay (fetch) | The decision ("is this author not_found? recover via write relays") lives at `process_batch`; the *fetch* is delegated to the relay layer through an injected closure |
| Health scoring | Relay (in-memory registry) | — | Health is per-relay observed transport behavior; belongs beside `RateLimiterRegistry` in `src/relay/` |
| Health-driven routing + per-relay concurrency | Daemon (fan-out) | Relay (registry read) | The fan-out in `daemon/mod.rs` owns relay selection + the global semaphore; per-relay semaphore + skip threshold are routing policy that reads the relay registry |
| Health/recovery metrics | Daemon (observe/sampler) | — | Consistent with Phase 4: gauges sampled on a coarse interval, counters fire-and-forget at the event site |

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| nostr / nostr-sdk | 0.44.3 / 0.44.1 | kind:10002 parsing, `nip65::extract_relay_list`, `RelayMetadata`, `RelayUrl` | Locked stack; the NIP-65 helper is built-in — no hand-rolled tag parsing `[VERIFIED: Cargo.lock; nostr-0.44.3/src/nips/nip65.rs]` |
| sqlx | 0.9.0 | `pubkey_relays` migration + `apply_relay_list` raw SQL, offline `.sqlx` metadata | Locked; same `query!`/`query_scalar!` `$N`-bind pattern as `apply_follow_list` `[VERIFIED: Cargo.lock]` |
| tokio | 1.52.3 | `Semaphore` for per-relay concurrency | Locked; `tokio::sync::Semaphore` already used for the global crawl cap `[VERIFIED: Cargo.lock]` |
| governor | 0.10.4 | Existing per-relay GCRA limiter (reused, not extended) | Locked; the health registry is *separate* from GCRA `[VERIFIED: Cargo.lock]` |
| metrics | 0.24.6 | Per-relay health gauge (labeled), `nip65_recovered_total`, concurrency-in-use | Locked; labeled-gauge pattern already in `relay/rate_limit.rs` `[VERIFIED: Cargo.lock]` |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| chrono | (transitive, already direct) | `DateTime<Utc>` for `seen_at` timestamp in `pubkey_relays` | Already used by `apply_follow_list`'s `created_at` bind `[VERIFIED: src/store/follows.rs]` |
| std `Instant` | std | Latency measurement around the fetch | `fetch_window_with_deadline` already uses `Instant::now()` `[VERIFIED: src/relay/fetch.rs]` |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| `nip65::extract_relay_list` | Manual `event.tags.iter()` + `TagStandard::RelayMetadata` match | The helper IS exactly that match, maintained upstream — no reason to inline it (CLAUDE.md: never hand-roll nostr parsing) |
| Parallel `RelayHealthRegistry` | Extend `RateLimiterRegistry` | Locked allows either; parallel keeps GCRA concerns separate and avoids widening the well-tested limiter API (Pitfall 3) |
| In-memory health | `relay_health` table | Explicitly deferred — multi-day daemon re-learns quickly; avoids migration + write load |

**Installation:** None. Every dependency is already present and resolved.
```bash
# No `cargo add`. Phase 5 introduces zero new crates.
```

**Version verification:** `Cargo.lock` confirms governor 0.10.4, metrics 0.24.6, nostr 0.44.3, nostr-sdk 0.44.1, sqlx 0.9.0, tokio 1.52.3 already resolved `[VERIFIED: Cargo.lock, 2026-06-15]`.

## Package Legitimacy Audit

> Not applicable — Phase 5 installs **zero** external packages. All functionality is built on the already-locked, already-resolved dependency tree (`Cargo.lock` verified). No `cargo add`, no new transitive deps.

**Packages removed due to [SLOP] verdict:** none
**Packages flagged as suspicious [SUS]:** none

## Project Constraints (from CLAUDE.md)

- **Never hand-roll nostr parsing/crypto** — use `nip65::extract_relay_list` for r-tags; reuse `verify::accept`/`pick_winner` unchanged.
- **Do NOT enable `nostr-sdk` `gossip(true)`** — outbox routing is manual via `fetch_complete_with_timeout` (explicitly in "What NOT to Use" and the deferred list).
- **PostgreSQL is the shared store; schema is the public contract** — migration 0004 must be additive/idempotent; keep internal bookkeeping out of contract views.
- **sqlx raw SQL as contract**, `$N` binds, bytea pubkeys as `Vec<u8>`, surrogate bigint ids; `.sqlx/` offline metadata committed + regenerated with `cargo sqlx prepare -- --all-targets`; build with `SQLX_OFFLINE=true`.
- **`tracing` + `metrics`** for observability; the exporter appends `_total` to counters — do NOT manually suffix counter names (Phase 4 WR-02).
- **GSD workflow** — all edits flow through the phase execution, not ad-hoc.

## Architecture Patterns

### System Architecture Diagram

```
                          claim_batch (discovered authors)
                                     │
                                     ▼
                    ┌──────────────────────────────────┐
   daemon fan-out   │  fetch_union(batch)               │   reads RelayHealthRegistry
   (daemon/mod.rs)  │  for relay in curated:            │◄──────────────┐
                    │    if score(relay) < threshold    │               │
                    │       && not probe-due → SKIP      │               │
                    │    acquire GLOBAL permit (loop_)   │   per-relay    │
                    │    acquire PER-RELAY permit ───────┼──► Semaphore   │
                    │    acquire GCRA token (governor)   │   map          │
                    │    t0 = Instant::now()             │               │
                    │    fetch_complete_with_timeout ────┼──► Ok→record latency+success
                    │                                    │    Err→record timeout/connect  ──► EWMA update
                    │    concat raw events (D-08 union)  │               │
                    └──────────────┬─────────────────────┘               │
                                   ▼                          NOTICE consumer (rate-limit hit)
                   acquire_validated_lists (ONE ingest pass)              │
                                   ▼                          ────────────┘
                   process_batch per-author resolution (crawl/apply.rs)
                       │                         │
                  hit (vfl)                 None arm (not_found candidate)
                       │                         │
                       ▼                         ▼  [if nip65_fallback_enabled]
                 apply_validated         write_relays = lookup_pubkey_relays(author)
                 (status=fetched)            │ empty? → on-demand fetch kind:10002
                                             │           from curated → extract_relay_list
                                             │           → apply_relay_list → re-read
                                             ▼
                                  fallback_fetch(author, write_relays[..cap, healthiest first])
                                             │
                                  ┌──────────┴──────────┐
                                hit                    miss
                                  │                     │
                       acquire_validated_lists    set_fetch_status("not_found")
                       → apply_validated
                       → nip65_recovered_total++

   kind:10002 winner (any time it is seen in a batch):
     pick_winner → extract_relay_list → ValidatedRelayList → apply_relay_list
        → DELETE pubkey_relays WHERE pubkey_id=$1; INSERT new (url,marker,seen_at)  [one txn]
```

### Recommended Project Structure
```
migrations/
└── 0004_pubkey_relays.sql      # additive: pubkey_relays table + per-pubkey index
src/
├── ingest/
│   ├── mod.rs                  # + ValidatedRelayList type (sibling of ValidatedFollowList)
│   └── relay_list.rs           # NEW: extract_relay_list → (url, marker) via nip65 helper
├── store/
│   └── relays.rs               # NEW: apply_relay_list (delete-not-in-set) + lookup_write_relays
├── relay/
│   ├── health.rs               # NEW: RelayHealthRegistry (EWMA), parallel to rate_limit.rs
│   ├── fetch.rs                # record health at Err/Ok arms of fetch_complete_with_timeout
│   └── mod.rs                  # record rate-limit hit in handle_relay_message
├── crawl/
│   └── apply.rs                # process_batch gains injected fallback_fetch; None arm calls it
└── daemon/
    ├── config.rs              # + 5 new fields + validate() guards
    ├── mod.rs                 # fan-out: skip-threshold + per-relay Semaphore + ordering
    ├── observe.rs             # + METRIC_RELAY_HEALTH, METRIC_NIP65_RECOVERED, METRIC_RELAY_CONCURRENCY consts
    └── sampler.rs             # emit per-relay health gauge (labeled) on coarse interval
ops/grafana-dashboard.json     # + panels for health gauge, nip65_recovered, per-relay concurrency
tests/
├── common/mod.rs              # ScriptedGraph → relay-URL-aware + error-injecting
├── relay_list.rs             # extend: r-tag extraction asserts (currently only pick_winner)
├── relay_health.rs           # NEW: EWMA math + routing/permit-scaling
└── nip65_fallback.rs         # NEW: not_found-on-curated → recovered-on-write-relay
```

### Pattern 1: Migration 0004 — `pubkey_relays` table (newest-wins replace)
**What:** Additive, idempotent migration mirroring 0002/0003 conventions exactly.
**When to use:** RELAY-05 prerequisite storage.
**Example:**
```sql
-- Source: mirrors migrations/0002_frontier.sql / 0003_staleness.sql conventions
-- Phase 5: NIP-65 (kind:10002) advertised relay storage (RELAY-05).
-- Additive + idempotent: CREATE ... IF NOT EXISTS, named CHECK. sqlx wraps each
-- migration in a transaction. NOT part of the public contract (internal routing
-- bookkeeping); deliberately absent from the contract views (GRAPH-04).
CREATE TABLE IF NOT EXISTS pubkey_relays (
    pubkey_id BIGINT      NOT NULL REFERENCES pubkeys(id),
    url       TEXT        NOT NULL,
    marker    TEXT        NOT NULL
              CONSTRAINT pubkey_relays_marker_check
              CHECK (marker IN ('read','write','both')),
    seen_at   TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (pubkey_id, url)
);
-- Per-pubkey lookup for the fallback path ("write relays for author X").
CREATE INDEX IF NOT EXISTS pubkey_relays_pubkey_idx ON pubkey_relays (pubkey_id);
COMMENT ON TABLE pubkey_relays IS
    'INTERNAL: per-pubkey NIP-65 (kind:10002) advertised relays for outbox-style fallback routing (RELAY-05). Newest-wins replaced per pubkey on each winning kind:10002. NOT part of the public contract.';
```
- `[CITED: migrations/0002_frontier.sql]` for the named-CHECK / `IF NOT EXISTS` / `COMMENT ON` idempotency conventions.
- Decision needed: PK `(pubkey_id, url)` makes the delete-not-in-set a clean per-pubkey replace. The marker stores `'both'` for a bare `r`-tag (NIP-65 default), see Pattern 2.

### Pattern 2: r-tag extraction via the built-in NIP-65 helper
**What:** Extract `(url, marker)` pairs from the winning kind:10002 event.
**When to use:** Whenever a kind:10002 winner is resolved (and on the on-demand fallback fetch).
**Example:**
```rust
// Source: nostr-0.44.3/src/nips/nip65.rs (re-exported as nostr_sdk::nip65)
use nostr_sdk::nip65::{self, RelayMetadata};

/// Marker as stored in pubkey_relays.marker ('read' | 'write' | 'both').
/// NIP-65: a bare `r` tag (no read/write token) means BOTH read and write.
fn marker_of(meta: &Option<RelayMetadata>) -> &'static str {
    match meta {
        Some(RelayMetadata::Read)  => "read",
        Some(RelayMetadata::Write) => "write",
        None                       => "both",   // bare r-tag = read+write
    }
}

// `extract_relay_list(&event)` yields (&RelayUrl, &Option<RelayMetadata>).
// RelayUrl → String via as_str_without_trailing_slash() (matches the relay_url
// string keys used everywhere else in the relay layer).
let pairs: Vec<(String, &'static str)> = nip65::extract_relay_list(&winner)
    .map(|(url, meta)| (url.as_str_without_trailing_slash().to_string(), marker_of(meta)))
    .collect();
```
- `[VERIFIED: nostr-0.44.3/src/nips/nip65.rs]` — `extract_relay_list(event) -> impl Iterator<Item = (&RelayUrl, &Option<RelayMetadata>)>`; `RelayMetadata` is `Read | Write` only; `None` = both.
- `[VERIFIED: nostr-0.44.3/src/types/url.rs]` — `RelayUrl::as_str_without_trailing_slash()` + `Display`.
- **WRITE relays for outbox fetch = `write` OR `both`** (a bare r-tag advertises both). The fallback selects markers in `('write','both')`.

### Pattern 3: `apply_relay_list` — transactional newest-wins replace
**What:** Delete the pubkey's prior relay rows, insert the new set, in one transaction — mirroring `apply_follow_list`'s edge-diff shape (here a full replace is simpler than a diff because the row count is tiny).
**When to use:** Persisting a `ValidatedRelayList`.
**Example:**
```rust
// Source: mirrors src/store/follows.rs::apply_follow_list transaction shape
pub async fn apply_relay_list(
    pool: &PgPool,
    pubkey_id: i64,
    relays: &[(String, &str)],      // (url, marker)
    seen_at: DateTime<Utc>,
) -> Result<(), StoreError> {
    let mut tx = pool.begin().await?;
    // Newest-wins: drop the pubkey's prior rows wholesale, then insert the winner's.
    sqlx::query!("DELETE FROM pubkey_relays WHERE pubkey_id = $1", pubkey_id)
        .execute(&mut *tx).await?;
    for (url, marker) in relays {
        sqlx::query!(
            "INSERT INTO pubkey_relays (pubkey_id, url, marker, seen_at) \
             VALUES ($1, $2, $3, $4) ON CONFLICT (pubkey_id, url) DO NOTHING",
            pubkey_id, url, *marker, seen_at
        ).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

// And the fallback read:
pub async fn lookup_write_relays(pool: &PgPool, pubkey_id: i64) -> Result<Vec<String>, StoreError> {
    let rows = sqlx::query_scalar!(
        "SELECT url FROM pubkey_relays WHERE pubkey_id = $1 AND marker IN ('write','both')",
        pubkey_id
    ).fetch_all(pool).await?;
    Ok(rows)
}
```
- Full-replace (not a diff) is correct here: relay lists are a handful of rows; the GRAPH-02 idempotency concern that motivated the kind-3 diff (touch-zero-rows-on-unchanged) does not apply to this tiny, non-hot table. If a no-op-on-unchanged guard is wanted, gate on the winning event id (out of scope unless trivially cheap).
- `.sqlx` offline metadata must be regenerated for the 3 new queries: `cargo sqlx prepare -- --all-targets` (lib-only prunes integration metadata — see Phase 4 deviation in STATE).

### Pattern 4: Injected `fallback_fetch` closure at the `not_found` arm (no circular dep)
**What:** `process_batch` gains a second injected closure so the per-pubkey, per-write-relay fallback fetch stays testable and the crawl layer never depends on the relay-client concretely.
**When to use:** RELAY-05 fallback.
**Example:**
```rust
// Source: extends src/crawl/apply.rs::process_batch (current union_fetch seam)
// New parameter shape — by author + its write relays, returning the raw union:
//   fallback_fetch: Fn(PublicKey, Vec<String>) -> Fut<Result<Vec<Event>, RelayError>>
//
// In the None arm (currently apply.rs:193-195):
None => {
    let recovered = if fallback_enabled {
        // 1. Resolve write relays; if unknown, on-demand fetch+persist kind:10002
        //    from the curated set first (this itself reuses acquire_validated_lists
        //    with want_kind = Kind::RelayList, then apply_relay_list).
        let mut write_relays = lookup_write_relays(pool, claimed.id).await?;
        if write_relays.is_empty() {
            write_relays = resolve_relays_on_demand(pool, author, /* curated fetch */).await?;
        }
        // 2. Prefer healthier relays, cap to nip65_max_write_relays.
        write_relays.sort_by(|a, b| health.score(b).total_cmp(&health.score(a)));
        write_relays.truncate(nip65_max_write_relays);
        // 3. Fetch kind:3 from the write relays; route the raw union through the
        //    SAME single-author acquire_validated_lists pass.
        let raw = fallback_fetch(author, write_relays).await;
        match raw {
            Ok(events) => {
                let one = HashSet::from([author]);
                acquire_validated_lists(&one, want_kind, now, future_clamp_secs, follow_cap, || std::future::ready(Ok(events))).await.ok()
                    .and_then(|mut v| v.pop())
            }
            Err(_) => None,
        }
    } else { None };

    match recovered {
        Some(vfl) => { apply_validated(pool, &vfl).await?; metrics::counter!("nip65_recovered").increment(1); }
        None      => { set_fetch_status(pool, claimed.id, "not_found", stamp).await?; }
    }
}
```
- The closure boundary keeps `process_batch` testable with `ScriptedGraph` (inject a closure that returns scripted events for a write-relay URL) and avoids importing the live `Client` into `crawl/apply.rs` (no circular dep — same discipline as the existing `union_fetch`).
- `nip65_recovered` counter (NO manual `_total`; exporter appends it — WR-02). Exports as `nip65_recovered_total`.
- Reuse `acquire_validated_lists` for the single-author re-resolution so verify/dedup/newest-wins/follow-cap all still apply to fallback events (a write relay is just as adversarial as a curated one).

### Pattern 5: `RelayHealthRegistry` — parallel to `RateLimiterRegistry`
**What:** New in-memory registry, same `Mutex<HashMap<String, _>>`-behind-`Arc` shape; one EWMA score per relay url.
**When to use:** RELAY-06 state.
**Example:**
```rust
// Source: mirrors src/relay/rate_limit.rs::RateLimiterRegistry structure
pub struct RelayHealthRegistry {
    alpha: f64,                              // EWMA smoothing in (0,1]
    scores: Mutex<HashMap<String, f64>>,     // current health in [0,1], default new=1.0
    in_use: Mutex<HashMap<String, u32>>,     // per-relay concurrency-in-use gauge source
}
impl RelayHealthRegistry {
    pub fn record_success(&self, relay: &str, latency: Duration) { self.update(relay, success_sample(latency)); }
    pub fn record_timeout(&self, relay: &str)        { self.update(relay, 0.0); }
    pub fn record_connect_failure(&self, relay: &str){ self.update(relay, 0.0); }
    pub fn record_rate_limited(&self, relay: &str)   { self.update(relay, 0.2); } // penalize but not zero
    pub fn score(&self, relay: &str) -> f64 {
        *self.scores.lock().expect("health map").get(relay).unwrap_or(&1.0)  // unknown = healthy
    }
    fn update(&self, relay: &str, sample: f64) {
        let mut m = self.scores.lock().expect("health map");
        let prev = m.get(relay).copied().unwrap_or(1.0);
        m.insert(relay.to_string(), self.alpha * sample + (1.0 - self.alpha) * prev);
    }
}
```
- Built once at daemon start, wrapped in `Arc`, cloned into the fan-out + the NOTICE consumer + the sampler — exactly how `RateLimiterRegistry` is threaded today `[VERIFIED: src/daemon/mod.rs:163-168]`.
- Unknown relay scores 1.0 (healthy by default) so a never-seen relay is not skipped before it has a chance.

### Pattern 6: EWMA score math
**What:** A simple, monotone, in-[0,1] update where success raises and failure lowers the score.
- **Per-event sample** in [0,1]:
  - success: `sample = latency_factor`, e.g. `latency_factor = 1.0 / (1.0 + latency_secs / latency_scale)` (latency_scale ≈ a few seconds) so a fast success ≈ 1.0 and a slow-but-successful fetch is penalized toward 0.5.
  - timeout / connect failure: `sample = 0.0`.
  - rate-limited notice: `sample = 0.2` (degrade, do not zero — a rate-limited relay is still usable, mirroring the existing `RateLimited` vs `Blocked` split `[VERIFIED: src/relay/rate_limit.rs]`).
- **EWMA update:** `score = alpha * sample + (1 - alpha) * score_prev`. Smaller `alpha` = slower to react (more memory); locked as configurable, default conservative (recommend `alpha ≈ 0.3`).
- **Maps to routing:** `route(relay) = score(relay) >= relay_health_threshold || probe_due(relay)`.
- **Maps to permits:** `permits(relay) = max(1, round(per_relay_concurrency * score(relay)))` — min 1 so even a degraded relay keeps a single probe slot.

### Pattern 7: Skip-below-threshold with periodic probe
**What:** Degraded relays are skipped in the fan-out but periodically allowed one probe request so a recovered relay climbs back.
**Example:**
```rust
// In the fan-out loop, per relay:
let healthy = health.score(relay) >= cfg.relay_health_threshold;
let probe_due = last_probe(relay).elapsed() >= PROBE_INTERVAL;   // e.g. 60s
if !healthy && !probe_due { continue; }   // skip — steer traffic away
// else proceed (a probe success will raise the EWMA and re-admit the relay)
```
- Probe bookkeeping can live in the `RelayHealthRegistry` (a `last_probe: Mutex<HashMap<String, Instant>>`) or be derived from "we attempted this relay at time T". Keep it in the registry for testability.

### Pattern 8: Per-relay `Semaphore` integration
**What:** A `HashMap<String, Arc<Semaphore>>` whose permit counts scale with health, layered between the global crawl semaphore and the GCRA token.
- Resize on health change is awkward with `tokio::Semaphore` (it has `add_permits` but no "remove"). **Recommended approach:** at the point of acquiring a per-relay permit, read `permits(relay)` and use `Semaphore::try_acquire_many`/a fixed-size semaphore sized at `per_relay_concurrency`, then *gate admission* on the health-scaled count by counting in-use against the scaled target (the `in_use` map in Pattern 5). Simpler equivalent: keep a fixed `Semaphore::new(per_relay_concurrency)` per relay and additionally refuse to spawn when `in_use(relay) >= permits(relay)`. This avoids the `Semaphore`-shrink problem entirely while still scaling effective concurrency by health.
- Expose `in_use(relay)` as the per-relay concurrency-in-use gauge.

### Anti-Patterns to Avoid
- **`gossip(true)`** — explicitly forbidden; bypasses pagination/NIP-11/GCRA/test seams.
- **Hand-rolling r-tag parsing** — `nip65::extract_relay_list` exists; CLAUDE.md forbids reimplementing nostr parsing.
- **Putting `pubkey_relays` in a contract view** — it is internal routing state, not part of the spam-layer contract (GRAPH-04).
- **Resizing a `tokio::Semaphore` down** — there is no remove-permit API; use the in-use-count gate instead (Pattern 8).
- **Recording health inside the GCRA limiter** — keep health and rate-limiting as separate registries (Pitfall 3).
- **Holding the health-map `Mutex` across an `.await`** — same lesson as `RateLimiterRegistry::acquire` (clone out, drop lock, then await); the health updates are synchronous so this is naturally safe, but do not introduce an awaiting path under the lock.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| kind:10002 r-tag parsing | Manual `tag.as_vec()[0]=="r"` matching | `nostr_sdk::nip65::extract_relay_list` | Built-in, handles `TagStandard::RelayMetadata` + bare-tag-means-both correctly `[VERIFIED]` |
| Relay URL normalization | String trimming | `RelayUrl::as_str_without_trailing_slash()` | Canonical form matching the relay_url keys used by the GCRA limiter `[VERIFIED]` |
| Replaceable resolution for kind:10002 | New resolver | `pick_winner` (kind-agnostic, already proven on kind:10002 by `tests/relay_list.rs`) | INGEST-05 reuse, zero new validation logic |
| Newest-wins persistence | New diff algorithm | Copy `apply_follow_list`'s txn shape (delete + insert in one tx) | Proven transactional pattern; relay list is small so full-replace is fine |
| Single-author fallback validation | Bypass ingest | `acquire_validated_lists` with `requested = {author}` | Write relays are adversarial too — must verify/dedup/clamp |
| Per-relay rate limiting in fallback | New limiter | Existing `RateLimiterRegistry` via `fetch_complete_with_timeout` | The fallback fetch goes through the same gated path |

**Key insight:** Phase 5 is a *wiring* phase. Every hard primitive (parsing, crypto, replaceable resolution, transactional writes, rate limiting, pagination, the fan-out, the fetch path) already exists and is tested. The new code is: one migration, two small store fns, one extraction fn, one in-memory registry, and the closure/threading to connect them.

## Runtime State Inventory

> Phase 5 is greenfield-additive (new table, new in-memory state, new code paths). It is NOT a rename/refactor/migration of existing state. The closest concern is the *new* `pubkey_relays` table, which starts empty and is populated by live observation — there is no pre-existing data to migrate.

| Category | Items Found | Action Required |
|----------|-------------|------------------|
| Stored data | None — `pubkey_relays` is brand new and starts empty; existing `pubkeys`/`follows` rows are untouched. r-tag URLs were previously *dropped* at ingest (`ingest/mod.rs` comment), so there is no historical relay data to backfill. | None (new table populated going forward) |
| Live service config | None — no external service holds Phase-5 state. | None |
| OS-registered state | None. | None |
| Secrets/env vars | New config fields (`nip65_fallback_enabled`, etc.) are added to the TOML/`WOT__*` overlay; no secrets. | Document in `config.example.toml` |
| Build artifacts | `.sqlx/` offline metadata must be regenerated for the 3 new queries (`apply_relay_list` DELETE/INSERT, `lookup_write_relays`). | `cargo sqlx prepare -- --all-targets` after adding queries |

**Nothing found in category:** Stored-data backfill, live-service, OS-state, and secrets are all "None — verified by reading the existing schema (migrations 0001–0003) and config.rs; the only persisted addition is the empty new table."

## Common Pitfalls

### Pitfall 1: Deadlock from inconsistent lock/permit acquisition order
**What goes wrong:** Three gates now stack — the global crawl `Semaphore` (`daemon/loop_.rs:98`), the new per-relay `Semaphore`, and the per-relay GCRA token (`RateLimiterRegistry::acquire`). Acquiring them in different orders across paths can deadlock.
**Why it happens:** Two tasks each holding one resource and waiting on the other.
**How to avoid:** Fix ONE global order everywhere: **global crawl permit → per-relay permit → GCRA token → fetch.** The global permit is already acquired in `loop_.rs` before `process_batch` is spawned `[VERIFIED: src/daemon/loop_.rs:137]`; the per-relay permit and GCRA token are both acquired *inside* the per-relay fan-out iteration, so order them per-relay-permit-then-GCRA there. Never acquire a global permit while holding a per-relay permit.
**Warning signs:** A test that fans out to multiple relays with `per_relay_concurrency=1` hangs.

### Pitfall 2: `marker IN ('write','both')` vs treating bare r-tag as write-only
**What goes wrong:** A bare `r`-tag (no read/write token) is stored as `'read'` or dropped, so write relays are missed and fallback never fires.
**Why it happens:** NIP-65 says a bare r-tag means BOTH read and write; `extract_relay_list` returns `None` for that case, which is easy to misread as "no marker → skip."
**How to avoid:** Map `None → 'both'` (Pattern 2) and select write relays with `marker IN ('write','both')`.
**Warning signs:** Pubkeys with bare-r-tag kind:10002 events are never recovered.

### Pitfall 3: Coupling health into the GCRA limiter
**What goes wrong:** Extending `RateLimiterRegistry` with health state widens a well-tested struct, risks regressing the CR-05 shared-`Arc<DirectLimiter>` invariant, and entangles two independent concerns.
**Why it happens:** The locked text says "extends or parallels."
**How to avoid:** Build a **parallel** `RelayHealthRegistry` (Pattern 5). The existing limiter already exposes `failure_count`/`active_relay_count` for the sampler; health is a richer, separate signal.
**Warning signs:** Touching `rate_limit.rs` more than the one line in `record_notice` that also pings health.

### Pitfall 4: On-demand kind:10002 fetch infinite-recursing into the fallback
**What goes wrong:** The on-demand relay-list fetch at the `not_found` arm is itself a fetch that could miss and try to fall back again.
**Why it happens:** Reusing the same `process_batch`/fallback path for the kind:10002 fetch.
**How to avoid:** The on-demand kind:10002 fetch is a *plain* curated fetch (`acquire_validated_lists` with `want_kind = Kind::RelayList`), NOT routed through the kind-3 fallback. It either yields write relays (persist + use) or yields nothing (then stamp `not_found`). No recursion.
**Warning signs:** Stack growth / repeated kind:10002 REQs for one author.

### Pitfall 5: `tokio::Semaphore` cannot shrink
**What goes wrong:** Trying to lower a relay's permit count when its health drops panics or is impossible (`Semaphore` has `add_permits` but no remove).
**Why it happens:** Assuming the semaphore size itself tracks health.
**How to avoid:** Fixed-size `Semaphore::new(per_relay_concurrency)` per relay + an `in_use`-count admission gate against the health-scaled target (Pattern 8).
**Warning signs:** No safe API found for reducing permits.

### Pitfall 6: Forgetting `.sqlx` regeneration / the `--all-targets` flag
**What goes wrong:** Offline build (`SQLX_OFFLINE=true`) fails in CI because the 3 new queries have no cached metadata, or integration-test query metadata is pruned.
**Why it happens:** `cargo sqlx prepare` without `-- --all-targets` only captures lib queries (Phase 4 deviation, recorded in STATE).
**How to avoid:** `cargo sqlx prepare -- --all-targets` and commit `.sqlx/`.
**Warning signs:** Green local build, red offline/CI build.

### Pitfall 7: Labeled per-relay health gauge cardinality
**What goes wrong:** A per-relay-URL-labeled gauge is fine for a curated set of ~tens of relays, but if fallback write-relay URLs (unbounded, from arbitrary kind:10002 events) are also labeled, cardinality explodes (Phase 4 Pitfall 7 / T-04-06).
**Why it happens:** Recording health for every write relay ever fetched and exporting it labeled.
**How to avoid:** Only emit the labeled health gauge for the **curated** relay set (the same bounded list the Phase 4 sampler already iterates `[VERIFIED: src/daemon/sampler.rs:157]`). Health for transient write relays is tracked in-memory for routing but NOT exported per-URL.
**Warning signs:** `/metrics` exposition grows unboundedly during a long crawl.

## Code Examples

### Threading the new registry into the daemon (mirrors RateLimiterRegistry)
```rust
// Source: pattern from src/daemon/mod.rs:163-168 (RateLimiterRegistry threading)
let health = Arc::new(RelayHealthRegistry::new(cfg.health_alpha));
// cloned into: the fetch fan-out (record + read), the notice consumer (rate-limit hit),
// and the sampler (emit labeled gauge for curated relays only).
let _notice = spawn_notice_consumer(client.clone(), Arc::clone(&registry), Arc::clone(&health));
```

### Recording health at the fetch arms
```rust
// Source: extends src/relay/fetch.rs::fetch_complete_with_timeout
let t0 = std::time::Instant::now();
match fetch_complete_with_timeout(/* ... */).await {
    Ok(events) => { health.record_success(relay_url, t0.elapsed()); Ok(events) }
    Err(RelayError::FetchTimeout(_)) => { health.record_timeout(relay_url); Err(/*..*/) }
    Err(RelayError::Client(_))       => { health.record_connect_failure(relay_url); Err(/*..*/) }
    Err(e)                           => { health.record_connect_failure(relay_url); Err(e) }
}
// Rate-limit hits are recorded in handle_relay_message (relay/mod.rs) alongside record_notice.
```
- Note: timeout vs connect-fail vs client-error distinction comes from the `RelayError` variant `[VERIFIED: src/error.rs]`. `RelayError::FetchTimeout` is the explicit timeout signal; `RelayError::Client` wraps nostr-sdk connect/subscribe/fetch errors. nostr-sdk does not expose a clean per-relay connect-status sampler, so map `Client(_)` to connect-failure and rely on the timeout variant for timeouts. Latency is measured with `Instant` around the fetch (the seam `fetch_window_with_deadline` already uses this idiom `[VERIFIED: src/relay/fetch.rs:248]`).

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| r-tag URLs dropped at ingest boundary | Extract via `nip65::extract_relay_list`, persist to `pubkey_relays` | Phase 5 | Enables RELAY-05 fallback |
| Static uniform fan-out over `cfg.relays` | Health-scored skip + per-relay concurrency | Phase 5 | Steers around degraded relays (RELAY-06) |
| Relay health = max NOTICE failure count (aggregate gauge) | Continuous per-relay EWMA in [0,1] | Phase 5 | Smooth routing/concurrency scaling |

**Deprecated/outdated:** none for this phase — the stack is current as of the Phase 4 verification.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | Default EWMA `alpha ≈ 0.3`, `latency_scale ≈ a few seconds`, rate-limit sample `0.2`, probe interval `~60s`, defaults for the 4 new config knobs | Pattern 6 / 7 | LOW — all explicitly Claude's discretion; tunable, no correctness impact. Planner picks final defaults. |
| A2 | Full-replace (not a diff) is acceptable for `apply_relay_list` because the row count per pubkey is tiny | Pattern 3 | LOW — relay lists are a few rows; the GRAPH-02 touch-zero idempotency need does not apply to this small non-hot table |
| A3 | `pubkey_relays` should NOT appear in any public contract view (internal routing state) | Pattern 1 | LOW — locked text says "unless it belongs there"; spam layer consumes the graph, not routing tables. Confirm with operator if the spam layer wants relay hints (out of v1 scope). |
| A4 | Mapping `RelayError::Client(_)` → connect-failure (nostr-sdk exposes no clean per-relay connect-status sampler) | Code Examples | LOW — `Client` errors are genuinely failures; the exact sub-classification only affects the EWMA sample, which is already coarse |

**If this table is empty:** it is not — all four are LOW-risk discretion/modeling choices, none are compliance/security/correctness claims.

## Open Questions

1. **Should the on-demand kind:10002 fetch count against the author's fetch-retry budget?**
   - What we know: the on-demand fetch is a separate curated fetch before the kind-3 fallback.
   - What's unclear: whether a *failed* on-demand kind:10002 fetch should requeue the author or proceed to `not_found`.
   - Recommendation: treat a missing/failed kind:10002 as "no write relays known" → proceed to terminal `not_found` (do not consume the kind-3 retry budget on a separate kind's miss). Cheap and avoids bounce loops.

2. **Probe-state location and reset on success.**
   - What we know: a skipped relay needs an occasional probe.
   - What's unclear: exact reset semantics (does a successful probe immediately re-admit, or only after the EWMA crosses the threshold?).
   - Recommendation: a successful probe updates the EWMA; re-admission is purely `score >= threshold` on the next tick. Keep `last_probe` in the registry.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Rust toolchain | build | ✓ | 1.94.0 (pinned, sqlx 0.9 MSRV) | — |
| All Phase-5 crates | build | ✓ | resolved in Cargo.lock | — |
| Docker (testcontainers Postgres) | integration tests | ✓ (per Phase 1–4 tests) | — | run per-binary `-- --test-threads=2`, re-run once on container/port flake |
| `sqlx-cli` | `.sqlx` regeneration | assumed installed (used since Phase 1) | — | `cargo install sqlx-cli` if missing |

**Missing dependencies with no fallback:** none.
**Missing dependencies with fallback:** none — all in-stack and already used by prior phases.

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in `#[test]` / `#[tokio::test]` + testcontainers Postgres (`tests/common/mod.rs`) |
| Config file | `Cargo.toml` (test deps); no separate runner config — optional `cargo-nextest` |
| Quick run command | `cargo test --test relay_health -- --test-threads=2` (per-binary, offline EWMA/routing tests need no DB) |
| Full suite command | `cargo test -- --test-threads=2` (testcontainers race; re-run once on container/port flake) |

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| RELAY-05 | kind:10002 r-tags extracted (bare=both, read, write) into `ValidatedRelayList` | unit (offline) | `cargo test --test relay_list -- --test-threads=2` (extend existing) | ✅ exists (extend) |
| RELAY-05 | `apply_relay_list` newest-wins replace (new winner deletes prior rows) | integration (Postgres) | `cargo test --test relay_list relay_list_replace -- --test-threads=2` | ❌ Wave 0 (new test in relay_list.rs) |
| RELAY-05 | not_found-on-curated author recovered via its write relay; `nip65_recovered_total`++ | integration | `cargo test --test nip65_fallback -- --test-threads=2` | ❌ Wave 0 |
| RELAY-05 | miss-on-curated AND miss-on-write-relay → terminal `not_found` | integration | `cargo test --test nip65_fallback fallback_miss_stamps_not_found` | ❌ Wave 0 |
| RELAY-06 | EWMA score rises on success, falls on timeout/connect-fail/rate-limit | unit (offline) | `cargo test --test relay_health ewma_moves_with_signals` | ❌ Wave 0 |
| RELAY-06 | relay below threshold is skipped, then probed back after recovery | unit (offline) | `cargo test --test relay_health skip_then_probe` | ❌ Wave 0 |
| RELAY-06 | per-relay permits scale with score (degraded gets fewer, min 1) | unit (offline) | `cargo test --test relay_health permits_scale_with_health` | ❌ Wave 0 |
| RELAY-06 | acquisition ordering is deadlock-free under per_relay_concurrency=1 | integration | `cargo test --test nip65_fallback no_deadlock_single_permit` | ❌ Wave 0 |
| config | new fields fail-fast: threshold∈[0,1], alpha∈(0,1], max_write_relays>0, per_relay_concurrency>0 | unit | `cargo test --test daemon_config -- --test-threads=2` (extend) | ✅ exists (extend) |

### Sampling Rate
- **Per task commit:** `cargo test --test relay_health` + `cargo build` (offline EWMA/routing tests are fast, no DB).
- **Per wave merge:** `cargo test --test nip65_fallback --test relay_list --test daemon_config -- --test-threads=2`.
- **Phase gate:** full suite green (`cargo test -- --test-threads=2`, re-run once on flake) before `/gsd-verify-work`.

### Wave 0 Gaps
- [ ] Make `ScriptedGraph` relay-URL-aware: change the inner map from `author → event` to `(relay_url, author) → events` (or `author → HashMap<relay_url, Vec<Event>>`) so a test can model "author absent on curated relay A, present on write relay B." Add an error-injection variant: a closure that returns `Err(RelayError::FetchTimeout(url))` / `Err(RelayError::Client(..))` for a designated relay so health/timeout capture is exercised. Keep it `Send`/`Clone`/`Arc`-backed (it crosses `tokio::spawn` in the daemon loop) `[VERIFIED: tests/common/mod.rs:150]`.
- [ ] `tests/relay_health.rs` — offline EWMA + routing + permit-scaling (covers RELAY-06; no DB).
- [ ] `tests/nip65_fallback.rs` — fallback recovery + miss + deadlock-safety (covers RELAY-05/06; Postgres).
- [ ] Extend `tests/relay_list.rs` — r-tag extraction (bare/read/write markers) + `apply_relay_list` replace (RELAY-05). Build a kind:10002 event with real r-tags in the fixture: `EventBuilder::new(Kind::RelayList, "").tags([Tag::custom(TagKind::single_letter('r'...), ["wss://...","write"])...])` — or use the nostr NIP-65 builder helper if present; assert `nip65::extract_relay_list` yields the expected pairs.
- [ ] Extend `tests/daemon_config.rs` — fail-fast guards for the 5 new fields.
- [ ] Extend `common/mod.rs` fixtures with a `relay_list_event(author_seed, &[(url, marker)])` helper (sibling of `follows_event`).

## Security Domain

> `security_enforcement` is `false` in `.planning/config.json` — this section is informational only, not a gate.

Brief note for the planner (not enforced): write relays are arbitrary, attacker-influenced URLs drawn from adversarial kind:10002 events. The fallback fetch already routes through `acquire_validated_lists` (verify id+sig, drop unsolicited authors, dedup, newest-wins, follow-cap) `[VERIFIED: src/relay/mod.rs:149]`, so a hostile write relay cannot inject follows for an author it does not control — the same INGEST-01/Pitfall-4 protections apply. `RelayUrl::parse` rejects non-`ws`/`wss` schemes `[VERIFIED: nostr-0.44.3/src/types/url.rs:108]`. Bound fallback fan-out with `nip65_max_write_relays` (locked) so a kind:10002 advertising 500 write relays cannot blow up fan-out. Keep the labeled health gauge to the bounded curated set to avoid metric-cardinality DoS (Pitfall 7).

## Sources

### Primary (HIGH confidence)
- `Cargo.lock` — resolved versions (governor 0.10.4, metrics 0.24.6, nostr 0.44.3, nostr-sdk 0.44.1, sqlx 0.9.0, tokio 1.52.3) — confirms no new deps `[VERIFIED]`
- `nostr-0.44.3/src/nips/nip65.rs` — `extract_relay_list`, `RelayMetadata { Read, Write }`, bare-tag-means-both semantics `[VERIFIED]`
- `nostr-0.44.3/src/types/url.rs` — `RelayUrl::as_str_without_trailing_slash`, `parse` scheme validation, `Display` `[VERIFIED]`
- `nostr-0.44.3/src/prelude.rs` + `nostr-sdk-0.44.1/src/lib.rs` — `nip65` re-export reachability `[VERIFIED]`
- Project source read in-session: `src/ingest/{mod,replaceable,verify}.rs`, `src/relay/{mod,fetch,rate_limit,nip11}.rs`, `src/crawl/{mod,apply,frontier}.rs`, `src/store/{mod,follows,pubkeys}.rs`, `src/daemon/{mod,loop_,config,observe,sampler}.rs`, `migrations/0001-0003`, `tests/{common/mod,mock_relay/mod,relay_list,daemon_loop,observe}.rs`, `ops/grafana-dashboard.json` `[VERIFIED]`

### Secondary (MEDIUM confidence)
- CLAUDE.md stack/“what NOT to use” tables (project-authoritative; cross-checked against Cargo.lock) `[CITED]`

### Tertiary (LOW confidence)
- none — no WebSearch needed; everything verified locally against the installed crates and project source.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — every version confirmed in `Cargo.lock`; no new deps.
- Architecture: HIGH — all integration seams (fan-out, fetch arms, not_found arm, registry threading) read directly from current source.
- NIP-65 API: HIGH — `extract_relay_list` / `RelayMetadata` / `RelayUrl` verified in the installed `nostr-0.44.3` source.
- Pitfalls: HIGH — deadlock ordering, semaphore-shrink, cardinality, and `.sqlx` regen are grounded in the existing code and Phase 4 recorded lessons.
- EWMA defaults/formula: MEDIUM (Claude's discretion; assumptions logged A1).

**Research date:** 2026-06-15
**Valid until:** 2026-07-15 (stack is locked + already resolved; only changes if the dependency tree is bumped, which is out of this phase's scope)
