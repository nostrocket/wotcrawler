---
phase: 05-nip-65-outbox-routing-relay-health
verified: 2026-06-15T12:00:00Z
status: passed
score: 9/9 must-haves verified
overrides_applied: 0
re_verification: false
---

# Phase 5: NIP-65 Outbox Routing & Relay Health — Verification Report

**Phase Goal:** Pubkeys the curated set cannot supply are recovered via their advertised NIP-65 write relays, and observed relay behavior drives routing and per-relay concurrency so the crawler steers around degraded relays.
**Verified:** 2026-06-15T12:00:00Z
**Status:** passed
**Re-verification:** No — initial verification

---

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | A winning kind:10002 event's r-tag relay URLs and read/write markers are extracted into a ValidatedRelayList (bare r-tag = both) | VERIFIED | `src/ingest/relay_list.rs:extract_relay_pairs` uses `nip65::extract_relay_list`; `marker_of` maps `None`→`"both"`, `Read`→`"read"`, `Write`→`"write"`. `tests/relay_list.rs`: 5 tests pass including bare/read/write and trailing-slash normalization. |
| 2 | apply_relay_list persists a pubkey's relays as a newest-wins full replace in one transaction | VERIFIED | `src/store/relays.rs:apply_relay_list`: `pool.begin()` → DELETE by pubkey_id → INSERT ON CONFLICT DO NOTHING per row → `tx.commit()`. `tests/relay_lists_store.rs:apply_relay_list_newest_wins_replace` passes. |
| 3 | lookup_write_relays returns only relays with marker IN ('write','both') for a pubkey | VERIFIED | `src/store/relays.rs:lookup_write_relays`: `SELECT url FROM pubkey_relays WHERE pubkey_id = $1 AND marker IN ('write','both')`. `tests/relay_lists_store.rs:lookup_write_relays_write_and_both` passes. |
| 4 | When a pubkey's kind-3 is not found on the curated set, process_batch attempts a fallback fetch from that pubkey's NIP-65 write relays before stamping not_found | VERIFIED | `src/crawl/apply.rs:fallback_recover` called from the `None` arm of the by_author match (line 242). `tests/nip65_fallback.rs:fallback_recovers_via_write_relay` and `fallback_miss_stamps_not_found` pass. |
| 5 | If write relays are unknown, an on-demand curated kind:10002 fetch resolves+persists them first (a failed kind:10002 fetch does NOT consume the kind-3 retry budget) | VERIFIED | `fallback_recover` lines 324-341: on empty write_relays, calls `relay_list_fetch`, resolves via `resolve_relay_list`, calls `apply_relay_list` keyed by freshly-upserted pubkey_id (WR-02 fix). `tests/nip65_fallback.rs:unknown_write_relays_no_kind10002_stamps_not_found` asserts fetch_attempts == 0 and `on_demand_kind10002_resolves_then_recovers` proves end-to-end persistence. |
| 6 | Each relay carries a continuous EWMA health score in [0,1]; success raises it (penalized by latency), timeout/connect-failure drives it toward 0, rate-limit hit degrades it (not to zero) | VERIFIED | `src/relay/health.rs:RelayHealthRegistry`: `record_success` (sample=1/(1+lat/3)), `record_timeout` (0.0), `record_connect_failure` (0.0), `record_rate_limited` (0.2), all through `update` EWMA. `tests/relay_health.rs:ewma_moves_with_signals` passes. |
| 7 | Per-relay permit count scales with health: max(1, round(per_relay_concurrency * score)) so a degraded relay gets fewer (but at least 1) permits; a relay below relay_health_threshold is skipped for routing unless a probe is due | VERIFIED | `permits()` line 157-159; `try_mark_attempt()` lines 236-262 (atomic CR-02 fix). Fan-out uses `try_mark_attempt` at daemon/mod.rs:259. `tests/relay_health.rs:permits_scale_with_health`, `skip_then_probe`, `try_mark_attempt_single_probe_under_concurrency` all pass. |
| 8 | Resource acquisition order is fixed everywhere: global crawl permit → per-relay permit → GCRA token → fetch (deadlock-safe); the CR-01 livelock (permit held across in_use spin) and CR-02 probe race are fixed | VERIFIED | `admit_per_relay` in health.rs lines 288-343: in_use check loop (sleep 1ms, WR-01 fix) BEFORE semaphore acquire. `try_mark_attempt` holds last_probe lock across check+claim (CR-02). Code comment at daemon/mod.rs:234-251. `tests/nip65_fallback.rs:no_deadlock_single_permit` and `tests/relay_health.rs:admit_per_relay_no_livelock_under_saturation` pass. |
| 9 | Per-relay health gauge (curated set only), nip65_recovered counter, and per-relay concurrency-in-use gauge are exported; Grafana panels render them | VERIFIED | `observe.rs`: `METRIC_RELAY_HEALTH`, `METRIC_NIP65_RECOVERED`, `METRIC_RELAY_CONCURRENCY` consts (lines 80, 87, 93). `sampler.rs:sample_gauges` emits labeled gauges ONLY over the bounded `relays` slice (lines 171-175). Grafana JSON valid and contains `"expr": "relay_health"`, `"expr": "sum(rate(nip65_recovered_total[5m]))"`, `"expr": "relay_concurrency_in_use"`. `nip65_recovered` fired without `_total` (apply.rs:266). |

