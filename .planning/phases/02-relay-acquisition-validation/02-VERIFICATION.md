---
phase: 02-relay-acquisition-validation
verified: 2026-06-13T08:30:00Z
status: gaps_found
score: 7/9 must-haves verified
overrides_applied: 0
re_verification:
  previous_status: gaps_found
  previous_score: 6/9
  gaps_closed:
    - "CR-01 (dedup-before-verify in ingest orchestrator): verified::accept now gates seen.insert; id_squat test passes"
    - "CR-02 (FetchTimeout never constructed): fetch_window_with_deadline constructs FetchTimeout on elapsed >= timeout; 2 construction sites confirmed"
    - "CR-04 (no MAX_PAGES_PER_CHUNK budget): MAX_PAGES_PER_CHUNK = 10_000 constant defined and enforced in paginate_chunk"
    - "CR-05 (quota multiplication under concurrency): Arc<DirectLimiter> per relay; clone-and-await pattern; concurrent test passes"
    - "CR-06 (NIP-11 no timeout or body bound): LazyLock<reqwest::Client> with .timeout(10s)/.connect_timeout(5s); MAX_NIP11_BYTES stream-and-bail; 9 nip11_limits tests pass"
    - "WR-01 (backoff zero-delay at high failure counts): failures >= 64 early return cap; saturation test passes"
    - "WR-02 (no upper clamp on advertised max_limit): clamp_max_limit + MAX_ADVERTISED_LIMIT = 5000; 9 nip11_limits tests pass"
    - "WR-03 production wiring disconnected: acquire(), get_or_fetch(), record_notice() all have production callers; production_wiring.rs 5/5 pass"
  gaps_remaining:
    - "CR-03 (inclusive boundary stall against deterministic relays): partially closed — inclusive boundary is correct but the zero-new-id guard fires on a stalled until=T while genuine events remain. The review's new CR-01 re-opens this."
    - "WR-03 residual: per-relay limiter keyed on pool_label (joined string) not individual relay_url in fetch_complete_with_timeout"
  regressions: []
