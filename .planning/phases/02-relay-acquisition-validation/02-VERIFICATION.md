---
phase: 02-relay-acquisition-validation
verified: 2026-06-12T14:10:12Z
status: gaps_found
score: 6/9 must-haves verified
overrides_applied: 0
gaps:
  - truth: "A capped result set triggers another page (until-window back); EOSE alone is never treated as completeness"
    status: partial
    reason: "CR-03 (confirmed in code): page_back uses `oldest - 1` (exclusive), meaning events at the exact boundary second that were cut by the cap are permanently lost. Pagination logic is implemented and exercised by tests, but with a known completeness hole. CR-04 (confirmed in code): paginate_chunk has no page budget or monotonic-progress guard; a relay ignoring `until` drives an infinite loop with unbounded memory growth."
    artifacts:
      - path: "src/relay/fetch.rs"
        issue: "page_back returns `saturating_sub(1)` (oldest-1 exclusive), losing events at the boundary second when a cap fires mid-second. No MAX_PAGES guard; `out.extend(events)` is unbounded."
    missing:
      - "page_back must return `oldest` (inclusive) and paginate_chunk must dedup + stop on zero new ids"
      - "paginate_chunk must enforce a MAX_PAGES_PER_CHUNK budget with a hard error/requeue rather than looping forever"

  - truth: "Per-relay rate limiting throttles outbound requests and a rate-limited notice triggers per-relay backoff"
    status: failed
    reason: "CR-05 (confirmed in code): RateLimiterRegistry::acquire removes the limiter from the map before awaiting. A concurrent caller finds the entry vacant and creates a fresh limiter with full burst capacity — quota is multiplied by concurrency level, defeating the politeness guarantee (T-02-10). On return, `or_insert(limiter)` keeps the new stateless entry and discards the one with accrued GCRA state (the comment on line 176 documents the exact opposite of what the code does). WR-01 (confirmed via analysis): backoff_delay_unjittered uses u128.checked_shl which silently truncates high bits at failures 119-127 with base=1s (u128 shift of a number whose low set bit is at position 9 pushes all bits above 127), returning Duration::ZERO at failure counts the code claims it saturates to cap. Additionally, WR-03 (confirmed by grep): RateLimiterRegistry.acquire(), LimitCache.get_or_fetch(), and record_notice() have zero callers outside their own modules in src/. The production path acquire_validated_lists -> fetch_complete issues REQs with no acquire() gate."
    artifacts:
      - path: "src/relay/rate_limit.rs"
        issue: "acquire() take-out/put-back creates quota multiplication under concurrency. or_insert() on line 177 keeps the wrong (stateless) limiter. checked_shl on line 111 returns Some(0) for failures 119-127 with base=1s."
      - path: "src/relay/mod.rs"
        issue: "acquire_validated_lists_client calls fetch_complete with no registry.acquire() gate. No notification handler spawned to feed record_notice()."
      - path: "src/relay/fetch.rs"
        issue: "fetch_complete_with_timeout calls client.fetch_events with no rate limiter gate before each REQ."
    missing:
      - "Store Arc<DirectLimiter> in the map; clone the Arc, release the lock, await — no removal/reinsertion"
      - "Call registry.acquire(relay_url) before each client.fetch_events call in the production path"
      - "Spawn a notifications() consumer routing NOTICE/CLOSED messages into record_notice()/backoff()"
      - "Fix backoff_delay_unjittered: saturate at cap when failures >= 64 (shift any set bit past bit 63 in u128 for ns values)"

  - truth: "The crawler connects to a configurable curated relay set and reconnects with exponential backoff and jitter after a drop"
    status: partial
    reason: "connect_curated is implemented and wired correctly. The app-side backoff_delay function provides capped-exponential-with-jitter. However backoff_delay_unjittered has the WR-01 arithmetic bug (zero delay at failures 119-127 with default 1s base), which means after 119+ consecutive failures against a relay, the crawler retries with NO delay — the opposite of a saturating cap. The connect path itself is sound; the backoff schedule has a silent defect at high failure counts."
    artifacts:
      - path: "src/relay/rate_limit.rs"
        issue: "backoff_delay_unjittered returns Duration::ZERO for failures 119-127 with base=1s due to u128 bit truncation in checked_shl (WR-01 confirmed in code)."
    missing:
      - "Replace checked_shl with an early saturation check: if failures >= 64, return cap directly (any 2^64 factor in nanoseconds exceeds any reasonable cap)"

  - truth: "RelayError::FetchTimeout is never constructed — timed-out windows are silently recorded as complete"
    status: failed
    reason: "CR-02 (confirmed in code): FetchTimeout variant exists in error.rs but grep across the entire src/ tree finds it is never constructed. fetch_complete_with_timeout passes a timeout to client.fetch_events, but the SDK returns a partial Ok on timeout (nostr-relay-pool 0.44.1 on timeout drops the activity sender and stream ends without error). No Instant::now()/elapsed check or tokio::time::timeout wrapper is present in fetch.rs. The `started.elapsed() >= timeout` detection from the PLAN is absent. The SUMMARY claims 'a timed-out window surfaces as RelayError::FetchTimeout so the caller requeues' — this is not implemented."
    artifacts:
      - path: "src/relay/fetch.rs"
        issue: "FetchTimeout is never constructed. The SDK timeout returns partial Ok, not an error. No elapsed-time check exists."
      - path: "src/error.rs"
        issue: "FetchTimeout(String) variant is dead code — only in error.rs, never constructed."
    missing:
      - "Detect timeout expiration independently: record Instant::now() before fetch_events, check elapsed >= timeout after Ok return, construct RelayError::FetchTimeout if elapsed"
      - "Add a test covering the timed-out window requeue path"

  - truth: "NIP-11 HTTP fetch has no timeout and no response-size bound"
    status: failed
    reason: "CR-06 (confirmed in code): fetch_limits uses reqwest::Client::new() with no timeout set (line 136). The response is read with resp.text().await with no body-size bound (line 146). A hostile relay can accept the TCP connection and never respond (hanging the crawler) or stream an arbitrarily large body. The project's own Pitfall 9 rule requires every network fetch to carry a deadline."
    artifacts:
      - path: "src/relay/nip11.rs"
        issue: "reqwest::Client::new() has no default timeout. resp.text() reads unbounded body."
    missing:
      - "Build reqwest client with .timeout(Duration::from_secs(10)).connect_timeout(Duration::from_secs(5)).build()"
      - "Bound the body: read bytes() and reject if > MAX_NIP11_BYTES (e.g. 64 KiB)"
      - "Build the client once (e.g. lazily via once_cell), not per call"
