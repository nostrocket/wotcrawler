---
phase: 02-relay-acquisition-validation
reviewed: 2026-06-12T14:03:27Z
depth: standard
files_reviewed: 22
files_reviewed_list:
  - src/error.rs
  - src/ingest/follow_list.rs
  - src/ingest/mod.rs
  - src/ingest/replaceable.rs
  - src/ingest/verify.rs
  - src/lib.rs
  - src/relay/fetch.rs
  - src/relay/mod.rs
  - src/relay/nip11.rs
  - src/relay/rate_limit.rs
  - tests/acquire_pipeline.rs
  - tests/common/mod.rs
  - tests/dedup.rs
  - tests/follow_list_bounds.rs
  - tests/mock_relay/mod.rs
  - tests/nip11_limits.rs
  - tests/pagination.rs
  - tests/rate_limit_backoff.rs
  - tests/reconnect_policy.rs
  - tests/relay_list.rs
  - tests/replaceable.rs
  - tests/verify_gate.rs
findings:
  critical: 6
  warning: 3
  info: 9
  total: 18
status: issues_found
---

# Phase 2: Code Review Report

**Reviewed:** 2026-06-12T14:03:27Z
**Depth:** standard
**Files Reviewed:** 22
**Status:** issues_found

## Summary

Phase 2 (relay acquisition & validation) was reviewed adversarially at standard depth: all 10 source files and 12 test files, plus cross-checks against the vendored `nostr-relay-pool 0.44.1` source, `Cargo.toml`, `cargo check --tests` (passes), and `cargo clippy` (2 minor test warnings). The ingest gate (verify/replaceable/follow_list) is largely sound and well-tested, but the acquisition layer has serious gaps **exactly where the project's own threat model says relays are adversarial**:

- The dedup-before-verify ordering in both the ingest orchestrator and the fetch-level dedup allows a hostile relay to suppress a victim's genuine follow list by id-squatting (CR-01).
- The documented Pitfall-9 guarantee ("a timed-out window is requeued, never treated as complete") is **not implemented**: `RelayError::FetchTimeout` is never constructed anywhere, and the SDK's `fetch_events` returns a partial `Ok` on timeout (verified in vendored nostr-relay-pool source: on timeout the activity sender is dropped and the stream ends without error), so timed-out windows are silently recorded complete (CR-02).
- The pagination loop can silently skip events at a same-second cap boundary (CR-03) and can loop forever against a relay that ignores `until` (CR-04).
- The per-relay rate limiter is broken under exactly the concurrent use it documents (CR-05), and the NIP-11 HTTP fetch has no timeout or body-size bound (CR-06).
- A reproduced arithmetic bug makes the backoff schedule return a ZERO delay for failure counts 119-127 (WR-01) — the very saturation bug the code comment claims was fixed.
- The RELAY-04 mechanisms (rate limiter, notice backoff, NIP-11 limit cache) exist but are wired into no production path (WR-03).

## Narrative Findings (AI reviewer)

## Critical Issues

### CR-01: Dedup-before-verify lets a hostile relay suppress a victim's genuine follow list (id-squatting)

**File:** `src/ingest/mod.rs:128-135` (also `src/relay/fetch.rs:91-100`)
**Issue:** `ingest_events` inserts the **unverified, relay-claimed** `event.id` into the seen-set *before* `verify::accept` runs:

```rust
if !seen.insert(event.id) {
    continue; // duplicate id from another relay — already handled.
}
if !verify::accept(&event, want_kind, requested) {
    continue;
}
```

`event.id` is just an untrusted field until `Event::verify()` recomputes it. Event ids are public, so a hostile relay can return a forged event carrying the *claimed* id of the victim's genuine newest kind-3 (with tampered content). If the forged copy is iterated first, it consumes the id in `seen`, fails verification, and the genuine event arriving later from an honest relay is then skipped as a "duplicate". The victim's newest follow list is silently dropped and an older list (or none) wins — defeating INGEST-02's purpose. The same first-occurrence-wins pre-verification dedup exists in `fetch::dedup_by_id` (`src/relay/fetch.rs:91-100`), so fixing only the orchestrator is insufficient: the forged copy can evict the genuine one before ingest ever sees it.
**Fix:** Only record an id as seen after it has passed verification, and make the fetch-level dedup verification-agnostic-safe (e.g., dedup on the full event or drop `dedup_by_id` and rely solely on the post-verify seen-set):