gaps:
  - truth: "A capped window pages back inclusively so NO events are lost when a relay caps mid-second — the completeness guarantee"
    status: failed
    reason: "The inclusive boundary (page_back returns oldest timestamp, not oldest-1) is correct per se, but the zero-new-id termination guard cannot distinguish 'boundary second exhausted' from 'relay stalled at boundary second with more events remaining'. When more than cap events share the oldest second, a deterministic newest-first relay returns the same cap-sized prefix for every request with until=T. The loop issues Window1(until=now) -> sees A(T) oldest, 2 events = cap -> until=T. Window2(until=T) -> relay returns [A(T), B(T)] -> new_ids=1 (B). Still capped -> until=T. Window3(until=T) -> relay returns [A(T), B(T)] again -> new_ids=0 -> LOOP BREAKS. C(T) is never fetched and is silently lost. The test `inclusive_boundary_keeps_boundary_event` does not catch this because ScriptedRelay is hand-fed window2=[boundary_a, boundary_b] — it scripts the relay to return the cut event, which a real deterministic relay never does. The test validates the mock, not the production invariant."
    artifacts:
      - path: "src/relay/fetch.rs"
        issue: "paginate_chunk zero-new-id guard (line 131-133) terminates the loop when until is pinned at an unexhausted boundary second. A deterministic relay returning the same prefix for repeated until=T results in new_ids=0 while genuine events remain. No stall detection exists."
      - path: "tests/pagination.rs"
        issue: "inclusive_boundary_keeps_boundary_event (lines 36-71) scripts ScriptedRelay to return the cut boundary event in window2. Real deterministic nearest-first relays return the same cap-sized prefix for a repeated until=T, not the missing sibling. Test validates mock behavior, not production invariant."
    missing:
      - "Stall detection: track prev_until across loop iterations. When returned >= cap AND until == prev_until AND new_ids == 0, this is an unresolvable stall — surface as Err(FetchTimeout(...)) or a dedicated error so the caller requeues. Never silently complete."
      - "New test using a deterministic relay mock that returns the same fixed prefix for a repeated until=T (not scripted to vary). Assert the stall surfaces as an error, not a silent return of a truncated event list."

  - truth: "Per-relay rate limiting throttles outbound requests with one GCRA quota per relay in production"
    status: failed
    reason: "fetch_complete_with_timeout derives the rate-limiter key from pool_label(client) (src/relay/fetch.rs:273), which joins ALL connected relay URLs into one string (e.g. 'wss://r1.example, wss://r2.example'). This joined string is used as the registry key for both registry.acquire() (line 278) and FetchTimeout labels. Consequences: (1) All relays in the pool share a single GCRA limiter instead of having one each — WR-03's 'per-relay quota' collapses to a shared pool quota, undermining RELAY-04. (2) When pool membership changes (a relay drops or reconnects), the joined string changes, minting a fresh full-burst limiter and discarding accrued GCRA state for the entire pool. acquire_validated_lists_client receives a real relay_url (src/relay/mod.rs:205) and correctly uses it for limit_cache.get_or_fetch and registry.reset, but does NOT thread it into fetch_complete (line 227) — so the two relay_url notions diverge within one call. The production_wiring.rs tests exercise paginate_chunk_gated directly with an explicit relay_url, bypassing fetch_complete_with_timeout; they do not test this failure mode."
    artifacts:
      - path: "src/relay/fetch.rs"
        issue: "fetch_complete_with_timeout line 273: `let relay_url = pool_label(client).await;` uses the joined pool label as the limiter key. This is documented as 'Label for FetchTimeout AND the per-relay rate-limiter key' but collapsing all relays into one key defeats per-relay throttling."
      - path: "src/relay/mod.rs"
        issue: "acquire_validated_lists_client has a real per-relay relay_url (line 205) but passes it to fetch_complete without threading it through. fetch_complete internally re-derives the wrong (pool-wide) key."
    missing:
      - "Thread the caller's relay_url through fetch_complete / fetch_complete_with_timeout as an explicit parameter; use it as the rate-limiter registry key and the FetchTimeout label. Reserve pool_label for human-readable diagnostics only."
      - "Test that covers the full production path (fetch_complete_with_timeout) with two relays in the pool and asserts each relay has its own independent GCRA limiter key (e.g., verify the limiter map has two entries keyed on individual URLs, not one entry keyed on the joined string)."

human_verification:
  - test: "Live-relay politeness verification"
    expected: "Sustained run against one curated relay shows <= DEFAULT_REQS_PER_SECOND requests per second; rate-limited notices produce escalating backoff delays visible in logs"
    why_human: "Cannot verify relay politeness or notice-driven behavior without a live relay connection and time-series observation. Note: the pool_label WR-03 residual means this would also reveal the per-relay quota collapsing to a single shared quota across all relays."
---

# Phase 2: Relay Acquisition & Validation Re-Verification Report

**Phase Goal:** The crawler can pull kind-3 and kind:10002 events from a curated relay set politely and completely, and only correct, deduplicated, newest-wins follow lists emerge from the acquisition half.
**Verified:** 2026-06-13T08:30:00Z
**Status:** gaps_found
**Re-verification:** Yes — after gap-closure plans 02-05 through 02-09

## Context

Plans 02-05..02-09 ran as gap-closure for BLOCKERs CR-01..CR-06 and WARNINGs WR-01..WR-03 from the initial verification. A fresh code review (02-REVIEW.md, status: issues_found) confirmed that most fixes landed correctly but flagged:

1. **New BLOCKER (re-opened from prior CR-03):** The inclusive boundary page-back combined with the zero-new-id stop guard cannot guarantee completeness when more than `cap` events share the boundary second and the relay is deterministic/newest-first. The fix made the boundary inclusive but did not add stall detection.

2. **Residual WR-03 (downgraded from prior BLOCKER but still a gap):** `fetch_complete_with_timeout` keys the rate limiter on `pool_label(client)` — a joined string of ALL relay URLs — not the individual relay URL. Per-relay throttling collapses to a shared pool quota.