deferred: []
human_verification:
  - test: "Live-relay politeness verification"
    expected: "Sustained run against one curated relay shows <= DEFAULT_REQS_PER_SECOND requests per second; rate-limited notices produce escalating backoff delays visible in logs"
    why_human: "Cannot verify relay politeness or notice-driven behavior without a live relay connection and time-series observation"
---

# Phase 2: Relay Acquisition & Validation Verification Report

**Phase Goal:** The crawler can pull kind-3 and kind:10002 events from a curated relay set politely and completely, and only correct, deduplicated, newest-wins follow lists emerge from the acquisition half.
**Verified:** 2026-06-12T14:10:12Z
**Status:** gaps_found
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|---------|
| 1 | Forged/invalid-sig event rejected before acceptance and counted | VERIFIED | verify::accept calls Event::verify() (id+sig); ingest_invalid_signature metric incremented; tests/verify_gate.rs passes (4/4) |
| 2 | Wrong kind/author event dropped and counted | VERIFIED | verify::accept checks kind==want_kind && pubkey in requested; ingest_unsolicited metric; verify_gate::unsolicited tests pass |
| 3 | Same event id from multiple relays processed at most once | VERIFIED | ingest_events HashSet<EventId> seen-set before verify gate; tests/dedup.rs passes |
| 4 | Future-dated event beyond configurable clamp rejected; newest-wins; same-ts tie → lowest id | VERIFIED | replaceable::pick_winner: future_clamp_secs saturating cutoff, max_by with then_with(b.id.cmp(&a.id)); tests/replaceable.rs 4 tests pass |
| 5 | Malformed p-tags skipped; oversized follow list bounded without panicking | VERIFIED | follow_list::followee_pubkeys uses Tags::public_keys() (skips malformed); reject-not-truncate on cap; no unwrap() in follow_list.rs |
| 6 | kind:10002 events resolved under identical replaceable rules | VERIFIED | pick_winner is kind-agnostic over &Event; tests/relay_list.rs passes (2/2) |
| 7 | Crawler connects curated relay set with reconnect; exponential backoff + jitter | PARTIAL | connect_curated wired correctly; backoff_delay provides exponential+jitter; BUT backoff_delay_unjittered returns ZERO for failures 119-127 with base=1s (WR-01 confirmed) |
| 8 | NIP-11 limits read/cached; max_limit caps pagination; missing docs → defaults | PARTIAL | LimitCache and limits_from_doc implemented correctly; BUT fetch_limits has no reqwest timeout or body-size bound (CR-06); AND LimitCache.get_or_fetch has zero callers in the production path (WR-03) |
| 9 | Per-relay rate limiting throttles requests; rate-limited notice triggers backoff | FAILED | RateLimiterRegistry exists but has two implementation bugs (CR-05: quota multiplication, wrong limiter kept); backoff_delay_unjittered has zero-delay defect at high failure counts (WR-01); acquire() has zero callers in the production fetch path (WR-03) |
| 10 | Capped result triggers another page; EOSE never treated as completeness | PARTIAL | paginate_chunk + page_back implemented; E2E test passes; BUT page_back uses oldest-1 (exclusive boundary, CR-03 confirmed) and paginate_chunk has no page budget guard against adversarial relays (CR-04 confirmed) |
| 11 | Timed-out windows requeued, not treated as complete | FAILED | FetchTimeout variant defined in error.rs but never constructed anywhere in src/. fetch_complete_with_timeout passes a timeout to SDK, but SDK returns partial Ok on timeout. No elapsed-time detection. SUMMARY claim is not implemented. |
| 12 | Fetch→ingest wired: ValidatedFollowList emerges end-to-end | VERIFIED | acquire_validated_lists and acquire_validated_lists_client implemented in relay/mod.rs; E2E test (acquire_pipeline.rs) passes |

