---
phase: 02-relay-acquisition-validation
reviewed: 2026-06-13T07:14:00Z
depth: standard
files_reviewed: 5
files_reviewed_list:
  - src/ingest/mod.rs
  - src/relay/fetch.rs
  - src/relay/mod.rs
  - src/relay/nip11.rs
  - src/relay/rate_limit.rs
findings:
  critical: 1
  warning: 4
  info: 3
  total: 8
status: issues_found
---

# Phase 02: Code Review Report

**Reviewed:** 2026-06-13T07:14:00Z
**Depth:** standard
**Files Reviewed:** 5
**Status:** issues_found

## Summary

Re-review of the five files modified by gap-closure plans 02-05..02-09, which claimed to
close BLOCKERs CR-01..CR-06 and WARNINGs WR-01..WR-03 from the prior review (preserved in
this file's git history). The previously-flagged fixes are largely sound:

- **CR-01** (dedup-after-verify ordering) is now correct in `ingest_events`
  (`src/ingest/mod.rs:140-146`): `verify::accept` gates the `seen.insert`, so a forged
  id-squat copy fails verify and never consumes a genuine id. The fetch-half pre-verify
  dedup was removed. Closed.
- **CR-02** (`FetchTimeout` construction) is now real: `fetch_window_with_deadline`
  (`src/relay/fetch.rs:195-211`) does the independent elapsed check and constructs the
  variant. Closed, with a caveat — see WR-01 below.
- **CR-04 / T-02-16** (`MAX_PAGES_PER_CHUNK` budget + zero-new-id guard) bounds an
  adversarial always-full relay. Closed.
- **CR-05** (shared `Arc<DirectLimiter>`, never removed) correctly fixes the
  quota-multiplication-by-concurrency bug. Closed at the registry level — but see WR-03,
  the production key is wrong.
- **CR-06 / T-02-19** (`MAX_NIP11_BYTES` streaming bound + client timeouts) rejects
  oversize bodies before full buffering and carries request/connect deadlines. Closed.
- **WR-01-prior / T-02-20** (backoff saturation at `failures >= 64`) correctly avoids the
  `checked_shl` high-bit-truncation zero-delay storm. Closed.

However, **CR-03 (the inclusive boundary page-back) does NOT actually close the
boundary-second completeness hole it claims to fix** against a real (non-scripted) relay.
This is a re-opened correctness BLOCKER for follow-list completeness — the core project
value. Four warnings (one a residual on the CR-02 fix, one on the CR-05 production wiring)
and three info items follow.

## Narrative Findings (AI reviewer)

## Critical Issues

### CR-01: Inclusive boundary page-back cannot recover boundary-second events a real relay cuts off (prior CR-03 fix incomplete)

**File:** `src/relay/fetch.rs:59-72` (`page_back`), `src/relay/fetch.rs:98-138` (`paginate_chunk`)

**Issue:** The CR-03 fix pages back *inclusively* to `oldest` (`page_back` returns
`Some(ts)` where `ts == oldest.created_at`, line 68) and relies on cross-window id-dedup
plus the zero-new-id guard (lines 117-133) to terminate. The doc (lines 61-67) claims
this "re-requests that second" so a sibling event sharing the oldest second that the cap
cut off is recovered.

This does not hold against a real relay when **more than `cap` events share the boundary
second**. Relays serve newest-first within an `until` window. Concretely, with `cap = 2`
and three events all at `created_at = T` (ids A < B < C) plus a newer event N at T+1:

- Window 1 (`until = now`): relay returns the newest two: `[N(T+1), A(T)]`. `oldest = T`,
  `returned = 2 >= cap` -> `page_back` returns `Some(T)`.
- Window 2 (`until = T`): relay returns the newest two at-or-before T: `[A(T), B(T)]`.
  `new_ids = 1` (B). `returned = 2 >= cap` -> `page_back` returns `Some(T)` AGAIN.
- Window 3 (`until = T`): relay returns `[A(T), B(T)]` again (same newest-first prefix at
  the pinned `until = T`). `new_ids = 0` -> **loop breaks (line 131-133). Event C(T) is
  never fetched and is silently lost.**

The loop can only make progress when the relay happens to vary which boundary-second
events it returns for an identical `until = T`. A deterministic newest-first relay (the
common case) re-serves the same prefix, so the zero-new-id guard fires while genuine
events remain unfetched. There is **no `until` value that both re-requests the missing
`T` events and guarantees forward progress** under a count-only cap: advancing `until`
below `T` skips the remaining `T` siblings; holding it at `T` re-serves the same prefix.

The passing test `inclusive_boundary_keeps_boundary_event`
(`tests/pagination.rs:37-71`) only succeeds because `ScriptedRelay` is hand-fed
`window2 = [boundary_a, boundary_b]` — it scripts the relay into returning the cut event,
which a real newest-first relay under a cap will not do. The test validates the mock, not
the production invariant.

Impact: silent truncation of any follow list where an author has more than `cap`
events/tags clustered at one second boundary — directly violating the phase claim "no
pubkeys are silently dropped across windows" (`tests/pagination.rs:3`) and the project's
core completeness value. Critically, `paginate_chunk` runs over an author *chunk*
(`fetch_complete_with_timeout` line 275-291) with one shared `until`, so the boundary
second is shared across *all* authors in the chunk; many authors' newest events colliding
on one second makes `> cap` at the boundary realistic at scale, not a corner case.

**Fix:** Count-vs-cap pagination cannot guarantee boundary completeness when `> cap`
events share the boundary second. Detect the stall and requeue instead of silently
completing — never let `new_ids == 0` break the loop while `until` is pinned at an
unexhausted boundary second:
```rust
let mut prev_until: Option<Timestamp> = None;
// ... inside the loop, after computing `oldest` and `new_ids`:
let stalled_at_boundary = returned >= cap && Some(until) == prev_until && new_ids == 0;
if stalled_at_boundary {
    return Err(RelayError::FetchTimeout(format!(
        "boundary second {until:?} holds more than cap={cap} events; \
         cannot paginate further by `until` — requeue this author chunk"
    )));
}
if new_ids == 0 { break; }
match page_back(returned, cap, oldest) {
    Some(next) => { prev_until = Some(until); until = next; }
    None => break,
}
```
A more complete fix narrows a stalled author chunk to a single author (or adds a kind/id
secondary filter) so the boundary second can be fully drained; at minimum the stall must
surface as a requeue, never as silent completion. Add a test with a *deterministic
newest-first* mock (returns the same prefix for a repeated `until`) so the regression is
caught against real relay behavior, not a scripted one.

## Warnings

### WR-01: A complete-but-slow window is misclassified as a timeout and requeued indefinitely (residual on the CR-02 fix)

**File:** `src/relay/fetch.rs:195-211` (`fetch_window_with_deadline`), `src/relay/fetch.rs:283-289`

**Issue:** The production closure calls `client.fetch_events(filter, timeout)` with the
SAME `timeout` that the enclosing `fetch_window_with_deadline` measures against, then
checks `started.elapsed() >= timeout` (line 207). `fetch_events` runs *up to* `timeout`
and returns whatever it collected. A relay that legitimately delivers a *complete*
sub-cap window but consistently takes the full deadline to do so returns `Ok` at
`elapsed ≈ timeout`; the `>=` check then converts that complete window into
`FetchTimeout` and requeues it. The next attempt against the same slow-but-complete relay
trips the identical check — a permanent requeue loop that never records the (actually
complete) follow list. The `>=` boundary also makes a fetch finishing at *exactly* the
deadline a guaranteed false timeout.

The CR-02 fix is correct for genuinely-truncated partial windows, but as written it
cannot distinguish "partial because timed out" from "complete but slow" — both surface as
`Ok` at/after the deadline. The result is a liveness defect that turns consistently-slow
honest relays into permanently-failing ones.

**Fix:** Give the inner `fetch_events` a strictly shorter budget than the outer deadline
so a complete window returns with measurable slack — e.g. pass `timeout.mul_f32(0.9)` (or
`timeout - grace`) to `fetch_events` while `fetch_window_with_deadline` measures the full
`timeout`. Alternatively treat a window as a timeout only when it returned the FULL `cap`
*and* elapsed >= timeout (a complete short window is never a timeout). Document the
slow-relay behavior either way so the operator can raise the timeout.

### WR-02: `record_notice` escalation and post-fetch `reset` share one counter, so the two backoff sources clobber each other

**File:** `src/relay/rate_limit.rs:197-234`, `src/relay/mod.rs:232-234`

**Issue:** `record_notice` increments the SAME `failures` map (lines 200-205) that
`reset` clears on a successful fetch (`src/relay/mod.rs:233`, called from
`acquire_validated_lists_client` when `result.is_ok()`). The module docs
(`rate_limit.rs:10-18`) also assign this counter to connection-failure backoff. Two
distinct escalation sources therefore share one per-relay integer:

- A `rate-limited` NOTICE (routed via `spawn_notice_consumer` -> `handle_relay_message`
  -> `record_notice`) bumps the count, but an interleaved *successful event fetch* from
  the same relay calls `reset` and wipes that politeness escalation — even though the
  relay is still rate-limiting future REQs.
- Conversely, a NOTICE inflates the index used to compute reconnect/re-arm backoff even
  when the socket is perfectly healthy.

The signals are conflated, so each can spuriously reset or inflate the other; the
backoff schedule a relay actually experiences becomes order-dependent on unrelated events.

**Fix:** Keep rate-limit-notice escalation and connection-failure escalation in separate
per-relay counters (or namespace the map key). `reset` after a successful *fetch* should
clear only the fetch/connection counter — a successful read does not prove the relay
stopped rate-limiting subsequent REQs.

### WR-03: The per-relay limiter is keyed on the whole-pool label, not an individual relay — the CR-05 per-relay quota is not per-relay in production

**File:** `src/relay/fetch.rs:266-312` (`fetch_complete_with_timeout` / `pool_label`), `src/relay/mod.rs:202-236`

**Issue:** `fetch_complete_with_timeout` derives `relay_url` from `pool_label(client)`
(line 273), which joins *every* connected relay url with `", "` (lines 307-311). That
joined string is then used as the per-relay rate-limiter key passed to
`paginate_chunk_gated` -> `registry.acquire(relay_url)` (line 277). Consequences:

1. The GCRA limiter is keyed on the entire pool, so CR-05's "per-relay quota" collapses
   into a single shared quota across all connected relays — the opposite of the T-02-10
   politeness intent (each relay should get its own quota).
2. The key string changes whenever pool membership changes (a relay drops or reconnects),
   minting a fresh full-burst limiter and discarding the accrued GCRA state — the same
   class of state-loss CR-05 set out to eliminate.

Note `acquire_validated_lists_client` already receives a real `relay_url: &str`
(`src/relay/mod.rs:204`) and uses it for `limit_cache.get_or_fetch` and
`registry.reset` — but it does NOT thread it into `fetch_complete` (line 227), so the
fetch path re-derives a different, pool-wide key. The two `relay_url` notions disagree
within one call.

**Fix:** Thread the caller's real `relay_url` (already passed to
`acquire_validated_lists_client`) through `fetch_complete` /
`fetch_complete_with_timeout` and use it as the limiter key (and the timeout label).
Reserve `pool_label` for human-readable diagnostics only, never as a registry key. If a
single fetch genuinely fans across multiple relays, gate per-relay inside the pool rather
than under one merged key.