The prior 5 BLOCKERs (CR-01, CR-02, CR-04, CR-05, CR-06) and Warnings (WR-01, WR-02) are **confirmed closed** by code inspection and test results.

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|---------|
| 1 | Forged/invalid-sig event rejected before acceptance and counted | VERIFIED | verify::accept gates seen.insert in ingest/mod.rs:140-143; id_squat.rs test passes |
| 2 | Wrong kind/author event dropped and counted | VERIFIED | verify::accept checks kind==want_kind && pubkey in requested; verify_gate tests pass |
| 3 | Same verified event id from multiple relays processed at most once | VERIFIED | seen.insert runs only after verify::accept (CR-01 closed); id_squat test load-bearing (RED commit 62190dc proves it failed before fix) |
| 4 | Future-dated event beyond clamp rejected; newest-wins; same-ts tie → lowest id | VERIFIED | replaceable::pick_winner: future_clamp_secs saturating cutoff, max_by+then_with; 4 replaceable tests pass |
| 5 | Malformed p-tags skipped; oversized follow list bounded without panicking | VERIFIED | follow_list::followee_pubkeys uses Tags::public_keys(); reject-not-truncate on cap; follow_list_bounds tests pass |
| 6 | kind:10002 events resolved under identical replaceable rules | VERIFIED | pick_winner is kind-agnostic; relay_list.rs 2/2 pass |
| 7 | Crawler connects curated relay set with reconnect; exponential backoff + jitter | VERIFIED | connect_curated wired; backoff_delay_unjittered: failures>=64 early return cap (WR-01 closed); saturation test sweeps 64..=127 asserting non-zero and <= cap |
| 8 | NIP-11 limits read/cached; max_limit clamped; missing docs → defaults; timeout + body bound | VERIFIED | LazyLock<reqwest::Client> with .timeout(10s)/.connect_timeout(5s); MAX_NIP11_BYTES stream-and-bail; MAX_ADVERTISED_LIMIT=5000 via clamp_max_limit; 9 nip11_limits tests pass; LimitCache wired to production path via get_or_fetch(relay_url) in acquire_validated_lists_client |
| 9 | Per-relay rate limiting throttles requests; pool key is per-relay, not pool-wide | FAILED | fetch_complete_with_timeout derives relay_url from pool_label(client) (line 273) — a joined string of all relay URLs, not the individual relay URL passed to acquire_validated_lists_client. The rate limiter is keyed on the pool, not per relay. Production wiring tests exercise paginate_chunk_gated directly (explicit relay_url) and do not catch this path. |
| 10 | Capped result triggers another page; EOSE never treated as completeness; no silent event loss | FAILED | page_back is inclusive (CR-03 addressed). BUT: zero-new-id guard fires as 'exhausted' when the loop is stalled at until=T with more events remaining at that second. A deterministic newest-first relay returning the same cap-sized prefix for repeated until=T causes new_ids=0 while C(T) is never fetched. inclusive_boundary_keeps_boundary_event test does not cover this: it scripts the mock to return the cut event, which real relays do not do. MAX_PAGES_PER_CHUNK budget guards against an adversarial always-new-id relay (CR-04 closed) but not the stall case. |
| 11 | Timed-out windows requeued, not treated as complete | VERIFIED | fetch_window_with_deadline constructs RelayError::FetchTimeout when elapsed >= timeout (2 sites: line 103 and 208); fetch_timeout.rs 2/2 pass |
| 12 | Fetch->ingest wired: ValidatedFollowList emerges end-to-end | VERIFIED | acquire_validated_lists and acquire_validated_lists_client wired; E2E test acquire_pipeline.rs passes |

