---
phase: 02-relay-acquisition-validation
verified: 2026-06-13T09:15:00Z
status: gaps_found
score: 8/9 must-haves verified
overrides_applied: 0
re_verification:
  previous_status: gaps_found
  previous_score: 7/9
  gaps_closed:
    - "WR-03 residual (pool_label as rate-limiter key): relay_url now threaded explicitly through fetch_complete / fetch_complete_with_timeout from acquire_validated_lists_client; pool_label demoted to diagnostics-only string in FetchTimeout message; two_pooled_relays_get_independent_limiter_keys test asserts active_relay_count()==2 and has_limiter per individual url, not joined string"
  gaps_remaining:
    - "CR-01-new (no-newer-event boundary stall undetected): the prev_until stall guard fires only on the SECOND visit to a pinned until=T, but when all events in the pool share the boundary second with no newer event, Window 1 sets prev_until=Some(now) != Some(T), so Window 2's zero-new-id result is misclassified as genuine exhaustion and breaks Ok — C(T) silently lost. The shipped test covers only the 2-visit path (includes N(T+1) so prev_until reaches Some(T) before the stall); the no-newer-event scenario is untested and undetected."
  regressions: []
gaps:
  - truth: "A capped window pages back completely — NO events are silently lost when a deterministic newest-first relay re-serves the same cap-sized prefix for a pinned until=T"
    status: failed
    reason: |
      The prev_until stall guard in paginate_chunk (lines 151-159) requires
      prev_until == Some(current_until) to fire — a condition that can only be
      satisfied on the SECOND or later visit to a given until=T. prev_until is set
      to Some(current_until) only when new_ids > 0 AND page_back returns Some(next)
      (line 167). This means the stall fires on iteration N+1 only when iteration N
      contributed at least one new event (allowing prev_until to advance to Some(T)).

      CONFIRMED SILENT-TRUNCATION PATH (traced against actual code):
        cap=2, pool=[A(T), B(T), C(T)], no newer event.
        Iter 1 (until=now): fetch returns [A(T), B(T)]. new_ids=2, capped.
          page_back returns Some(T). until=T. prev_until=Some(now).
        Iter 2 (until=T): fetch returns [A(T), B(T)]. new_ids=0.
          Stall check: returned(2)>=cap(2)=YES, prev_until(Some(now))==Some(T)=FALSE.
          Loop breaks. Ok([A,B]) returned. C(T) silently lost.

      The test deterministic_boundary_stall_surfaces_error avoids this gap by
      seeding pool=[N(T+1), A(T), B(T), C(T)]: Window 1 returns [N(T+1), A(T)],
      Window 2 returns [A(T), B(T)] with new_ids=1 allowing prev_until to reach
      Some(T), Window 3 then triggers the stall. The no-newer-event path is
      untested and undetected in the shipped code.

      Real-world occurrence: clock-synchronized clients writing kind-3 events
      simultaneously; migration artifacts; a relay re-serving a batch of archived
      events with identical created_at; any crawl that begins at the exact second
      as the oldest event in the follow list.
    artifacts:
      - path: "src/relay/fetch.rs"
        issue: "paginate_chunk stall check (lines 151-159): prev_until==Some(current_until) is FALSE on the first visit to a boundary second when all pool events share that second (prev_until is Some(now), current_until is T). The fix must detect the stall on any capped zero-new-id window where page_back would return the same until as current_until — regardless of how many times until has been visited."
      - path: "tests/pagination.rs"
        issue: "deterministic_boundary_stall_surfaces_error (lines 164-218): seeds the pool with a newer event N(T+1), which ensures prev_until reaches Some(T) before the stall. A companion test with no newer event (all events at T) is missing and would FAIL against current code."
    missing:
      - "Fix paginate_chunk stall detection to fire on the FIRST capped zero-new-id visit to a boundary second, not only the second. One approach: when new_ids==0 AND returned>=cap, check whether page_back(returned, cap, oldest)==Some(current_until) (i.e. page_back would return the same until); if so, the relay is already pinned on this very first re-request — return Err immediately. This removes the prev_until dependency for stall detection and catches the no-newer-event case."
      - "Add a test with a pool containing no event newer than the boundary second (e.g. pool=[A(T), B(T), C(T)] with no N(T+1)) and assert the result is Err, not a silent truncated Ok([A,B])."
human_verification:
  - test: "Live-relay politeness verification"
    expected: "Sustained run against the curated relay set shows <= 4 requests per second PER RELAY independently (not shared across the pool); rate-limited notices produce escalating backoff delays visible in logs. The WR-03 fix (per-relay key threading) is now in place so each relay should have its own independent GCRA limiter observable as separate throttling behavior."
    why_human: "Cannot verify per-relay throttling rates or notice-driven backoff without a live relay connection and time-series observation of outbound REQ rates per relay."