```rust
// src/ingest/mod.rs
for event in events {
    if seen.contains(&event.id) {
        continue;
    }
    if !verify::accept(&event, want_kind, requested) {
        continue; // do NOT mark the id seen — a forged copy must not squat it.
    }
    seen.insert(event.id); // id is now proven to belong to this content.
    by_author.entry(event.pubkey).or_default().push(event);
}
```

(A verified id genuinely identifies its content, so post-verify dedup is sound; the only cost is re-verifying true duplicates, which is bounded by relay count.)

### CR-02: `RelayError::FetchTimeout` is never constructed — timed-out windows are recorded as complete (the exact Pitfall 9 failure the module claims to prevent)

**File:** `src/relay/fetch.rs:125-151` (variant at `src/error.rs:60-61`)
**Issue:** The module docs (lines 8-10, 108-111) and the error enum promise that "a timed-out window surfaces as `RelayError::FetchTimeout` so the caller requeues those authors rather than treating them as done." No code path constructs `FetchTimeout` (grep confirms only doc references exist). Worse, the actual SDK behavior makes the timeout *invisible*: in vendored `nostr-relay-pool-0.44.1/src/relay/inner.rs:1457-1461`, when the subscription timeout fires, `handle_auto_closing` returns `None`, the SDK logs a warning, drops the activity sender, and the event stream simply **ends** — `fetch_events` returns `Ok(partial_events)`. In `paginate_chunk`, a timed-out partial window almost always has `returned < cap`, so `page_back` returns `None` and the chunk is recorded complete. Result: slow/hostile relays produce silently incomplete follow data marked as fresh — data loss against the phase's core "complete and continuously fresh" contract, with no error and no requeue.
**Fix:** Detect the deadline expiring independently of the SDK and surface it:

```rust
let started = tokio::time::Instant::now();
let events = client.fetch_events(filter, timeout).await.map_err(RelayError::Client)?;
if started.elapsed() >= timeout {
    // The SDK returns partial Ok on timeout (verified in nostr-relay-pool
    // 0.44.1); treat an elapsed deadline as a timed-out window, not completion.
    return Err(RelayError::FetchTimeout(/* relay/pool identifier */));
}
```

Alternatively wrap with `tokio::time::timeout(timeout + grace, ...)` around a `stream_events`-based collection where the close *reason* is observable. Either way, the requeue contract must actually exist before this ships, and a test must cover it.

### CR-03: `until = oldest - 1` page-back silently skips events sharing the boundary second — permanent, undetected data loss

**File:** `src/relay/fetch.rs:42-50`
**Issue:** When a window is capped, the next page requests `until = oldest - 1`. If the relay's cap cut the window *mid-second* — i.e., more events exist with `created_at == oldest` than were returned — those remaining events can never be fetched: the next REQ's `until` excludes the entire boundary second. With `max_limit = 500` and author-chunked kind-3 fetches, multiple authors' newest events landing on the same second at a window boundary is routine at scale. Those authors' lists are silently lost while the chunk is recorded complete, violating the test suite's own claim ("no pubkeys are silently dropped across windows", `tests/pagination.rs:3`). Nothing detects or repairs this.
**Fix:** Page back *inclusively* (`until = oldest`) and rely on id-dedup for the overlap, terminating when a window contributes no new event ids:

```rust
pub fn page_back(returned: usize, cap: usize, oldest: Option<Timestamp>) -> Option<Timestamp> {
    match (returned >= cap, oldest) {
        (true, Some(ts)) => Some(ts), // inclusive: re-cover the boundary second
        _ => None,
    }
}
```

with `paginate_chunk` tracking seen ids per chunk and stopping when `new_events == 0` (this also requires the CR-04 progress guard below to avoid looping on a single over-cap second).

### CR-04: `paginate_chunk` has no progress/page bound — an adversarial relay ignoring `until` drives an infinite loop with unbounded memory

**File:** `src/relay/fetch.rs:68-86`
**Issue:** Loop termination depends entirely on the relay honoring the `until` filter. A hostile or buggy relay that keeps returning the same `>= cap` window regardless of `until` makes `page_back` return the same `oldest - 1` forever: the loop never terminates and `out.extend(events)` grows without bound (memory exhaustion). The same happens if a relay serves `>= cap` events with `created_at = 0` (`saturating_sub(1)` pins `until` at 0). Returned events are also never checked against the filter (`created_at <= until`, requested kind/authors) client-side, so the loop's own control variable is fully adversary-controlled. The project's threat model explicitly treats relays as adversarial; this is the one loop in the codebase whose termination they control.
**Fix:** Enforce monotonic progress and a hard page budget:

```rust
const MAX_PAGES_PER_CHUNK: usize = 1_000; // generous; a real chunk is a handful
let mut pages = 0;
loop {
    // ... fetch ...
    // Drop events that violate the filter (created_at > until): the relay is
    // lying and they must not steer pagination.
    events.retain(|e| e.created_at <= until);
    pages += 1;
    if pages >= MAX_PAGES_PER_CHUNK {
        return Err(RelayError::FetchTimeout(...)); // or a dedicated variant; requeue, don't trust
    }
    match page_back(returned, cap, oldest) {
        Some(next) if next < until => until = next, // strictly decreasing or stop
        _ => break,
    }
}
```

### CR-05: `RateLimiterRegistry::acquire` multiplies the quota under concurrency and discards accrued limiter state — the politeness guarantee (T-02-10) is void

**File:** `src/relay/rate_limit.rs:157-179`
**Issue:** Two independent bugs in the take-out/await/put-back scheme:

1. **Quota multiplication.** Caller A `remove`s the relay's limiter from the map; caller B then finds the entry vacant and creates a *fresh* limiter with full burst capacity. N concurrent callers get N independent limiters, multiplying the per-relay rate by the concurrency level — precisely the concurrent-fetch scenario the doc claims to gate ("concurrent fetches never exceed the configured per-relay quota") and the IP-ban threat (T-02-10) this registry exists to prevent.
2. **Wrong limiter kept.** After awaiting, `map.entry(relay_url.to_string()).or_insert(limiter)` keeps the *existing* (fresh, stateless) entry and **drops `limiter`** — the one with accrued GCRA state. The comment on line 176 ("keep the one that has accrued state (ours) and drop the spare") describes the exact opposite of what `or_insert` does. Accrued rate state is erased on every contended acquire.

**Fix:** Store `Arc<DirectLimiter>` in the map; clone the `Arc`, release the lock, then await — no removal, no reinsertion, one shared limiter per relay:

```rust
limiters: Mutex<HashMap<String, Arc<DirectLimiter>>>,
// ...
let limiter: Arc<DirectLimiter> = {
    let mut map = self.limiters.lock().expect("rate-limiter map not poisoned");
    Arc::clone(map.entry(relay_url.to_string()).or_insert_with(|| {
        Arc::new(RateLimiter::direct(Quota::per_second(self.reqs_per_second)))
    }))
};
limiter.until_ready().await;
```

### CR-06: NIP-11 HTTP fetch has no timeout and no response-size bound — one hostile relay hangs the crawler or exhausts memory