**Score:** 7/9 must-haves verified (using the 9 plan-declared must_haves)

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `src/ingest/verify.rs` | Sig + kind/author gate | VERIFIED | accept() implemented; Event::verify(); metrics counters present |
| `src/ingest/replaceable.rs` | Clamp + newest-wins + tie-break | VERIFIED | pick_winner with saturating clamp, max_by+then_with |
| `src/ingest/follow_list.rs` | p-tag extraction + dedup + cap | VERIFIED | public_keys() skip, self-drop, reject-not-truncate |
| `src/ingest/mod.rs` | Orchestrator + ValidatedFollowList, dedup-after-verify | VERIFIED | verify::accept before seen.insert (CR-01 closed); ValidatedFollowList 4 fields |
| `src/relay/mod.rs` | connect_curated + acquire seam wired to registry/cache/notices | VERIFIED | connect_curated, acquire_validated_lists_client wired to LimitCache + RateLimiterRegistry + spawn_notice_consumer |
| `src/relay/nip11.rs` | NIP-11 fetch with timeout + body bound + LazyLock client + MAX_ADVERTISED_LIMIT | VERIFIED | NIP11_CLIENT LazyLock with .timeout(10s)/.connect_timeout(5s); MAX_NIP11_BYTES stream-and-bail; clamp_max_limit; 9 tests pass |
| `src/relay/rate_limit.rs` | Governor limiter with Arc-per-relay, backoff saturation | VERIFIED | Arc<DirectLimiter> in map; clone-and-await; failures>=64 early cap; 7 tests pass |
| `src/relay/fetch.rs` | Inclusive page-back, MAX_PAGES_PER_CHUNK, FetchTimeout, pool_label key | STUB | page_back inclusive (line 68); MAX_PAGES_PER_CHUNK=10_000 (line 48); FetchTimeout constructed (lines 103, 208). BUT pool_label used as rate-limiter key (line 273) — not per-relay; stall detection missing for deterministic-relay boundary exhaustion |
| `tests/acquire_pipeline.rs` | E2E test | VERIFIED | 1/1 passes |
| `tests/pagination.rs` | RELAY-03 pagination test including inclusive boundary | PARTIAL | 7/7 pass, but inclusive_boundary_keeps_boundary_event scripts the mock to return the cut event (not real relay behavior); stall-at-boundary-second not tested |
| `tests/fetch_timeout.rs` | FetchTimeout requeue test | VERIFIED | 2/2 pass |
| `tests/id_squat.rs` | CR-01 id-squat attack test | VERIFIED | 1/1 passes; load-bearing (RED commit 62190dc) |
| `tests/production_wiring.rs` | WR-03 wiring test | PARTIAL | 5/5 pass, but tests exercise paginate_chunk_gated with explicit relay_url; fetch_complete_with_timeout pool_label key not tested |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `src/relay/mod.rs` | `src/relay/fetch.rs fetch_complete` | acquire_validated_lists_client | WIRED | fetch_complete called from production wrapper (line 227) |
| `src/relay/mod.rs` | `src/ingest/mod.rs ingest_events` | acquire_validated_lists | WIRED | ingest::ingest_events called; all events route through it |
| `src/relay/mod.rs` | `src/relay/nip11.rs LimitCache::get_or_fetch` | limit_cache.get_or_fetch(relay_url) | WIRED | line 220 sources max_limit from cache; correct relay_url used |
| `src/relay/fetch.rs paginate_chunk_gated` | `src/relay/rate_limit.rs acquire()` | registry.acquire(relay_url) before each REQ | WIRED | line 178; relay_url passed from paginate_chunk_gated caller |
| `src/relay/fetch.rs fetch_complete_with_timeout` | `src/relay/rate_limit.rs acquire()` | pool_label key | PARTIAL | acquire IS called (line 278), but relay_url is pool_label joined string, not per-relay URL. The gate exists but the key is wrong. |
| `src/relay/mod.rs` | `src/relay/rate_limit.rs record_notice` | spawn_notice_consumer -> handle_relay_message | WIRED | handle_relay_message calls registry.record_notice; spawn_notice_consumer wired in mod.rs |
| `tests/production_wiring.rs` | `tests/mock_relay/mod.rs` | ScriptedRelay | WIRED | paginate_chunk_gated tested with injected ScriptedRelay |

