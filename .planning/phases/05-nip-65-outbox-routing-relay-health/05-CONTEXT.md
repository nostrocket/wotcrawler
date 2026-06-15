# Phase 5: NIP-65 Outbox Routing & Relay Health - Context

**Gathered:** 2026-06-15
**Status:** Ready for planning

<domain>
## Phase Boundary

The final milestone phase. Recover pubkeys the curated relay set cannot supply by falling back to their advertised NIP-65 (kind:10002) write relays, and make observed relay behavior drive routing and per-relay concurrency so the crawler steers traffic away from degraded relays toward healthy ones.

Requirements in scope: RELAY-05 (NIP-65 write-relay fallback), RELAY-06 (relay health score driving routing + per-relay concurrency).

Builds on: INGEST-05 (kind:10002 validated under replaceable-event rules — Phase 2, already complete but the r-tag relay URLs are NOT yet extracted/persisted); the manual per-relay fetch path (`fetch_complete_with_timeout`, Phase 2); `RateLimiterRegistry` (Phase 2); the daemon fetch_union fan-out + observability (Phase 4).

Out of scope (v2): NIP-77 negentropy bulk sync (RELAY-07), streaming live kind-3 subscriptions (RELAY-08), adaptive per-pubkey refresh (FRESH-04).
</domain>

<decisions>
## Implementation Decisions

