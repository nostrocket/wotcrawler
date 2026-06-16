---
phase: 02-relay-acquisition-validation
verified: 2026-06-13T12:00:00Z
status: passed
score: 9/9 must-haves verified
human_verification_result: passed (Live-relay politeness verified via 02-UAT.md on 2026-06-13)
overrides_applied: 0
re_verification:
  previous_status: gaps_found
  previous_score: 8/9
  gaps_closed:
    - "CR-01-new (no-newer-event boundary stall undetected): paginate_chunk stall detection now fires on the FIRST capped zero-new-id re-request by OR-combining page_back(returned, cap, oldest) == Some(current_until) with the existing prev_until 2-visit guard. The no-newer-event companion test (no_newer_event_boundary_stall_surfaces_error) passes (Err surfaced) and deterministic_boundary_stall_surfaces_error stays green. zero_new_id_window_stops_even_when_capped renamed to capped_reserved_prefix_at_pinned_boundary_surfaces_error with expectation updated from Ok to Err (that test itself was an instance of the silent-truncation bug)."
  gaps_remaining: []
  regressions: []
human_verification:
  - test: "Live-relay politeness verification"
    expected: "Sustained run against the curated relay set shows <= 4 requests per second PER RELAY independently (not shared across the pool); rate-limited notices produce escalating backoff delays visible in logs. The WR-03 fix (per-relay key threading) is in place so each relay should have its own independent GCRA limiter observable as separate throttling behavior."
    why_human: "Cannot verify per-relay throttling rates or notice-driven backoff without a live relay connection and time-series observation of outbound REQ rates per relay."
---

# Phase 2: Relay Acquisition & Validation Re-Verification Report (Run 4)

**Phase Goal:** The crawler can pull kind-3 and kind:10002 events from a curated relay set politely and completely, and only correct, deduplicated, newest-wins follow lists emerge from the acquisition half.
**Verified:** 2026-06-13T12:00:00Z
**Status:** human_needed
**Re-verification:** Yes — after gap-closure plan 02-12 (CR-01-new / RELAY-03 no-newer-event boundary stall)

## Context

Plan 02-12 closed the single remaining BLOCKER gap from the previous verification (Run 3, score 8/9). The prior gap — the no-newer-event boundary stall in `paginate_chunk` where `prev_until == Some(now) != Some(T)` on the second window caused silent truncation — is now closed by OR-combining a page_back re-pin check with the existing 2-visit guard. All 9 must-have truths are now verified.

## CR-01-new Fix Verification

**Scenario under test:** cap=2, pool=[A(T), B(T), C(T)], no event newer than T.

**Code trace against the fixed paginate_chunk (lines 161-173 of src/relay/fetch.rs):**

```
Iteration 1 (current_until=now):
  fetch returns [A(T), B(T)]; new_ids=2; page_back(2,2,Some(T))=Some(T); until=T; prev_until=Some(now).

Iteration 2 (current_until=T):
  fetch returns [A(T), B(T)]; new_ids=0.
  Check: returned(2) >= cap(2): TRUE.
    prev_until(Some(now)) == Some(T): FALSE.
    page_back(2,2,Some(T)) == Some(T) == Some(current_until): TRUE.
  OR-clause fires -> Err("boundary-second stall: ..."). C(T) is NOT silently lost.
```

**Verdict: CR-01-new is CLOSED.** The first-visit stall detector (`page_back(returned, cap, oldest) == Some(current_until)`) catches the no-newer-event case on the second window — the same window that previously broke to `Ok` silently dropping C(T). The `no_newer_event_boundary_stall_surfaces_error` test was confirmed RED before the fix and GREEN after (`result.is_err()` asserted, plus `pinned_at_t >= 1`).