---

# Phase 2: Relay Acquisition & Validation Re-Verification Report (Run 3)

**Phase Goal:** The crawler can pull kind-3 and kind:10002 events from a curated relay set politely and completely, and only correct, deduplicated, newest-wins follow lists emerge from the acquisition half.
**Verified:** 2026-06-13T09:15:00Z
**Status:** gaps_found
**Re-verification:** Yes — after gap-closure plans 02-10 and 02-11

## Context

Plans 02-10 and 02-11 closed the two blockers remaining after the previous re-verification:

- **02-11 (WR-03 residual / RELAY-04):** CONFIRMED CLOSED. `relay_url` is now threaded explicitly from `acquire_validated_lists_client` through `fetch_complete` and `fetch_complete_with_timeout` as an explicit parameter. `pool_label` is demoted to a diagnostics-only string folded into the FetchTimeout message. `active_relay_count()` and `has_limiter()` introspection methods added. `two_pooled_relays_get_independent_limiter_keys` test asserts two relays mint two independent keys, not one joined-pool-string key.

- **02-10 (CR-03 residual / RELAY-03):** PARTIALLY CLOSED. The `prev_until` stall tracker fires correctly in the test scenario (which includes a newer event N(T+1)), but the 02-REVIEW.md's new CR-01 finding identifies a remaining gap: the no-newer-event scenario where all pool events share the boundary second. This was independently confirmed by direct code trace below.

## CR-01 Independent Verification

The 02-REVIEW.md raised a correctness claim about `paginate_chunk` in `src/relay/fetch.rs`. This verification independently traced the scenario against the actual code.

**Scenario under test:** `cap=2`, pool events = `[A(T), B(T), C(T)]`, no event at any second newer than T.

**Code trace against lines 96-168 of `src/relay/fetch.rs`:**

```
Initialization:
  out=[], seen={}, until=now(), prev_until=None, pages=0

Iteration 1 (current_until=now):
  filter.until=now, fetch returns [A(T), B(T)] (cap=2 prefix of pool clamped to now)
  returned=2, oldest=Some(T), new_ids=2 (A and B inserted into seen)
  new_ids==0? NO → skip stall check
  page_back(2, 2, Some(T)) → Some(T); until=T
  prev_until = Some(now)   [line 167]

Iteration 2 (current_until=T):
  filter.until=T, fetch returns [A(T), B(T)] (same cap=2 prefix clamped to T)
  returned=2, oldest=Some(T), new_ids=0 (both A and B already in seen)
  new_ids==0? YES → stall check (lines 152-158):
    returned(2) >= cap(2): TRUE
    prev_until(Some(now)) == Some(current_until=T): FALSE  ← now != T
  → stall NOT detected → break [line 159]
  → return Ok([A(T), B(T)])
  C(T) is SILENTLY LOST.
```

**Verdict: CR-01 is CONFIRMED.** The code trace exactly reproduces the scenario the review describes. The stall guard fires only on the second visit to a pinned until=T. When no newer event exists in the pool, prev_until is Some(now) when current_until becomes T, making the comparison false. The loop breaks with Ok and silently truncates the follow list.

**Why the existing test does not cover this:**

`deterministic_boundary_stall_surfaces_error` seeds `pool = [N(T+1), A(T), B(T), C(T)]`. Because N is at T+1:
- Window 1 (until=now): returns [N(T+1), A(T)]. oldest=T, capped. until=T. **prev_until=Some(now).**
- Window 2 (until=T): returns [A(T), B(T)]. new_ids=1 (B is new). still capped. **prev_until=Some(T)** (updated at end of iteration 2).
- Window 3 (until=T): returns [A(T), B(T)]. new_ids=0. Check: prev_until(Some(T))==Some(T)=TRUE → Err surfaced.

