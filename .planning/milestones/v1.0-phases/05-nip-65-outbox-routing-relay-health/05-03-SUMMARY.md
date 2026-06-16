---
phase: 05-nip-65-outbox-routing-relay-health
plan: 03
subsystem: crawl
tags: [RELAY-05, nip-65, outbox-fallback, ingest, process_batch]
requires:
  - "05-01: store::relays::{apply_relay_list, lookup_write_relays}, ValidatedRelayList, ingest::relay_list::{extract_relay_pairs, from_event}, URL-aware/error-injecting ScriptedGraph + relay_list_event fixture"
  - "05-02: relay::health::RelayHealthRegistry (score, DEFAULT_HEALTH_ALPHA, DEFAULT_NIP65_MAX_WRITE_RELAYS)"
provides:
  - "crawl::apply::process_batch fallback params (fallback_enabled, nip65_max_write_relays, &RelayHealthRegistry, injected fallback_fetch + relay_list_fetch closures) + None-arm RELAY-05 recovery"
  - "crawl::apply::fallback_recover (single-author NIP-65 write-relay recovery helper)"
  - "ingest::relay_list::resolve_relay_list (single-author kind:10002 winner re-resolution; reuses verify/dedup/pick_winner)"
  - "tests/common::dt(secs) -> DateTime<Utc> fixture helper"
affects:
  - "05-04: must thread live fallback_fetch/relay_list_fetch closures + fallback config (enabled/max-write-relays) + the daemon RelayHealthRegistry through run_daemon_loop (currently passes fallback_enabled=false + no-op closures)"
tech-stack:
  added: []
  patterns:
    - "Injected-closure seam extended: a second/third Fn-returning-Future param keeps crawl/apply.rs free of the live nostr_sdk Client (no circular dep, ScriptedGraph-testable) — mirrors the existing union_fetch seam"
    - "Single-author re-resolution of adversarial fallback events through the unchanged acquire_validated_lists gate (verify/dedup/newest-wins/clamp still run)"
    - "resolve_relay_list reuses verify::accept + replaceable::pick_winner (no new validation logic) — the relay-list analogue of ingest_events that keeps the winning event for r-tag extraction"
key-files:
  created: []
  modified:
    - "src/crawl/apply.rs (process_batch fallback params + None-arm recovery + fallback_recover helper)"
    - "src/ingest/relay_list.rs (resolve_relay_list)"
    - "src/crawl/mod.rs (run_crawl call site: fallback disabled + no-op closures)"
    - "src/daemon/loop_.rs (run_daemon_loop call site: fallback disabled + no-op closures)"
    - "tests/nip65_fallback.rs (4 active RELAY-05 tests + 1 still-ignored 05-04 scaffold)"
    - "tests/common/mod.rs (dt fixture helper)"
decisions:
  - "Recovery is asserted via process_batch's returned applied-count (==1 on a hit — the same hit that fires nip65_recovered) rather than a metrics-recorder snapshot: deterministic, recorder-free, and the count already feeds crawl bookkeeping."
  - "resolve_relay_list lives in ingest::relay_list (not a process_batch closure) because the kind:10002 winner's r-tags must survive — ingest_events emits ValidatedFollowList and discards r-tags, so the on-demand path needs the winning event itself."
  - "run_crawl + run_daemon_loop call sites pass fallback_enabled=false + a fresh empty RelayHealthRegistry + no-op closures: this keeps the crate compiling and behavior identical (Phase-3 driver never exercised the fallback; the live wiring is 05-04's health-driven fan-out). Deviation Rule 3 (blocking compile)."
metrics:
  duration: 22min
  completed: 2026-06-15
  tasks: 2
  files: 6
---

# Phase 05 Plan 03: NIP-65 Outbox Fallback at the not_found Hook Summary

RELAY-05 is wired at the `process_batch` `not_found` (None) arm: a curated-miss author is recovered from its NIP-65 write relays via an injected `fallback_fetch` closure (on-demand curated kind:10002 resolve+persist when write relays are unknown), re-validated through the same single-author `acquire_validated_lists` gate, applied + counted (`nip65_recovered`) on a hit, and stamped terminal `not_found` on a miss — all while `crawl/apply.rs` stays free of the live `Client`.

## What Was Built