**File:** `src/relay/nip11.rs:134-154`
**Issue:** `fetch_limits` uses `reqwest::Client::new()`, which has **no default timeout**, and reads the body with unbounded `resp.text()`. A hostile relay (the module's own stated threat, T-02-13: "a relay may ship a hostile or absent document") can (a) accept the TCP connection and never respond, hanging `fetch_limits` — and therefore `LimitCache::get_or_fetch` and the calling crawl task — forever, or (b) stream an arbitrarily large body into memory. This directly violates the project's Pitfall 9 rule that every network fetch carries a deadline; the module defends against hostile *values* in the document but not a hostile *transport*.
**Fix:**

```rust
let client = reqwest::Client::builder()
    .timeout(Duration::from_secs(10))
    .connect_timeout(Duration::from_secs(5))
    .redirect(reqwest::redirect::Policy::limited(2))
    .build()
    .map_err(|e| RelayError::Nip11Fetch { relay: relay_url.to_string(), reason: e.to_string() })?;
// Bound the body: NIP-11 documents are tiny.
const MAX_NIP11_BYTES: usize = 64 * 1024;
let bytes = resp.bytes().await.map_err(...)?;
if bytes.len() > MAX_NIP11_BYTES {
    return Err(RelayError::Nip11Fetch { relay: relay_url.to_string(), reason: "NIP-11 body too large".into() });
}
```

(Build the client once, not per call. Note also that once NIP-65 gossip feeds *event-sourced* relay urls into this path, the unvalidated `nip11_http_url` becomes an SSRF surface — see WR-02/IN-05 context.)

## Warnings

### WR-01: `backoff_delay_unjittered` returns a ZERO delay for failure counts 119-127 with the default 1s base — the saturation bug the comment claims was fixed

**File:** `src/relay/rate_limit.rs:100-116`
**Issue:** `u128::checked_shl` only guards the *shift amount* (`None` iff `rhs >= 128`); bits shifted out of the top are **silently discarded**. With `base = 1s` (`10^9` ns, lowest set bit at position 9), every set bit is shifted past bit 127 once `failures >= 119`, so `checked_shl` returns `Some(0)` and the function returns `Duration::ZERO`. Reproduced: `failures=119..=127` with `base=1s, cap=300s` all yield `0 ns` — i.e., *no backoff at the highest failure counts*, against a relay that has rate-limited the crawler 119+ consecutive times. The `failures >= 128` guard misses this range, and the existing tests (`tests/reconnect_policy.rs:50-52`) probe 20, 63, and 255 — skipping the broken band. The comment ("we must NOT truncate the factor... that was the saturation bug") documents the intent but the chosen API truncates anyway.
**Fix:** Saturate early — any shift that can exceed the cap is the cap:

```rust
pub fn backoff_delay_unjittered(failures: u32, base: Duration, cap: Duration) -> Duration {
    if base.is_zero() { return Duration::ZERO; }
    // base >= 1ns, cap <= ~584 years < 2^64 ns: any 2^64 factor exceeds any cap.
    if failures >= 64 { return cap; }
    let scaled = base.as_nanos().saturating_mul(1u128 << failures);
    Duration::from_nanos(scaled.min(cap.as_nanos()).min(u64::MAX as u128) as u64)
}
```

Add a regression test at `failures = 119..=127`.

### WR-02: Hostile NIP-11 documents advertising absurdly large limits are accepted verbatim, re-opening the silent-truncation trap (Pitfall 1) the cap exists to defeat

**File:** `src/relay/nip11.rs:81-86`
**Issue:** `clamp_limit` defends only against *non-positive* advertised values; any positive `i32` up to `2_147_483_647` is accepted as `max_limit`. The page-back defense works only if the cap is **at most** the relay's true enforced limit: a relay advertising `max_limit: 2000000000` while silently truncating responses at 500 makes `returned >= cap` never true, so every truncated window is treated as "genuinely exhausted" — exactly Pitfall 1, restored by a single hostile JSON field (T-02-13 covers hostile documents; this half is unhandled). The huge value also flows into `Filter::limit(cap)` on every REQ.
**Fix:** Clamp advertised limits to a sane ceiling as well as a floor:

```rust
const MAX_SANE_LIMIT: usize = 10_000;
fn clamp_limit(advertised: Option<i32>, default: usize) -> usize {
    match advertised {
        Some(v) if v > 0 => (v as usize).min(MAX_SANE_LIMIT),
        _ => default,
    }
}
```

(Capping at a ceiling is safe: a too-small cap only costs extra pages; a too-large cap costs silent incompleteness.)

### WR-03: RELAY-02/RELAY-04 mechanisms are dead in the production path — no rate limiting, no notice handling, no NIP-11 cache use anywhere in the acquisition flow

**File:** `src/relay/mod.rs:183-203`, `src/relay/fetch.rs:113-151`, `src/relay/rate_limit.rs:157`, `src/relay/nip11.rs:178`
**Issue:** Grep over `src/` confirms `RateLimiterRegistry`, `LimitCache::get_or_fetch`, `record_notice`, and `backoff` have **zero callers outside their own modules**. The production entry point `acquire_validated_lists_client` -> `fetch_complete` issues REQs with no `acquire()` gate (the doc on `acquire` claims it "gates every outbound REQ" — it gates none), no notification handler anywhere subscribes to relay NOTICE/CLOSED messages to feed `record_notice` (so `rate-limited`/`blocked` notices are never observed), and `max_limit` is a hand-passed parameter never sourced from the NIP-11 cache. Additionally, `fetch_complete` applies a single `max_limit` to a pool-wide `fetch_events`: with multiple connected relays the only safe value is the *minimum* of the per-relay caps, and nothing computes or documents that. RELAY-02 and RELAY-04 are currently library code with tests, not enforced behavior — the "two halves never connected" anti-pattern the codebase itself warns about (`src/relay/mod.rs:120-122`).
**Fix:** Either wire them in this phase (call `registry.acquire(relay_url)` before each REQ; spawn a `client.notifications()` consumer routing `RelayMessage::Notice`/`Closed` into `record_notice`/`backoff`; derive `max_limit` as `min` over `LimitCache::get_or_fetch` for the connected set) or explicitly document in the phase plan that wiring is deferred — and downgrade the misleading doc claims ("gates every outbound REQ") until it is.

## Info

### IN-01: `RelayError::RelayNotFound` is dead code

**File:** `src/error.rs:45-46`
**Issue:** Never constructed anywhere in `src/` or `tests/`. (`FetchTimeout` is covered by CR-02; the `IngestError` variants are at least documented as reserved.)
**Fix:** Remove it, or add it when the per-relay fetch path that needs it lands.

### IN-02: Expensive signature verification runs before the cheap kind/author gate

**File:** `src/ingest/verify.rs:30-46`
**Issue:** `Event::verify()` (secp256k1 + SHA-256) runs on every event, including unsolicited junk a hostile relay injects, before the O(1) kind/author check. Reordering (kind/author first, then verify) rejects flood junk without burning CPU. Pure ordering change; both checks still run for accepted events. (Keep the distinct rejection counters: an event failing kind/author is "unsolicited" regardless of signature validity.)
**Fix:** Swap the two checks.

### IN-03: `dedup_by_id` uses `HashMap<EventId, ()>` instead of `HashSet<EventId>`

**File:** `src/relay/fetch.rs:91-100`
**Issue:** Semantics are set membership; the map of unit values obscures intent.
**Fix:** `let mut seen: HashSet<EventId> = HashSet::with_capacity(events.len());` (note CR-01 may remove this function entirely).

### IN-04: `followee_pubkeys` fully materializes an oversized list before rejecting it

**File:** `src/ingest/follow_list.rs:38-62`
**Issue:** A follow-bomb event's entire deduped p-tag set is allocated before the cap check. Bounded by event size, but the rejection can be made allocation-cheap by bailing as soon as `followees.len() > follow_cap` inside the loop (the cap applies to the deduped set, so the early exit is exact).
**Fix:** Move the cap check into the loop body and return `None` at `follow_cap + 1`.

### IN-05: `fetch_limits` discards the detailed parse-error reason it just built

**File:** `src/relay/nip11.rs:150-153`
**Issue:** `limits_from_json` produces a `Nip11Fetch` error embedding the parser's message; `fetch_limits` then maps it with `|_|` to the generic "response was not a valid NIP-11 document", throwing away the diagnostic and double-wrapping the same variant.
**Fix:** Re-map only the `relay` field, preserving `reason`.

### IN-06: `LimitCache::get_or_fetch` duplicates fetches for the same relay under concurrency

**File:** `src/relay/nip11.rs:178-196`
**Issue:** Two concurrent callers for an uncached relay both miss the cache (lock released across the await) and both fetch, contradicting the "fetched once and reused" contract. Harmless duplication today; becomes a stampede when many fetch tasks share the cache at startup.
**Fix:** Per-key in-flight guard (e.g., `tokio::sync::Mutex<HashMap<...>>` held across the fetch, or a `OnceCell` per entry).

### IN-07: `tokio::time::sleep` relies on the `time` feature being enabled transitively

**File:** `src/relay/rate_limit.rs:217`, `Cargo.toml:10`
**Issue:** `Cargo.toml` enables only `rt-multi-thread` + `macros`; the `time` feature arrives transitively via nostr-sdk/sqlx. If a future dependency change drops it, the crate stops compiling for a non-obvious reason.
**Fix:** Add `"time"` to the tokio features list explicitly.

### IN-08: Doc/comment inaccuracies and clippy nits in the test harness

**File:** `tests/mock_relay/mod.rs:12-14`, `src/relay/rate_limit.rs:76-90`, `tests/pagination.rs:44-48,82-86`
**Issue:** (a) The mock-relay module doc claims each scripted window "honor[s] the filter's `until`", but `fetch_fn` pops windows unconditionally and only *records* `until`. (b) `backoff_delay` is documented as "full jitter" but implements *equal jitter* (`[delay/2, delay]`); full jitter is `[0, delay]`. (c) Clippy flags the redundant `let fut = fetch(filter); async move { fut.await }` wrappers in `pagination.rs` (pass `&mut fetch` as in `acquire_pipeline.rs`). The `acquire` comment inaccuracy is covered by CR-05.
**Fix:** Correct the doc comments; simplify the test closures.

### IN-09: Initial pagination window starts at `Timestamp::now()`, excluding slightly future-dated events the ingest clamp would accept

**File:** `src/relay/fetch.rs:69`
**Issue:** `paginate_chunk` opens with `until = now`, so an author's newest event written by a clock-skewed (but honest) client with `created_at` a few seconds ahead is not fetched this pass, while ingest's clamp (`now + future_clamp_secs`) would have accepted it. Self-healing on the next refresh, but the fetch and clamp boundaries are inconsistent and the freshest list can transiently resolve to an older event.
**Fix:** Open the first window at `now + future_clamp_secs` (same slack as the clamp), or document the one-refresh lag as intended.

---

_Reviewed: 2026-06-12T14:03:27Z_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