**Genuine exhaustion preserved:**
- Short window (returned < cap): `page_back` returns None; OR-clause false; breaks Ok. (tests: `short_first_window_does_not_page`, `cross_window_dedup_keeps_each_event_once` window3)
- Window whose oldest is older than current_until: `page_back` returns `Some(older) != Some(current_until)`; OR-clause false; breaks Ok. (test: `capped_first_window_triggers_second_page` window2 at 3000 < until=4000)

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|---------|
| 1 | Forged/invalid-sig event rejected before acceptance and counted | VERIFIED | verify::accept gates seen.insert in ingest/mod.rs; id_squat.rs test passes |
| 2 | Wrong kind/author event dropped and counted | VERIFIED | verify::accept checks kind==want_kind && pubkey in requested; verify_gate tests pass |
| 3 | Same verified event id from multiple relays processed at most once | VERIFIED | HashSet<EventId> seen-set; verify::accept gates seen.insert; id_squat test RED commit 62190dc load-bearing |
| 4 | Future-dated event beyond clamp rejected; newest-wins; same-ts tie to lowest id | VERIFIED | replaceable::pick_winner: future_clamp_secs saturating cutoff, max_by+then_with; 4 replaceable tests pass |
| 5 | Malformed p-tags skipped; oversized follow list bounded without panicking | VERIFIED | follow_list::followee_pubkeys uses Tags::public_keys(); reject-not-truncate on cap; follow_list_bounds tests pass |
| 6 | kind:10002 events resolved under identical replaceable rules | VERIFIED | pick_winner is kind-agnostic; relay_list.rs 2/2 pass |
| 7 | Crawler connects curated relay set with reconnect; exponential backoff + jitter | VERIFIED | connect_curated wired; backoff_delay_unjittered: failures>=64 early return cap; saturation test sweeps 64..=127 |
| 8 | NIP-11 limits read/cached; max_limit clamped; missing docs yield defaults; timeout + body bound | VERIFIED | LazyLock<reqwest::Client> with .timeout(10s)/.connect_timeout(5s); MAX_NIP11_BYTES; MAX_ADVERTISED_LIMIT=5000; 9 nip11_limits tests pass |
| 9 | Per-relay rate limiting throttles requests; pool key is per-relay, not pool-wide | VERIFIED | fetch_complete/fetch_complete_with_timeout take explicit relay_url param (threaded from acquire_validated_lists_client line 227); pool_label used only in FetchTimeout message string for diagnostics (line 337); paginate_chunk_gated keyed on relay_url (line 345); two_pooled_relays_get_independent_limiter_keys: active_relay_count()==2, has_limiter("wss://r1.example") and has_limiter("wss://r2.example") both true |

**Truth #10 (RELAY-03 completeness — collapsed into truth #9/the phase goal):**

The previous verification split RELAY-03 completeness into a 10th truth. With CR-01-new closed, no-event-loss at cap boundaries is now fully verified. The RELAY-03 row in the requirements table below carries the combined evidence.

**Score:** 9/9 truths verified

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `src/ingest/verify.rs` | Sig + kind/author gate | VERIFIED | accept() implemented; Event::verify(); metrics counters present |
| `src/ingest/replaceable.rs` | Clamp + newest-wins + tie-break | VERIFIED | pick_winner with saturating clamp, max_by+then_with |
| `src/ingest/follow_list.rs` | p-tag extraction + dedup + cap | VERIFIED | public_keys() skip, self-drop, reject-not-truncate |
| `src/ingest/mod.rs` | Orchestrator + ValidatedFollowList, dedup-after-verify | VERIFIED | verify::accept before seen.insert; ValidatedFollowList 4 fields |
| `src/relay/mod.rs` | connect_curated + acquire seam wired to registry/cache/notices; relay_url threaded into fetch_complete | VERIFIED | line 227: `fetch::fetch_complete(client, relay_url, ...)` — individual relay_url threaded correctly |
| `src/relay/nip11.rs` | NIP-11 fetch with timeout + body bound + LazyLock client + MAX_ADVERTISED_LIMIT | VERIFIED | NIP11_CLIENT LazyLock with .timeout(10s)/.connect_timeout(5s); MAX_NIP11_BYTES; clamp_max_limit; 9 tests pass |
| `src/relay/rate_limit.rs` | Governor limiter with Arc-per-relay, backoff saturation, active_relay_count/has_limiter introspection | VERIFIED | Arc<DirectLimiter> in map; failures>=64 early cap; active_relay_count() and has_limiter() present at lines 252-264 |
| `src/relay/fetch.rs` | relay_url param threaded; pool_label diagnostics-only; stall detection covers both 2-visit AND no-newer-event first-visit paths | VERIFIED | relay_url param in fetch_complete/fetch_complete_with_timeout; pool_label diagnostics-only in pool_diag variable (line 337); stall condition at lines 161-173: `returned >= cap && (prev_until == Some(current_until) \|\| page_back(returned, cap, oldest) == Some(current_until))`; 9/9 pagination tests pass |
| `tests/acquire_pipeline.rs` | E2E test | VERIFIED | 1/1 passes |
| `tests/pagination.rs` | RELAY-03 pagination including both stall paths | VERIFIED | 9/9 pass: no_newer_event_boundary_stall_surfaces_error (new, covers CR-01-new first-visit path), deterministic_boundary_stall_surfaces_error (2-visit path, no regression), capped_reserved_prefix_at_pinned_boundary_surfaces_error (reconciled from zero_new_id_window, now expects Err), plus 6 other pagination tests |
| `tests/fetch_timeout.rs` | FetchTimeout requeue test | VERIFIED | 2/2 pass |
| `tests/id_squat.rs` | id-squat attack test | VERIFIED | 1/1 passes |
| `tests/production_wiring.rs` | Per-relay limiter keying test | VERIFIED | 6/6 pass; two_pooled_relays_get_independent_limiter_keys asserts two independent keys; joined-pool-string key absence asserted |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `src/relay/mod.rs` | `src/relay/fetch.rs fetch_complete` | `relay_url` threaded as explicit param | WIRED | Line 227: `fetch::fetch_complete(client, relay_url, ...)` — individual relay_url passed |
| `src/relay/fetch.rs fetch_complete_with_timeout` | `src/relay/rate_limit.rs acquire()` | `relay_url` param (not pool_label) | WIRED | Line 345: `paginate_chunk_gated(..., registry, relay_url, ...)` — pool_label is diagnostics only |
| `src/relay/mod.rs` | `src/ingest/mod.rs ingest_events` | acquire_validated_lists | WIRED | ingest::ingest_events called; all events route through it |
| `src/relay/mod.rs` | `src/relay/nip11.rs LimitCache::get_or_fetch` | `limit_cache.get_or_fetch(relay_url)` | WIRED | Line 220 sources max_limit from cache; correct relay_url used |
| `src/relay/fetch.rs paginate_chunk_gated` | `src/relay/rate_limit.rs acquire()` | `registry.acquire(relay_url)` before each REQ | WIRED | Line 221; relay_url from caller |
| `src/relay/mod.rs` | `src/relay/rate_limit.rs record_notice` | spawn_notice_consumer -> handle_relay_message | WIRED | handle_relay_message calls registry.record_notice; spawn_notice_consumer wired |
| `src/relay/fetch.rs paginate_chunk zero-new-id branch` | `src/relay/fetch.rs page_back` | `page_back(returned, cap, oldest) == Some(current_until)` in stall check | WIRED | Line 164: first-visit stall detector confirmed by grep and test execution |
| `tests/production_wiring.rs` | `src/relay/rate_limit.rs active_relay_count/has_limiter` | two_pooled_relays_get_independent_limiter_keys | WIRED | Test asserts 2 keys, per-url presence, joined-string absence |