**Task 1 — fallback injection (`src/crawl/apply.rs`, `src/ingest/relay_list.rs`):**
- `process_batch` gained `fallback_enabled: bool`, `nip65_max_write_relays: usize`, `health: &RelayHealthRegistry`, and two injected closures: `fallback_fetch: Fn(PublicKey, Vec<String>) -> Fut<Result<Vec<Event>, RelayError>>` (kind-3 from write relays) and `relay_list_fetch: Fn(PublicKey) -> Fut<...>` (the on-demand curated kind:10002 fetch). Both mirror the existing `union_fetch` injection seam.
- The None arm now calls `fallback_recover` (a new private helper) when `fallback_enabled`: (1) `lookup_write_relays`; (2) if empty, the PLAIN curated `relay_list_fetch` -> `resolve_relay_list` -> `apply_relay_list` (the sole persist-on-kind:10002-winner-seen hook in the phase; NOT routed through the kind-3 fallback — no recursion), then re-read write relays; (3) `sort_by(health.score desc)` + `truncate(nip65_max_write_relays)`; (4) `fallback_fetch` -> one single-author `acquire_validated_lists` pass; (5) hit -> `apply_validated` + `metrics::counter!("nip65_recovered").increment(1)` (no manual `_total`), miss -> `set_fetch_status("not_found")`.
- A missing/failed on-demand kind:10002 fetch yields no write relays and falls straight to `not_found` WITHOUT calling `requeue_or_fail` — Open Question 1 (no kind-3 retry-budget consumption).
- New `ingest::relay_list::resolve_relay_list`: re-resolves a single author's winning kind:10002 from a raw event union using the unchanged `verify::accept` (verify-before-dedup, CR-01) + `replaceable::pick_winner`, then `from_event` for the r-tags. No new validation logic.
- `run_crawl` and `run_daemon_loop` call sites updated to keep the crate compiling: `fallback_enabled = false`, a fresh empty health registry, and no-op closures (live wiring deferred to 05-04).

**Task 2 — tests (`tests/nip65_fallback.rs`, `tests/common/mod.rs`):**
- `fallback_recovers_via_write_relay`: author absent on curated, present on a pre-seeded write relay -> `fetched`, 2 edges, applied==1.
- `fallback_miss_stamps_not_found`: miss on curated AND known write relay -> terminal `not_found`, 0 edges.
- `unknown_write_relays_no_kind10002_stamps_not_found`: no stored relays + empty on-demand kind:10002 -> `not_found` with `fetch_attempts == 0` (proves no retry-budget consumption).
- `on_demand_kind10002_resolves_then_recovers`: on-demand curated kind:10002 resolves+persists the write relay, then recovers the kind-3 (sole persist hook proven end-to-end; r-tags round-trip via `extract_relay_pairs`).
- `no_deadlock_single_permit` remains `#[ignore]` (lands in 05-04).
- Added `common::dt(secs)` `DateTime<Utc>` fixture helper for relay-list `seen_at`.

## Verification

- `SQLX_OFFLINE=true cargo build --tests` — green (no `.sqlx` regen needed; no new `query!` macro — `resolve_relay_list` reuses existing store fns).
- `SQLX_OFFLINE=true cargo test --test nip65_fallback -- --test-threads=2` — 4 passed, 1 ignored (`no_deadlock_single_permit`).
- `SQLX_OFFLINE=true cargo test --test daemon_loop --test frontier -- --test-threads=2` — 16 passed (no regression from the call-site signature changes).
- `cargo clippy --lib` — clean.
- Grep gates: no `use nostr_sdk::Client` in apply.rs; `fallback_fetch` present; `nip65_recovered` un-suffixed; `requeue_or_fail` appears only in the transient-RelayError batch path, never in the fallback/None arm.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Updated run_crawl + run_daemon_loop call sites to keep the crate compiling**
- **Found during:** Task 1
- **Issue:** Extending `process_batch`'s signature broke its two existing call sites (`crawl/mod.rs`, `daemon/loop_.rs`). The plan scopes only apply.rs/ingest/tests, but the crate must compile.
- **Fix:** Both call sites pass `fallback_enabled = false`, a fresh empty `RelayHealthRegistry`, and no-op fallback closures — behavior is identical to before (neither path exercised the fallback). The live health-driven fallback wiring (real closures + config + the daemon registry) is 05-04's `affects` item.
- **Files modified:** src/crawl/mod.rs, src/daemon/loop_.rs
- **Commit:** 0a2248e

**2. [Rule 2 - Missing helper] Added the on-demand resolve helper `resolve_relay_list` to ingest, plus a `dt` test fixture**
- **Found during:** Task 1 / Task 2
- **Issue:** The plan calls for "any small ingest-side helper needed to convert the on-demand kind:10002 ValidatedRelayList into pairs". `ingest_events` discards r-tags, so a single-author winner re-resolution that KEEPS the event was required; tests also needed a `DateTime<Utc>` builder for relay-list `seen_at`.
- **Fix:** `ingest::relay_list::resolve_relay_list` (reuses `verify::accept` + `replaceable::pick_winner` — no new validation logic) and `tests/common::dt`.
- **Files modified:** src/ingest/relay_list.rs, tests/common/mod.rs
- **Commit:** 0a2248e (helper), dbdcbf1 (dt)

## Known Stubs

None. The `fallback_enabled=false` + no-op closures at the two call sites are an intentional, documented hand-off to 05-04 (recorded in the `affects` frontmatter), not a stub blocking this plan's goal — the RELAY-05 recovery logic itself is fully implemented and proven over a real DB.

## Self-Check: PASSED

All created/modified files present on disk; both task commits (0a2248e, dbdcbf1) exist in git history.