**Score:** 9/9 truths verified

---

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `migrations/0004_pubkey_relays.sql` | pubkey_relays table + index + INTERNAL comment, additive/idempotent | VERIFIED | `CREATE TABLE IF NOT EXISTS pubkey_relays` with named CHECK `pubkey_relays_marker_check`, index `pubkey_relays_pubkey_idx`, `INTERNAL:` + `NOT part of the public contract` in COMMENT ON TABLE. |
| `src/ingest/relay_list.rs` | extract_relay_pairs via nip65 helper; None→both, Read→read, Write→write | VERIFIED | Uses `nip65::extract_relay_list`; zero `as_vec`/`TagKind::single_letter` occurrences. `from_event` and `resolve_relay_list` present. |
| `src/ingest/mod.rs` | ValidatedRelayList type with fields pubkey, event_id, created_at, relays | VERIFIED | Lines 78-89: struct present with all 4 fields, `pub mod relay_list` registered at line 20. |
| `src/store/relays.rs` | apply_relay_list (transactional full replace) + lookup_write_relays | VERIFIED | Both functions present and `pub`. Transaction discipline mirrors `follows.rs`. |
| `src/relay/health.rs` | RelayHealthRegistry with EWMA/permit/probe; admit_per_relay; DEFAULT_HEALTH_* consts | VERIFIED | All 5 DEFAULT_* consts, all required methods, `admit_per_relay` free function. CR-01 (permit after in_use loop), CR-02 (`try_mark_attempt`), WR-01 (sleep 1ms) all present. |
| `src/daemon/config.rs` | 5 new fields with const-backed defaults + fail-fast validates + Debug | VERIFIED | Fields: `nip65_fallback_enabled`, `nip65_max_write_relays`, `relay_health_threshold`, `per_relay_concurrency`, `health_alpha`. All in Debug. `validate` checks all 4 guard conditions. |
| `src/daemon/mod.rs` | Health-driven fan-out with skip+probe, per-relay Semaphore, deadlock-safe order, health capture; single RelayHealthRegistry::new; fallback_fetch + relay_list_fetch closures; both using admit_per_relay (CR-03) | VERIFIED | Single `RelayHealthRegistry::new` at line 176. `try_mark_attempt` at line 259. `admit_per_relay` at lines 272, 365, 433. `fallback_sems` for CR-03 at lines 329-363, 422-431. process_batch called with all required params. |
| `src/relay/fetch.rs` | record_fetch_health mapping Ok→success+latency, FetchTimeout→timeout, other→connect_failure | VERIFIED | Lines 271-285: `record_fetch_health` function present, correct RelayError variant mapping. Called from daemon fan-out at mod.rs:294. |
| `src/daemon/observe.rs` | METRIC_RELAY_HEALTH, METRIC_NIP65_RECOVERED, METRIC_RELAY_CONCURRENCY consts | VERIFIED | Lines 80, 87, 93 respectively. |
| `src/daemon/sampler.rs` | Labeled per-relay health + concurrency-in-use gauges over curated set only; Arc<RelayHealthRegistry> threaded | VERIFIED | Lines 117-177: `sample_gauges` takes `Arc<RelayHealthRegistry>`, emits both gauges inside `for r in &relays` loop (curated-only). |
| `ops/grafana-dashboard.json` | 3 new panels: relay_health (labeled), nip65_recovered_total rate, relay_concurrency_in_use | VERIFIED | Valid JSON (`python3 -c "import json; json.load(...)"`). All 3 expressions confirmed present. |
| `tests/relay_health.rs` | ewma_moves_with_signals, skip_then_probe, permits_scale_with_health + CR-01/CR-02 regression tests | VERIFIED | 5 tests, all pass in 0.03s: `ewma_moves_with_signals`, `permits_scale_with_health`, `skip_then_probe`, `try_mark_attempt_single_probe_under_concurrency`, `admit_per_relay_no_livelock_under_saturation`. |
| `tests/nip65_fallback.rs` | fallback_recovers_via_write_relay, fallback_miss_stamps_not_found, no_deadlock_single_permit (un-ignored) | VERIFIED | 5 tests all pass (2.50s over testcontainers). All three named tests non-ignored. Additionally `unknown_write_relays_no_kind10002_stamps_not_found` and `on_demand_kind10002_resolves_then_recovers` present and green. |
| `tests/relay_lists_store.rs` | migration_0004_idempotent, apply_relay_list newest-wins, lookup_write_relays_write_and_both | VERIFIED | 3 tests, all pass (2.26s over testcontainers). |