### Data-Flow Trace (Level 4)

| Artifact | Data Variable | Source | Produces Real Data | Status |
|----------|---------------|--------|--------------------|--------|
| `src/relay/rate_limit.rs` | per-relay GCRA token | registry.acquire(relay_url) via paginate_chunk_gated | Yes — called before each window REQ, keyed on individual relay_url | FLOWING |
| `src/relay/nip11.rs` | max_limit | limit_cache.get_or_fetch(relay_url) in acquire_validated_lists_client | Yes — per-relay cache wired | FLOWING |
| `src/ingest/mod.rs` | ValidatedFollowList | ingest_events after verify->dedup->replaceable->follow_list | Yes — wired and tested | FLOWING |
| `src/relay/mod.rs` | failure_count escalation | record_notice via spawn_notice_consumer | Yes — consumer spawned, handle_relay_message wired | FLOWING |
| `src/relay/fetch.rs` | stall detection signal | page_back(returned, cap, oldest) == Some(current_until) in paginate_chunk | Yes — fires on first capped zero-new-id re-request of pinned boundary second | FLOWING |

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| pagination suite (all 9 tests) | `cargo test --test pagination` | 9 passed, 0 failed | PASS |
| no_newer_event_boundary_stall_surfaces_error (CR-01-new) | `cargo test --test pagination no_newer_event` | ok — result.is_err() + pinned_at_t >= 1 | PASS |
| deterministic_boundary_stall_surfaces_error (2-visit path, no regression) | `cargo test --test pagination deterministic` | ok | PASS |
| capped_reserved_prefix_at_pinned_boundary_surfaces_error (reconciled) | `cargo test --test pagination capped_reserved` | ok — Err(FetchTimeout(_)) | PASS |
| First-visit stall detector present in paginate_chunk | `grep -n "page_back(returned, cap, oldest)" src/relay/fetch.rs` | Line 164: `\|\| page_back(returned, cap, oldest) == Some(current_until)` | PASS |
| pool_label NOT used as limiter key | `grep -n "pool_label\|pool_diag" src/relay/fetch.rs` | pool_label private fn; pool_diag local var used only in timeout_label string; paginate_chunk_gated called with relay_url | PASS |
| production_wiring suite | `cargo test --test production_wiring` | 6 passed, 0 failed | PASS |
| fetch_timeout suite | `cargo test --test fetch_timeout` | 2 passed, 0 failed | PASS |
| id_squat suite | `cargo test --test id_squat` | 1 passed, 0 failed | PASS |
| acquire_pipeline suite | `cargo test --test acquire_pipeline` | 1 passed, 0 failed | PASS |
| Phase 2 suites (all non-infra) | all Phase 2 test suites | All passed, 0 failed | PASS |
| Debt markers (TBD/FIXME/XXX) | `grep -rn "TBD\|FIXME\|XXX" src/ tests/` | No results | PASS |
| contract_views_present (infrastructure) | `cargo test --test contract` | FAILED — PostgreSQL container not running (Docker present but no pg container) | INFRA-SKIP |