### Data-Flow Trace (Level 4)

| Artifact | Data Variable | Source | Produces Real Data | Status |
|----------|---------------|--------|--------------------|--------|
| `src/relay/rate_limit.rs` | per-relay GCRA token | registry.acquire(relay_url) via paginate_chunk_gated | Yes — called before each window REQ | FLOWING (but keyed on pool_label in production path through fetch_complete_with_timeout) |
| `src/relay/nip11.rs` | max_limit | limit_cache.get_or_fetch(relay_url) in acquire_validated_lists_client | Yes — per-relay cache wired | FLOWING |
| `src/ingest/mod.rs` | ValidatedFollowList | ingest_events after verify->dedup->replaceable->follow_list | Yes — wired and tested | FLOWING |
| `src/relay/mod.rs` | failure_count escalation | record_notice via spawn_notice_consumer | Yes — consumer spawned, handle_relay_message wired | FLOWING |

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| All INGEST tests pass | `cargo test --test verify_gate dedup id_squat replaceable relay_list follow_list_bounds` | 1+1+1+4+2+3 = 12 passed | PASS |
| All RELAY unit tests pass | `cargo test --test reconnect_policy rate_limit_backoff nip11_limits pagination fetch_timeout production_wiring` | 1+7+9+7+2+5 = 31 passed | PASS |
| E2E acquire pipeline | `cargo test --test acquire_pipeline` | 1/1 passed | PASS |
| FetchTimeout constructed on timeout | `grep -rn "FetchTimeout(" src/` | 2 construction sites in src/relay/fetch.rs (line 103, 208) | PASS |
| Rate limiter keyed on individual relay_url in production path | `grep -n "pool_label" src/relay/fetch.rs` | Line 273: `let relay_url = pool_label(client).await;` — joined string, not individual URL | FAIL |
| Stall detection for boundary-second > cap events | `grep -n "prev_until\|stalled\|stall" src/relay/fetch.rs` | No stall detection exists | FAIL |
| Full suite green | `cargo test --tests` | 19 result lines, all 0 failed | PASS |

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|-------------|-------------|--------|---------|
| RELAY-01 | 02-01, 02-03, 02-08 | Reconnect + exponential backoff + jitter | VERIFIED | connect_curated wired; backoff_delay_unjittered failures>=64 early-return cap; saturation test 64..=127 non-zero; concurrent_acquires_share_one_limiter passes |
| RELAY-02 | 02-01, 02-03, 02-07, 02-09 | NIP-11 limits read and respected in production | VERIFIED | LimitCache.get_or_fetch wired in acquire_validated_lists_client; MAX_NIP11_BYTES + MAX_ADVERTISED_LIMIT; NIP11_CLIENT with timeouts; 9 nip11_limits tests pass |
| RELAY-03 | 02-03, 02-04, 02-05 | Pagination + EOSE never trusted as complete | PARTIAL | paginate_chunk + page_back inclusive implemented; MAX_PAGES_PER_CHUNK guards adversarial relay; FetchTimeout on elapsed timeout. BUT: zero-new-id guard fires as exhaustion when loop is stalled at boundary second with >cap events; stall-at-boundary not tested against deterministic relay behavior |
| RELAY-04 | 02-03, 02-08, 02-09 | Per-relay rate limiting + notice backoff in production | PARTIAL | RateLimiterRegistry::acquire wired via paginate_chunk_gated; spawn_notice_consumer + handle_relay_message wired. BUT: fetch_complete_with_timeout passes pool_label (joined string) as relay_url to paginate_chunk_gated, collapsing per-relay throttling to a single pool quota |
| INGEST-01 | 02-02 | Sig verification + count before accept | VERIFIED | verify::accept with Event::verify() + metrics |
| INGEST-02 | 02-02, 02-06 | Duplicate ids processed once, dedup AFTER verify | VERIFIED | HashSet<EventId> seen-set; verify::accept gates seen.insert; id_squat test load-bearing |
| INGEST-03 | 02-02 | Future-clamp + newest-wins + lowest-id tie-break | VERIFIED | pick_winner with saturating clamp + max_by+then_with |
| INGEST-04 | 02-02 | Malformed p-tags skipped; oversized list bounded | VERIFIED | public_keys() skip + reject-not-truncate |
| INGEST-05 | 02-02 | kind:10002 under same replaceable rules | VERIFIED | pick_winner is kind-agnostic; relay_list.rs tests pass |

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| `src/relay/fetch.rs` | 273 | `pool_label(client)` used as rate-limiter registry key | BLOCKER (WR-03 residual) | All relays share one GCRA limiter; per-relay quota destroyed; key changes on relay connect/disconnect |
| `src/relay/fetch.rs` | 131-133 | zero-new-id guard terminates loop without stall detection | BLOCKER (CR-03 residual) | A deterministic newest-first relay with >cap events at the boundary second causes silent truncation; the loop mistakes a stalled until=T for genuine exhaustion |
| `tests/pagination.rs` | 48 | ScriptedRelay is hand-fed the cut boundary event in window2 | WARNING | Test validates mock behavior (a relay varying its prefix), not production invariant (a deterministic relay returning the same prefix); the boundary-stall failure mode is untested |