---

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `src/ingest/relay_list.rs` | `nostr_sdk::nip65::extract_relay_list` | r-tag extraction on winning kind:10002 | WIRED | Line 48: `nip65::extract_relay_list(event)` called in `extract_relay_pairs`. |
| `src/store/relays.rs` | pubkey_relays table | DELETE then INSERT in one tx; SELECT marker IN ('write','both') | WIRED | Lines 49-66 (txn delete+insert), lines 80-85 (`marker IN ('write','both')`). |
| `src/crawl/apply.rs (None arm)` | `store::relays::lookup_write_relays + apply_relay_list` | resolve write relays; on-demand curated kind:10002 fetch+persist when unknown | WIRED | Lines 316, 328, 334, 341 in `fallback_recover`. WR-02 fix: lookup uses `pubkey_id` after upsert. |
| `src/crawl/apply.rs` | `acquire_validated_lists` | single-author re-resolution of fallback events; nip65_recovered counter on hit | WIRED | Lines 361-373: `acquire_validated_lists` called with single-author set; `metrics::counter!("nip65_recovered").increment(1)` at line 266 in the recovery arm. |
| `src/relay/mod.rs` | `RelayHealthRegistry::record_rate_limited` | RateLimited arm beside record_notice; Blocked arm fires record_connect_failure (WR-03) | WIRED | Lines 273-284: `record_rate_limited` in RateLimited arm, `record_connect_failure` in Blocked arm. |
| `src/daemon/mod.rs` | `spawn_notice_consumer` | Arc<RelayHealthRegistry> constructed once (cfg.health_alpha) and passed in | WIRED | Line 176: `RelayHealthRegistry::new(cfg.health_alpha)`, line 178: passed to `spawn_notice_consumer`. Single construction confirmed. |
| `src/daemon/mod.rs fetch_union fan-out` | `RelayHealthRegistry::try_mark_attempt + admit_per_relay + record_*` | skip-below-threshold (atomic CR-02) + per-relay semaphore admission + Ok/Err health capture | WIRED | Lines 259 (`try_mark_attempt`), 272 (`admit_per_relay`), 294 (`record_fetch_health`). |
| `src/daemon/mod.rs` | `process_batch fallback_fetch + relay_list_fetch` | live-Client closures with admit_per_relay (CR-03) + fallback config + shared health registry | WIRED | Lines 335-457: `fallback_fetch` (admit_per_relay at 365), `relay_list_fetch` (admit_per_relay at 433), both using `fallback_sems` for per-URL semaphores. Lines 459-460: config values extracted. Lines 504-508: all threaded into `run_daemon_loop`. |
| `src/daemon/config.rs` | `relay::health::DEFAULT_HEALTH_*` | serde default fns reference consts by name | WIRED | Lines 29-32: all 5 DEFAULT_* consts imported and used in default_* fns (lines 161-175). |

---

### Data-Flow Trace (Level 4)

| Artifact | Data Variable | Source | Produces Real Data | Status |
|----------|---------------|--------|--------------------|--------|
| `src/crawl/apply.rs` | `write_relays` | `lookup_write_relays(pool, claimed.id)` → `pubkey_relays` table | Yes — real DB query via sqlx macro | FLOWING |
| `src/crawl/apply.rs` | `recovered` (ValidatedFollowList) | `acquire_validated_lists` over raw events from `fallback_fetch` closure | Yes — real fetch closure connected at daemon | FLOWING |
| `src/daemon/sampler.rs` | health gauge values | `health.score(r)` / `health.in_use(r)` from live registry | Yes — EWMA updated from real fetch/notice outcomes | FLOWING |