**Score:** 6/9 truths from plan must_haves verified (using the 9 plan-declared must_haves, which map to the 12 observable truths above)

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `src/ingest/verify.rs` | Sig + kind/author gate | VERIFIED | accept() implemented; Event::verify(); metrics counters present |
| `src/ingest/replaceable.rs` | Clamp + newest-wins + tie-break | VERIFIED | pick_winner with saturating clamp, max_by+then_with |
| `src/ingest/follow_list.rs` | p-tag extraction + dedup + cap | VERIFIED | public_keys() skip, self-drop, reject-not-truncate |
| `src/ingest/mod.rs` | Orchestrator + ValidatedFollowList | VERIFIED | ingest_events implemented; ValidatedFollowList with 4 fields |
| `src/relay/mod.rs` | connect_curated + acquire seam | VERIFIED | connect_curated, acquire_validated_lists, acquire_validated_lists_client |
| `src/relay/nip11.rs` | NIP-11 fetch + LimitCache | STUB | LimitCache substantive; fetch_limits has no timeout (CR-06); LimitCache unwired from production path |
| `src/relay/rate_limit.rs` | Governor limiter + notice backoff | STUB | RateLimiterRegistry code exists; acquire() has concurrency bug (CR-05); WR-01 zero-delay defect; unwired from production path (WR-03) |
| `src/relay/fetch.rs` | Author-chunked pagination | PARTIAL | paginate_chunk + page_back implemented; CR-03 boundary bug; CR-04 no page budget; CR-02 no FetchTimeout construction |
| `tests/acquire_pipeline.rs` | E2E test | VERIFIED | 1/1 passes |
| `tests/pagination.rs` | RELAY-03 pagination test | VERIFIED | 3/3 passes (tests do not cover the CR-03 or CR-04 failure modes) |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `src/relay/mod.rs` | `src/relay/fetch.rs fetch_complete` | acquire_validated_lists_client | WIRED | fetch_complete called from production wrapper |
| `src/relay/mod.rs` | `src/ingest/mod.rs ingest_events` | acquire_validated_lists | WIRED | ingest::ingest_events called; all events route through it |
| `src/relay/fetch.rs` | `src/relay/nip11.rs LimitCache` | max_limit feeds pagination cap | NOT_WIRED | max_limit is a parameter to fetch_complete but no production caller sources it from LimitCache |
| `src/relay/fetch.rs` | `src/relay/rate_limit.rs acquire()` | governor until_ready gates every REQ | NOT_WIRED | acquire() has zero callers in src/; the production path issues REQs unthrottled |
| `tests/acquire_pipeline.rs` | `tests/mock_relay/mod.rs` | E2E test uses mock relay | WIRED | ScriptedRelay used in acquire_pipeline |