### Human Verification Required

#### 1. Live-Relay Politeness

**Test:** Run acquire_validated_lists_client against one real curated relay for 60 seconds; observe outbound REQ rate and response to NOTICE messages.
**Expected:** REQ rate <= 4/second per relay; rate-limited NOTICE messages produce escalating delays in logs. With the pool_label key bug, all relays in the pool would share a single 4 req/sec quota (not each having their own), which would be visible as under-throttling when multiple relays are connected simultaneously.
**Why human:** Cannot verify relay politeness or notice-driven behavior without a live relay connection and time-series observation.

## Gaps Summary

Plans 02-05 through 02-09 closed most of the original gaps. The code review identified two remaining issues that block the phase goal:

**BLOCKER 1 (CR-03 residual — boundary-second completeness stall):** The inclusive page-back fix correctly sets `until = oldest` rather than `oldest - 1`. However, the zero-new-id termination guard cannot distinguish "boundary second genuinely exhausted" from "deterministic relay serving the same cap-sized prefix for a pinned until=T while additional events remain at that second." With `cap = 2` and three events `A(T)`, `B(T)`, `C(T)`: Window 1 returns `[N(T+1), A(T)]`, oldest=T, capped → until=T. Window 2 returns `[A(T), B(T)]`, new_ids=1 (B), still capped → until=T. Window 3 returns `[A(T), B(T)]` again, new_ids=0 → loop breaks. C(T) is silently lost. The test `inclusive_boundary_keeps_boundary_event` scripts the mock to return `boundary_b` in window 2, which a real deterministic relay never does. The fix must detect the stall (until is pinned and returned >= cap and new_ids == 0) and surface it as a requeue error rather than silent completion.

**BLOCKER 2 (WR-03 residual — pool_label as rate-limiter key):** `fetch_complete_with_timeout` at line 273 derives `relay_url = pool_label(client).await`, which joins all connected relay URLs into a single string. This string is then passed to `paginate_chunk_gated` as the registry key. Consequences: (1) all connected relays share one GCRA limiter instead of having independent per-relay quotas; (2) pool membership changes mint a new limiter and discard accrued GCRA state. `acquire_validated_lists_client` has the correct individual `relay_url` but does not thread it into `fetch_complete`. The production wiring tests bypass this path by calling `paginate_chunk_gated` directly.

The ingest gate (INGEST-01..05) is sound, well-tested, and fully closed. The previous BLOCKERs (CR-01 id-squat, CR-02 FetchTimeout, CR-04 page budget, CR-05 Arc limiter, CR-06 NIP-11 timeout) and Warnings (WR-01 backoff saturation, WR-02 MAX_ADVERTISED_LIMIT) are all confirmed closed.

---

_Verified: 2026-06-13T08:30:00Z_
_Verifier: Claude (gsd-verifier)_