Note on `contract_views_present`: this test belongs to Phase 1's GRAPH-04 contract (database views) and requires a running PostgreSQL Docker container. The failure is a CI infrastructure gap (Docker present but no postgres container started), not a Phase 2 regression. The test is unrelated to any file modified by plans 02-10 through 02-12 (`tests/contract.rs` has no reference to `fetch.rs` or `pagination.rs`). This failure is pre-existing and outside Phase 2 scope.

### Probe Execution

Step 7c: No probe scripts found in `scripts/*/tests/probe-*.sh`. Skipped.

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|-------------|-------------|--------|---------|
| RELAY-01 | 02-01, 02-03, 02-08 | Reconnect + exponential backoff + jitter | VERIFIED | connect_curated wired; backoff_delay_unjittered failures>=64 early-return cap; saturation test sweeps 64..=127 |
| RELAY-02 | 02-01, 02-03, 02-07, 02-09 | NIP-11 limits read and respected in production | VERIFIED | LimitCache.get_or_fetch wired; MAX_NIP11_BYTES + MAX_ADVERTISED_LIMIT; NIP11_CLIENT with timeouts; 9 nip11_limits tests pass |
| RELAY-03 | 02-03, 02-04, 02-05, 02-10, 02-12 | Pagination + EOSE never trusted as complete; no silent event loss at cap boundary | VERIFIED | paginate_chunk + page_back inclusive implemented; MAX_PAGES_PER_CHUNK guards adversarial relay; FetchTimeout on elapsed timeout; stall detection fires on BOTH first-visit (page_back re-pin) and 2-visit (prev_until) paths; 9/9 pagination tests pass including the new no_newer_event companion and reconciled capped_reserved test |
| RELAY-04 | 02-03, 02-08, 02-09, 02-11 | Per-relay rate limiting + notice backoff in production | VERIFIED | relay_url threaded through fetch_complete/fetch_complete_with_timeout; pool_label diagnostics-only; two_pooled_relays_get_independent_limiter_keys asserts 2 keys; spawn_notice_consumer + handle_relay_message wired |
| INGEST-01 | 02-02 | Sig verification + count before accept | VERIFIED | verify::accept with Event::verify() + metrics |
| INGEST-02 | 02-02, 02-06 | Duplicate ids processed once, dedup AFTER verify | VERIFIED | HashSet<EventId> seen-set; verify::accept gates seen.insert; id_squat test load-bearing |
| INGEST-03 | 02-02 | Future-clamp + newest-wins + lowest-id tie-break | VERIFIED | pick_winner with saturating clamp + max_by+then_with |
| INGEST-04 | 02-02 | Malformed p-tags skipped; oversized list bounded | VERIFIED | public_keys() skip + reject-not-truncate |
| INGEST-05 | 02-02 | kind:10002 under same replaceable rules | VERIFIED | pick_winner is kind-agnostic; relay_list.rs tests pass |

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| (none) | — | No TBD/FIXME/XXX debt markers in any Phase 2 source file | — | — |

No debt markers found. No orphaned stubs. The renamed `capped_reserved_prefix_at_pinned_boundary_surfaces_error` test (formerly `zero_new_id_window_stops_even_when_capped`) now correctly expects `Err` with a doc comment explaining the corrected invariant — this is substantive, not a stub.

### Human Verification Required

#### 1. Live-Relay Politeness Verification

**Test:** Run `acquire_validated_lists_client` against two real curated relays simultaneously for 60+ seconds; observe per-relay REQ rates and response to NOTICE messages.
**Expected:** Each relay throttled independently at <= 4 req/sec (GCRA per-relay quota), not a shared pool-wide 4 req/sec. Rate-limited notices produce escalating backoff delays in logs, visible per-relay rather than collapsed across the pool.
**Why human:** Cannot verify per-relay throttling rates or notice-driven backoff without live relay connections and time-series observation. The WR-03 fix (relay_url threading) is in place; the expected behavior should now be correct individual per-relay quotas.

## Gaps Summary

No automated gaps remain. All 9 must-have truths are verified. RELAY-03 is now fully closed — the boundary stall fires on the FIRST capped zero-new-id re-request via the `page_back(returned, cap, oldest) == Some(current_until)` check, including the no-newer-event scenario that the previous `prev_until`-based guard missed.

The single open item is live-relay politeness verification (human_needed), which was identified in Run 1 and remains unchanged: the GCRA per-relay throttling and notice-driven backoff can only be confirmed against real relay infrastructure with time-series observation. All automated evidence (unit tests, production wiring tests, relay_url threading) is now in place.

---

_Verified: 2026-06-13T12:00:00Z_
_Verifier: Claude (gsd-verifier)_