### Data-Flow Trace (Level 4)

| Artifact | Data Variable | Source | Produces Real Data | Status |
|----------|---------------|--------|--------------------|--------|
| `src/relay/rate_limit.rs` | per-relay GCRA token | RateLimiterRegistry::acquire() | No — never called | DISCONNECTED |
| `src/relay/nip11.rs` | max_limit | LimitCache::get_or_fetch() | No — never called from production path | DISCONNECTED |
| `src/ingest/mod.rs` | ValidatedFollowList | ingest_events | Yes — wired and tested | FLOWING |

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| All INGEST tests pass | `cargo test --test verify_gate dedup replaceable relay_list follow_list_bounds` | 16/16 passed | PASS |
| All RELAY unit tests pass | `cargo test --test reconnect_policy rate_limit_backoff nip11_limits pagination` | 17/17 passed | PASS |
| E2E acquire pipeline | `cargo test --test acquire_pipeline` | 1/1 passed | PASS |
| FetchTimeout constructed on timeout | `grep -rn "FetchTimeout(" src/` | zero results | FAIL |
| RateLimiterRegistry called in production path | `grep -rn "acquire\|get_or_fetch\|record_notice" src/ \| grep -v "rate_limit.rs\|nip11.rs"` | zero caller sites | FAIL |

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|-------------|-------------|--------|---------|
| RELAY-01 | 02-01, 02-03 | Reconnect + exponential backoff + jitter | PARTIAL | connect_curated wired; backoff_delay implemented; WR-01 zero-delay defect at failures 119-127 |
| RELAY-02 | 02-01, 02-03 | NIP-11 limits read and respected | PARTIAL | LimitCache implemented; fetch_limits missing timeout; max_limit not sourced from cache in production path |
| RELAY-03 | 02-03, 02-04 | Pagination + EOSE never trusted as complete | PARTIAL | paginate_chunk + page_back implemented; CR-03 exclusive boundary loses events; CR-04 no page budget |
| RELAY-04 | 02-03 | Per-relay rate limiting + notice backoff | FAILED | RateLimiterRegistry exists but unwired from production path; acquire() has concurrency bug |
| INGEST-01 | 02-02 | Sig verification + count before accept | VERIFIED | verify::accept with Event::verify() + metrics |
| INGEST-02 | 02-02 | Duplicate ids processed once | VERIFIED | HashSet<EventId> seen-set in ingest_events |
| INGEST-03 | 02-02 | Future-clamp + newest-wins + lowest-id tie-break | VERIFIED | pick_winner with saturating clamp + max_by+then_with |
| INGEST-04 | 02-02 | Malformed p-tags skipped; oversized list bounded | VERIFIED | public_keys() skip + reject-not-truncate |
| INGEST-05 | 02-02 | kind:10002 under same replaceable rules | VERIFIED | pick_winner is kind-agnostic; relay_list.rs tests pass |

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| `src/ingest/mod.rs` | 128-132 | Dedup-before-verify: seen.insert(event.id) before verify::accept | BLOCKER (CR-01) | A hostile relay can id-squat a victim's genuine event — forged copy consumes the id, genuine copy from honest relay is skipped as "duplicate" |
| `src/relay/fetch.rs` | 42-50 | page_back uses oldest-1 (exclusive boundary) | BLOCKER (CR-03) | Events at the cap boundary second are permanently lost — data loss on every capped window at scale |
| `src/relay/fetch.rs` | 68-86 | paginate_chunk has no page budget | BLOCKER (CR-04) | Adversarial relay ignoring `until` drives infinite loop + unbounded memory |
| `src/relay/fetch.rs` | all | FetchTimeout never constructed | BLOCKER (CR-02) | SDK returns partial Ok on timeout; timed-out windows marked complete; silent data loss |
| `src/relay/rate_limit.rs` | 157-179 | acquire() take-out/put-back: quota multiplication under concurrency | BLOCKER (CR-05) | N concurrent callers get N independent fresh limiters; politeness guarantee is void |
| `src/relay/rate_limit.rs` | 177 | or_insert(limiter) keeps wrong (stateless) entry | BLOCKER (CR-05) | Accrued GCRA state discarded on every contended acquire |
| `src/relay/nip11.rs` | 136 | reqwest::Client::new() with no timeout | BLOCKER (CR-06) | Hostile relay can hang the crawler permanently |
| `src/relay/nip11.rs` | 146 | resp.text() with no body-size bound | WARNING (CR-06) | Hostile relay can exhaust memory |
| `src/relay/rate_limit.rs` | 111 | checked_shl returns Some(0) for failures 119-127 with base=1s | WARNING (WR-01) | Zero backoff delay at highest failure counts — retries immediately instead of saturating at cap |
| `src/relay/nip11.rs` | 82-86 | clamp_limit accepts arbitrarily large positive i32 | WARNING (WR-02) | Hostile NIP-11 doc advertising max_limit=2000000000 restores Pitfall 1 (EOSE treated as complete) |
| `src/relay/mod.rs` | production path | RateLimiterRegistry, LimitCache, record_notice unwired | BLOCKER (WR-03) | RELAY-02 and RELAY-04 are library code with passing tests, not enforced behavior |