The presence of N(T+1) ensures prev_until advances to Some(T) before the stall window. Without it, prev_until stays at Some(now) and the stall is not detected.

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|---------|
| 1 | Forged/invalid-sig event rejected before acceptance and counted | VERIFIED | verify::accept gates seen.insert in ingest/mod.rs; id_squat.rs test passes |
| 2 | Wrong kind/author event dropped and counted | VERIFIED | verify::accept checks kind==want_kind && pubkey in requested; verify_gate tests pass |
| 3 | Same verified event id from multiple relays processed at most once | VERIFIED | HashSet<EventId> seen-set; verify::accept gates seen.insert; id_squat test RED commit 62190dc load-bearing |
| 4 | Future-dated event beyond clamp rejected; newest-wins; same-ts tie → lowest id | VERIFIED | replaceable::pick_winner: future_clamp_secs saturating cutoff, max_by+then_with; 4 replaceable tests pass |
| 5 | Malformed p-tags skipped; oversized follow list bounded without panicking | VERIFIED | follow_list::followee_pubkeys uses Tags::public_keys(); reject-not-truncate on cap; follow_list_bounds tests pass |
| 6 | kind:10002 events resolved under identical replaceable rules | VERIFIED | pick_winner is kind-agnostic; relay_list.rs 2/2 pass |
| 7 | Crawler connects curated relay set with reconnect; exponential backoff + jitter | VERIFIED | connect_curated wired; backoff_delay_unjittered: failures>=64 early return cap; saturation test sweeps 64..=127 |
| 8 | NIP-11 limits read/cached; max_limit clamped; missing docs → defaults; timeout + body bound | VERIFIED | LazyLock<reqwest::Client> with .timeout(10s)/.connect_timeout(5s); MAX_NIP11_BYTES; MAX_ADVERTISED_LIMIT=5000; 9 nip11_limits tests pass |
| 9 | Per-relay rate limiting throttles requests; pool key is per-relay, not pool-wide | VERIFIED | fetch_complete/fetch_complete_with_timeout take explicit relay_url param (threaded from acquire_validated_lists_client line 227); pool_label used only in FetchTimeout message string for diagnostics (line 327); paginate_chunk_gated keyed on relay_url (line 332); two_pooled_relays_get_independent_limiter_keys: active_relay_count()==2, has_limiter("wss://r1.example") and has_limiter("wss://r2.example") both true, has_limiter("wss://r1.example, wss://r2.example") false |
| 10 | Capped result triggers another page; EOSE never treated as completeness; no silent event loss | FAILED | paginate_chunk stall detection (lines 151-159) misses the no-newer-event case: prev_until==Some(now) on Window 2, current_until=T, comparison is false → breaks Ok silently dropping C(T). Confirmed by direct code trace (see above). deterministic_boundary_stall_surfaces_error covers the 2-visit path (N(T+1) present) but not the 1-visit path (no newer event). |

**Score:** 8/9 must-haves verified

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
| `src/relay/fetch.rs` | relay_url param threaded; pool_label diagnostics-only; stall detection for deterministic-relay boundary exhaustion | PARTIAL | relay_url param added to fetch_complete/fetch_complete_with_timeout (line 267, 308); pool_label is diagnostics-only in FetchTimeout message (lines 324, 327, 332). BUT stall detection (lines 151-159) misses no-newer-event scenario — prev_until stays Some(now) on boundary second entry, comparison fails, silent truncation |
| `tests/acquire_pipeline.rs` | E2E test | VERIFIED | 1/1 passes |
| `tests/pagination.rs` | RELAY-03 pagination including stall detection | PARTIAL | 8/8 pass, but deterministic_boundary_stall_surfaces_error only covers 2-visit path (pool has N(T+1)); no-newer-event (all events at T) path untested and would FAIL |
| `tests/fetch_timeout.rs` | FetchTimeout requeue test | VERIFIED | 2/2 pass; FetchTimeout label now includes per-relay url enriched with pool diagnostics |
| `tests/id_squat.rs` | id-squat attack test | VERIFIED | 1/1 passes |
| `tests/production_wiring.rs` | Per-relay limiter keying test | VERIFIED | 6/6 pass; two_pooled_relays_get_independent_limiter_keys asserts two independent keys; joined-pool-string key absence asserted |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `src/relay/mod.rs` | `src/relay/fetch.rs fetch_complete` | `relay_url` threaded as explicit param | WIRED | Line 227: `fetch::fetch_complete(client, relay_url, ...)` — individual relay_url passed, not re-derived from pool state |
| `src/relay/fetch.rs fetch_complete_with_timeout` | `src/relay/rate_limit.rs acquire()` | `relay_url` param (not pool_label) | WIRED | Line 332: `paginate_chunk_gated(..., registry, relay_url, ...)` — pool_label is diagnostics only (lines 324, 327) |
| `src/relay/mod.rs` | `src/ingest/mod.rs ingest_events` | acquire_validated_lists | WIRED | ingest::ingest_events called; all events route through it |
| `src/relay/mod.rs` | `src/relay/nip11.rs LimitCache::get_or_fetch` | `limit_cache.get_or_fetch(relay_url)` | WIRED | Line 220 sources max_limit from cache; correct relay_url used |
| `src/relay/fetch.rs paginate_chunk_gated` | `src/relay/rate_limit.rs acquire()` | `registry.acquire(relay_url)` before each REQ | WIRED | Line 208; relay_url from caller |
| `src/relay/mod.rs` | `src/relay/rate_limit.rs record_notice` | spawn_notice_consumer -> handle_relay_message | WIRED | handle_relay_message calls registry.record_notice; spawn_notice_consumer wired |
| `tests/production_wiring.rs` | `src/relay/rate_limit.rs active_relay_count/has_limiter` | two_pooled_relays_get_independent_limiter_keys | WIRED | Test asserts 2 keys, per-url presence, joined-string absence |