### WR-04: NIP-11 fetch follows relay-controlled redirects to arbitrary origins (SSRF surface)

**File:** `src/relay/nip11.rs:44-50` (client build), `src/relay/nip11.rs:186-242`

**Issue:** `NIP11_CLIENT` is built without overriding `reqwest`'s default redirect policy
(follows up to 10 redirects). `fetch_limits` GETs the relay-derived `http(s)` origin
(`nip11_http_url`, lines 186-194); a hostile relay can answer with a 3xx redirect to an
internal target (`http://169.254.169.254/...`, `http://127.0.0.1:.../`) and the client
follows it, issuing a request to an internal endpoint on the crawler host. The CR-06 body
bound limits response *size* but does not prevent the redirected request itself.
`nip11_http_url` additionally passes through any non-`ws(s)` scheme unchanged
(lines 191-193). The curated relay set is operator-controlled, but the *redirect target*
is fully relay-controlled, and this path becomes more exposed once NIP-65 gossip feeds
event-sourced relay urls in.

**Fix:** Disable redirects on the NIP-11 client
(`.redirect(reqwest::redirect::Policy::none())`) — a NIP-11 document is served directly at
the origin and never legitimately needs a redirect. Optionally reject resolved hosts that
are loopback/private/link-local before issuing the request.