### Human Verification Required

#### 1. Live-Relay Politeness

**Test:** Run acquire_validated_lists_client against one real curated relay for 60 seconds; observe outbound REQ rate and response to NOTICE messages.
**Expected:** REQ rate <= 4/second per relay; rate-limited NOTICE messages produce escalating delays in logs.
**Why human:** Cannot verify without a live relay and time-series observation. Note: currently rate limiter is unwired, so this test would reveal WR-03 as a live failure.

## Gaps Summary

The code review's 6 Critical / 3 Warning findings are substantially confirmed by code inspection. Five gaps block the phase goal:

**CR-01 (BLOCKER) — Dedup-before-verify in the ingest orchestrator and fetch.rs dedup_by_id.** The seen-set is populated before verify::accept runs. A hostile relay can id-squat a victim's genuine follow list: the forged copy (carrying the real event's claimed id) is iterated first, its id enters the seen-set, verification fails, and the genuine copy from an honest relay is then silently discarded as "duplicate". This directly inverts INGEST-02's purpose. The same attack works at the fetch level via dedup_by_id.

**CR-02 (BLOCKER) — FetchTimeout never constructed; timed-out windows recorded as complete.** The SDK returns partial Ok on timeout (verified in nostr-relay-pool 0.44.1 vendored source). No elapsed-time check or tokio::time::timeout wrapper exists in fetch.rs. The PLAN and SUMMARY both claim this works; the code proves it does not. Data loss from slow/hostile relays is silent and unauditable.

**CR-03 + CR-04 (BLOCKER) — Pagination completeness loss + adversarial infinite loop.** page_back's exclusive oldest-1 boundary permanently loses events at the boundary second when a relay caps mid-second. paginate_chunk has no page budget, allowing a relay that ignores `until` to drive an infinite loop. The tests do not cover these failure modes.

**CR-05 (BLOCKER) — Rate limiter is broken under concurrency and unwired from production.** The take-out/put-back acquire() multiplies quota by concurrency; the wrong (stateless) limiter is retained. More critically, acquire() has zero callers in the production path — RELAY-04 mechanisms (rate limiter, notice backoff, NIP-11 cache) are exercised only in their own tests, never in acquire_validated_lists.

**CR-06 (BLOCKER) — NIP-11 fetch has no timeout or body-size bound.** reqwest::Client::new() carries no deadline. A hostile relay can hang the crawler or exhaust memory through the NIP-11 path.

**WR-01 (WARNING) — Backoff zero-delay defect.** backoff_delay_unjittered returns Duration::ZERO for failure counts 119-127 with the default 1s base due to u128 bit truncation in checked_shl. The fix described in the code comment ("we must NOT truncate") is documented but not implemented.

**WR-02 (WARNING) — No upper bound on advertised NIP-11 limit.** clamp_limit accepts any positive i32; a relay advertising max_limit=2000000000 defeats the pagination completeness logic.

The ingest gate (INGEST-01 through INGEST-05) is sound and well-tested. The fundamental acquisition mechanisms (RELAY-01 connect, RELAY-02 parse, RELAY-03 pagination logic, RELAY-04 governor) have correct implementations in their modules. The phase goal is not achieved because: (a) five critical correctness defects exist in the acquisition layer, (b) RELAY-02 and RELAY-04 mechanisms are unwired from the production fetch path, and (c) the completeness claim ("complete") is undermined by CR-02, CR-03, and CR-04.

---

_Verified: 2026-06-12T14:10:12Z_
_Verifier: Claude (gsd-verifier)_