### kind:10002 Storage & Extraction (RELAY-05 prerequisite)
- New migration `0004` adds a `pubkey_relays` table: `(pubkey_id BIGINT REFERENCES pubkeys(id), url TEXT, marker TEXT CHECK (marker IN ('read','write','both')), seen_at TIMESTAMPTZ)`, with newest-wins replacement per pubkey (a fresh winning kind:10002 replaces that pubkey's prior relay rows — delete-not-in-set, mirroring the follows edge-diff pattern). Additive/idempotent per the 0001/0002/0003 conventions; keep internal bookkeeping out of the public contract views unless it belongs there.
- Extend the ingest path: after `pick_winner` resolves a kind:10002 event, extract the `r`-tag relay URLs + read/write markers into a new `ValidatedRelayList` type (companion to `ValidatedFollowList`), and persist via a new store fn `apply_relay_list(pool, pubkey_id, &[(url, marker)])`. INGEST-05's replaceable/verify/dedup pipeline is reused unchanged; only the winning-event r-tag extraction + persistence is new.
- Acquisition timing: persist relay lists whenever a kind:10002 winner is seen. At the `not_found` hook, if a pubkey's write relays are not yet known, fetch its kind:10002 **on-demand** from the curated set first, extract+persist, then use the write relays. (Avoids a blanket dual-kind fetch on every batch; write relays are acquired exactly when needed and cached for reuse.)

### NIP-65 Fallback Routing (RELAY-05)
- **Manual** fallback fetch using the existing `fetch_complete_with_timeout` path against the pubkey's NIP-65 write relays — NOT `nostr-sdk` `ClientOptions::gossip(true)`. Rationale: the whole acquisition stack (explicit pagination, NIP-11 limits, per-relay rate limiting, deterministic ScriptedGraph testing) is manual; gossip(true) would bypass that controllable, testable machinery.
- Trigger point: the `not_found` decision in `src/crawl/apply.rs` (the `None` arm, ~lines 186-194). Before stamping `not_found`, attempt the write-relay fallback; only stamp `not_found` if the fallback also yields nothing.
- Fan-out cap: configurable max write-relays tried per pubkey (default ~3) to bound fan-out; prefer healthier relays (per the health score) in selection; honor the existing per-relay rate limiting.
- Outcome: a fallback hit routes through `apply_validated` (status → `fetched`); still nothing → terminal `not_found`. Export a `nip65_recovered_total` counter so the operator sees fallback effectiveness (and it informs the curated-coverage concern carried from earlier phases).

### Relay Health Score (RELAY-06)
- Track all four signals named in the success criterion, per relay: connect failures, timeouts, rate-limit hits, and response latency. Add per-relay fetch success counts too (the registry today only tracks NOTICE-driven failures).
- Score model: a continuous EWMA-based health score in [0,1] per relay — success rate penalized by latency and rate-limit hits (configurable EWMA alpha). Continuous (not discrete tiers) so routing/concurrency can scale smoothly.
- State: an in-memory `RelayHealthRegistry` (extends or parallels `RateLimiterRegistry`), rebuilt from live observation each daemon run. Not persisted across restarts — a multi-day daemon re-learns health quickly, and a `relay_health` table would add migration + write load for marginal benefit.
- Capture sites: the `Err` arms of `fetch_complete_with_timeout` (timeout / connect failure / client error), the success path (record latency + success), and the existing NOTICE consumer (rate-limit hits). These feed the registry.

### Health-Driven Routing & Per-Relay Concurrency (RELAY-06)
- Routing effect: in the fan-out, skip relays whose health score is below a configurable threshold — BUT periodically probe skipped relays (a low-rate health probe) so a recovered relay can climb back into rotation. Avoids permanently blacklisting a relay after a transient outage.
- Per-relay concurrency: add a per-relay `Semaphore` (`HashMap<url, Arc<Semaphore>>`) whose permit count scales with the relay's health (healthy → more permits, degraded → fewer). Supplements the existing global crawl Semaphore + per-relay GCRA rate limiting.
- Config additions (with fail-fast validation in `validate()`): `nip65_fallback_enabled: bool`, `nip65_max_write_relays: usize` (>0), `relay_health_threshold: f64` (in [0,1]), `per_relay_concurrency: usize` (>0), and the health EWMA `alpha` (in (0,1]). Defaults chosen conservative/relay-polite.
- Observability: a per-relay health gauge (labeled by relay URL), the `nip65_recovered_total` counter, and per-relay concurrency-in-use; add corresponding Grafana panels to `ops/grafana-dashboard.json` (mind the exporter's `_total` counter suffix convention fixed in Phase 4).
</decisions>

<code_context>
## Existing Code Insights

### Reusable Assets
- INGEST-05 pipeline: `src/ingest/mod.rs::ingest_events` (accepts `Kind::RelayList`), `src/ingest/replaceable.rs::pick_winner` (kind-agnostic), `src/ingest/verify.rs::accept` — reuse unchanged; add only r-tag extraction from the winning kind:10002 event. The relay URLs are currently dropped at the ingest boundary (`ingest/mod.rs:109-110`).
- `not_found` hook: `src/crawl/apply.rs:186-194` (the `None` arm of the by-author match) — the exact RELAY-05 fallback insertion point.
- Fetch path: `src/relay/fetch.rs::fetch_complete_with_timeout` (per-relay, rate-limited, paginated) — reuse for the write-relay fallback fetch. `acquire_validated_lists` (`relay/mod.rs:149`) + `process_batch` (`crawl/apply.rs:107`) are the surrounding seams.
- `RateLimiterRegistry` (`src/relay/rate_limit.rs:131`): per-relay GCRA limiters + failure counts; `record_notice`, `backoff`, `reset`, `failure_count`, `active_relay_count`. Extend (or parallel) for the health score. The NOTICE consumer (`relay/mod.rs:280-311`) already routes rate-limit notices.
- Daemon fan-out: `src/daemon/mod.rs:176-222` fetch_union closure (the only routing site today — static uniform fan-out over `cfg.relays`). Global crawl `Semaphore` at `daemon/loop_.rs:98`. Per-relay metrics + sampler gauges (`daemon/sampler.rs`), observe router/metric constants (`daemon/observe.rs`).
- Config: `src/daemon/config.rs` (`relays`, `reqs_per_second`, `fetch_timeout`, `concurrency`, `batch_size`, `max_attempts`; `validate()` ~line 194 for new guards).

### Established Patterns
- Raw SQL via `sqlx::query!`/`query_scalar!` `$N` binds; bytea pubkeys as `Vec<u8>`; surrogate bigint ids; `.sqlx/` offline metadata committed + regenerated with `cargo sqlx prepare -- --all-targets` (lib-only prunes integration metadata here); build with `SQLX_OFFLINE=true`.
- Migrations additive/idempotent (`IF NOT EXISTS`, named CHECK constraints, `COMMENT ON`); newest-wins replace via transactional delete-not-in-set (see `apply_follow_list`).
- Metrics: `metrics::counter!`/`gauge!`/`histogram!`; exporter appends `_total` to counters in exposition (keep code + Grafana names aligned — Phase 4 WR-02 lesson; do NOT manually suffix counter names with `_total`).
- Tests: testcontainers Postgres fixture (`tests/common/mod.rs`), `ScriptedGraph` (Send, used with `run_daemon_loop`), `ScriptedRelay` (not Send). KNOWN testcontainers flake under full-suite load — run per-binary with `-- --test-threads=2`/`1`, re-run once on a container/port timeout.

### Integration Points
- Phase 5 must make the mocks relay-URL-aware (today `ScriptedGraph` collapses all relays into one union — it cannot model "relay A not_found, relay B has it") and add error injection (`Err(RelayError::FetchTimeout)`) — needed to test fallback + health-driven routing. Build these test seams.
- New: migration 0004 + `pubkey_relays`, `ValidatedRelayList` + r-tag extraction + `apply_relay_list`, `RelayHealthRegistry` + EWMA scoring, fallback logic at the not_found hook, health-driven relay selection + per-relay Semaphore in/around the fetch_union fan-out, new config fields + validation, new metrics + Grafana panels.
</code_context>

<specifics>
## Specific Ideas

- The `nip65_recovered_total` counter directly answers the long-standing curated-coverage concern (carried from Phase 3/4 STATE): it quantifies how many pubkeys the curated set alone could not supply.
- Health-driven routing should "steer around" degraded relays (skip-below-threshold + periodic probe), not hard-blacklist them — transient outages must self-heal.
- Per-relay concurrency scaling by health is the visible RELAY-06 success-criterion behavior ("a degraded relay receives less traffic than a healthy one").
</specifics>

<deferred>
## Deferred Ideas

- NIP-77 negentropy bulk sync (RELAY-07, v2 — ~16% relay support today).
- Streaming live kind-3 subscriptions for near-real-time updates (RELAY-08, v2).
- Persisting relay health across daemon restarts (a `relay_health` table) — in-memory observation is sufficient for a multi-day daemon; revisit only if restart warm-up proves costly.
- nostr-sdk gossip(true) outbox routing — explicitly rejected in favor of the manual, testable fetch path.
</deferred>