## Info

### IN-01: `limits_from_bytes` re-checks the size bound the streaming reader already enforces

**File:** `src/relay/nip11.rs:169-181`, `src/relay/nip11.rs:218-237`

**Issue:** `fetch_limits` streams and rejects at `MAX_NIP11_BYTES` (line 225), then calls
`limits_from_bytes`, which checks the same bound again (line 170). Harmless redundancy,
but the streaming guard means `limits_from_bytes`'s check can never fire from production —
only from the offline test seam. A future refactor that drops the streaming guard
assuming `limits_from_bytes` is the gate would be safe; one that relies on the inner check
firing in production would be wrong. Worth a clarifying comment.

**Fix:** Note in `fetch_limits` that the streaming check is the production gate, or
consolidate to a single bound.

### IN-02: `RateLimiterRegistry::acquire` returns `Result<(), RelayError>` with no fallible path

**File:** `src/relay/rate_limit.rs:166-187`

**Issue:** `acquire` cannot fail — `until_ready()` is infallible and the only other
operations are infallible map ops — yet it returns a `Result` every caller must `?`
(`paginate_chunk_gated` line 178). Minor API noise implying a failure mode that does not
exist.

**Fix:** Return `()`, or document the `Result` as forward-compat for a future fallible
quota source.

### IN-03: `MAX_ADVERTISED_LIMIT` doc example overstates what reaches the clamp

**File:** `src/relay/nip11.rs:70-78`, `src/relay/nip11.rs:113-132`

**Issue:** The doc cites a relay advertising `2_000_000_000`. The value is parsed as
`Option<i32>` (`clamp_limit`, line 113); `i32::MAX` is `2_147_483_647`, so that example
does reach the clamp and is correctly capped to `MAX_ADVERTISED_LIMIT`. But any value
above `i32::MAX` fails JSON deserialization in `RelayInformationDocument` upstream and
never reaches the clamp — the `i32` ceiling is the real first gate, the clamp handles only
the in-range-but-absurd remainder. The comment implies the clamp is the sole defense.
Cosmetic.

**Fix:** Note that out-of-`i32`-range values are rejected at parse time; the clamp covers
in-range-but-absurd advertised limits.

---

_Reviewed: 2026-06-13T07:14:00Z_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