---

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| relay_health offline unit tests (EWMA, probe, permits) | `SQLX_OFFLINE=true cargo test --test relay_health -- --test-threads=2` | 5/5 passed, 0.03s | PASS |
| relay_list extraction tests (marker mapping, normalization) | `SQLX_OFFLINE=true cargo test --test relay_list -- --test-threads=2` | 5/5 passed, 0.00s | PASS |
| relay_lists_store DB tests (migration idempotency, newest-wins, lookup) | `SQLX_OFFLINE=true cargo test --test relay_lists_store -- --test-threads=2` | 3/3 passed, 2.26s | PASS |
| NIP-65 fallback integration tests (recovery, miss, deadlock-safety) | `SQLX_OFFLINE=true cargo test --test nip65_fallback -- --test-threads=2` | 5/5 passed, 2.50s | PASS |
| daemon_config validation tests (5 new fields + guards) | `SQLX_OFFLINE=true cargo test --test daemon_config -- --test-threads=1` | 14/14 passed, 0.01s | PASS |
| Full build (daemon binary + all test binaries) | `SQLX_OFFLINE=true cargo build --bin crawler && cargo build --tests` | Clean, no errors | PASS |
| Grafana dashboard JSON validity | `python3 -c "import json; json.load(open('ops/grafana-dashboard.json'))"` | VALID JSON | PASS |

---

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|------------|------------|-------------|--------|----------|
| RELAY-05 | 05-01, 05-03 | When a pubkey's kind 3 isn't found on curated relays, fall back to NIP-65 write relays | SATISFIED | `fallback_recover` in apply.rs; `lookup_write_relays` + `apply_relay_list` in store/relays.rs; migration 0004; `tests/nip65_fallback.rs` fully green. |
| RELAY-06 | 05-02, 05-04 | Each relay carries a health score from observed behavior (connect failures, timeouts, rate-limit hits, response latency) driving routing and per-relay concurrency | SATISFIED | `RelayHealthRegistry` with EWMA; `admit_per_relay`; `try_mark_attempt` (CR-02); per-relay Semaphore + in_use gate (CR-01 fixed); `record_fetch_health` at fan-out; `record_rate_limited`/`record_connect_failure` in NOTICE consumer; health-scaled gauges in sampler. |

---

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| — | — | None found | — | No debt markers (TBD/FIXME/XXX), no stubs, no hardcoded empty returns in production code paths across all 16 reviewed files. |

**CLAUDE.md convention checks:**
- No `database_url` logged anywhere in Phase 5 files.
- No `nip65_recovered_total` (manual `_total`) in any source file — the metric is fired as `"nip65_recovered"` and the exporter appends `_total`.
- `pubkey_relays` confirmed absent from all contract views (`grep -rni "create .*view" migrations/ | grep -i pubkey_relays` returned empty).
- `src/crawl/apply.rs` confirmed Client-free (`grep "use nostr_sdk::Client"` returned empty).
- NIP-65 tag parsing uses the built-in helper, not hand-rolled (`as_vec`/`TagKind::single_letter` count in relay_list.rs = 0).

---

### Code Review Resolution Verification

All CR and WR findings reported in 05-REVIEW.md are confirmed fixed:

- **CR-01 (livelock):** `admit_per_relay` acquires the per-relay semaphore AFTER the `in_use < permits` admission loop (health.rs:310-326). Regression test `admit_per_relay_no_livelock_under_saturation` passes.
- **CR-02 (probe race):** `try_mark_attempt` holds the `last_probe` Mutex across the read-and-conditional-write (health.rs:236-262). The fan-out uses `try_mark_attempt` exclusively (daemon/mod.rs:259). Regression test `try_mark_attempt_single_probe_under_concurrency` passes (32 concurrent callers → exactly 1 winner).
- **CR-03 (fallback bypasses admission):** Both `fallback_fetch` and `relay_list_fetch` closures use `admit_per_relay` via a per-URL on-demand `fallback_sems` HashMap (daemon/mod.rs:329-457). Write-relay URLs are concurrency-capped and health-observed.
- **WR-01 (yield_now spin):** The admission loop uses `tokio::time::sleep(Duration::from_millis(1))` instead of `yield_now` (health.rs:314).
- **WR-02 (wrong pubkey_id):** `lookup_write_relays` after `apply_relay_list` uses the freshly-upserted `pubkey_id`, not `claimed.id` (apply.rs:334).
- **WR-03 (Blocked no health degrade):** `Blocked` arm now calls `health.record_connect_failure(relay_url)` (relay/mod.rs:284).
- **WR-04:** Confirmed justified false-positive (FnMut borrow constraint prevents the suggested reorder; existing pattern is functionally correct).
- **IN-01/IN-02:** Info-only, out of scope.

---

### Human Verification Required

None. All RELAY-05 and RELAY-06 mechanics are fully testable offline and via testcontainers. No `checkpoint:human-verify` blocks exist in Phase 5 plans. Live-relay validation of fallback effectiveness and health-routing in production is a Phase 4 operator-UAT concern (real relays / Grafana), not a Phase 5 completeness gate.

---

### Gaps Summary

No gaps. All 9 must-have truths verified, all artifacts substantive and wired, all key links confirmed, all tests pass, no debt markers, no stubs in production paths.

---

_Verified: 2026-06-15T12:00:00Z_
_Verifier: Claude (gsd-verifier)_