### Data-Flow Trace (Level 4)

| Artifact | Data Variable | Source | Produces Real Data | Status |
|----------|---------------|--------|--------------------|--------|
| `src/relay/rate_limit.rs` | per-relay GCRA token | registry.acquire(relay_url) via paginate_chunk_gated | Yes — called before each window REQ, keyed on individual relay_url | FLOWING |
| `src/relay/nip11.rs` | max_limit | limit_cache.get_or_fetch(relay_url) in acquire_validated_lists_client | Yes — per-relay cache wired | FLOWING |
| `src/ingest/mod.rs` | ValidatedFollowList | ingest_events after verify->dedup->replaceable->follow_list | Yes — wired and tested | FLOWING |
| `src/relay/mod.rs` | failure_count escalation | record_notice via spawn_notice_consumer | Yes — consumer spawned, handle_relay_message wired | FLOWING |

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| Full test suite | `cargo test --tests` | All 19 test suites pass, 0 failed | PASS |
| pool_label NOT used as limiter key | `grep -n "pool_label" src/relay/fetch.rs` | pool_label at line 324 assigned to `pool_diag`; used only in FetchTimeout message string (line 327); `paginate_chunk_gated` at line 332 uses `relay_url` param | PASS |
| relay_url param present in fetch_complete signatures | `grep -n "relay_url" src/relay/fetch.rs` | relay_url: &str in both fetch_complete (line 269) and fetch_complete_with_timeout (line 310); used at paginate_chunk_gated call (line 332) | PASS |
| relay_url threaded from mod.rs into fetch_complete | `grep -n "fetch_complete" src/relay/mod.rs` | Line 227: `fetch::fetch_complete(client, relay_url, authors, want_kind, max_limit, max_authors, registry)` | PASS |
| Stall detection covers no-newer-event scenario | `grep -n "prev_until" src/relay/fetch.rs` + code trace | prev_until guard (lines 151-159): `prev_until==Some(current_until)` only true on SECOND visit. No-newer-event scenario: prev_until=Some(now), current_until=T, comparison false → silent break | FAIL |
| Stall test covers no-newer-event pool | `grep -n "no.newer\|no newer\|all.at.T" tests/pagination.rs` | No such test exists; deterministic_boundary_stall_surfaces_error uses pool with N(T+1) | FAIL |
| two_pooled_relays test asserts independent keys | `grep -n "active_relay_count\|has_limiter" tests/production_wiring.rs` | Lines 197-207: asserts count==2, has_limiter(r1_url), has_limiter(r2_url), !has_limiter(joined) | PASS |

### Probe Execution

Step 7c: No probe scripts found in `scripts/*/tests/probe-*.sh`. Skipped.

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|-------------|-------------|--------|---------|
| RELAY-01 | 02-01, 02-03, 02-08 | Reconnect + exponential backoff + jitter | VERIFIED | connect_curated wired; backoff_delay_unjittered failures>=64 early-return cap; saturation test passes |
| RELAY-02 | 02-01, 02-03, 02-07, 02-09 | NIP-11 limits read and respected in production | VERIFIED | LimitCache.get_or_fetch wired; MAX_NIP11_BYTES + MAX_ADVERTISED_LIMIT; NIP11_CLIENT with timeouts; 9 nip11_limits tests pass |
| RELAY-03 | 02-03, 02-04, 02-05, 02-10 | Pagination + EOSE never trusted as complete | PARTIAL | paginate_chunk + page_back inclusive implemented; MAX_PAGES_PER_CHUNK guards adversarial relay; FetchTimeout on elapsed timeout; prev_until stall detection added but misses no-newer-event scenario (silent truncation confirmed by code trace) |
| RELAY-04 | 02-03, 02-08, 02-09, 02-11 | Per-relay rate limiting + notice backoff in production | VERIFIED | relay_url threaded through fetch_complete/fetch_complete_with_timeout; pool_label diagnostics-only; two_pooled_relays_get_independent_limiter_keys asserts 2 keys; spawn_notice_consumer + handle_relay_message wired |
| INGEST-01 | 02-02 | Sig verification + count before accept | VERIFIED | verify::accept with Event::verify() + metrics |
| INGEST-02 | 02-02, 02-06 | Duplicate ids processed once, dedup AFTER verify | VERIFIED | HashSet<EventId> seen-set; verify::accept gates seen.insert; id_squat test load-bearing |
| INGEST-03 | 02-02 | Future-clamp + newest-wins + lowest-id tie-break | VERIFIED | pick_winner with saturating clamp + max_by+then_with |
| INGEST-04 | 02-02 | Malformed p-tags skipped; oversized list bounded | VERIFIED | public_keys() skip + reject-not-truncate |
| INGEST-05 | 02-02 | kind:10002 under same replaceable rules | VERIFIED | pick_winner is kind-agnostic; relay_list.rs tests pass |

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| `src/relay/fetch.rs` | 151-159 | `prev_until==Some(current_until)` stall guard fires only on 2nd visit to pinned until=T | BLOCKER | No-newer-event scenario: all events at T, pool larger than cap, Window 2 sets new_ids=0 with prev_until=Some(now)!=Some(T) → silent Ok, C(T) lost. Core completeness invariant violated. |
| `tests/pagination.rs` | 164-218 | `deterministic_boundary_stall_surfaces_error` seeds pool with N(T+1) | WARNING | Test covers the 2-visit path but not the 1-visit (no-newer-event) path; the boundary gap it was designed to prevent remains undetected for that scenario |

No TBD/FIXME/XXX markers found in files modified by this phase.

### Human Verification Required

#### 1. Live-Relay Politeness Verification

**Test:** Run `acquire_validated_lists_client` against two real curated relays simultaneously for 60+ seconds; observe per-relay REQ rates and response to NOTICE messages.
**Expected:** Each relay throttled independently at <= 4 req/sec (GCRA per-relay quota), not a shared pool-wide 4 req/sec. Rate-limited notices produce escalating backoff delays in logs, visible per-relay rather than collapsed across the pool.
**Why human:** Cannot verify per-relay throttling rates or notice-driven backoff without live relay connections and time-series observation. The WR-03 fix (relay_url threading) is in place, so the expected behavior should now be correct individual per-relay quotas.

## Gaps Summary

**WR-03 is CLOSED (RELAY-04 restored):**

Plans 02-10 and 02-11 correctly threaded `relay_url` through `fetch_complete` / `fetch_complete_with_timeout` from `acquire_validated_lists_client`. `pool_label` is now diagnostics-only. `two_pooled_relays_get_independent_limiter_keys` is a load-bearing regression test proving two relays get two independent limiter keys. The production wiring path is now correctly per-relay from end to end.

**BLOCKER: CR-01-new (boundary-stall no-newer-event path — RELAY-03 still partial):**

The `prev_until` stall detection added by plan 02-10 closes the stall for the scenario where at least one newer event exists in the pool (so prev_until advances to Some(T) before the stall fires on the third window). However, the 02-REVIEW.md correctly identifies that when ALL events in the pool share the boundary second (no event at any newer second than T), the stall is not detected:

- Window 1 (until=now): returns [A(T), B(T)]. Capped. page_back returns Some(T). `prev_until = Some(now)`.
- Window 2 (until=T): returns [A(T), B(T)] again. `new_ids=0`. Stall check: `prev_until(Some(now)) == Some(current_until=T)` → FALSE. Loop breaks. `Ok([A,B])`. C(T) lost.

The code trace confirms: since `now` (a current wall-clock timestamp) is never equal to `T` (the boundary event second, typically in the past), `prev_until` from iteration 1 never equals `current_until` from iteration 2. The stall guard requires a second visit to the boundary second with `prev_until` already set to `Some(T)`, which only happens if iteration 2 contributed at least one new event.

This is a confirmed silent-truncation path on the project's core completeness invariant ("each follow list fetched completely; never silently drop pubkeys"). The fix in the review's CR-01 suggestion is straightforward: when `new_ids==0 AND returned>=cap`, check whether `page_back(returned, cap, oldest)==Some(current_until)` — if so, the relay is already pinned on this first re-request and the stall is certain, return Err immediately.

The ingest gate (INGEST-01..05) is sound, well-tested, and confirmed closed. RELAY-04 (per-relay rate limiting) is now correctly wired and confirmed closed. Five of nine requirements are fully satisfied. RELAY-03 remains partially open due to this confirmed gap.

---

_Verified: 2026-06-13T09:15:00Z_
_Verifier: Claude (gsd-verifier)_
