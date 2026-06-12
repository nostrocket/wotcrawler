# Phase 2 Spikes — API verification (RELAY-01, RELAY-02)

**Resolved:** 2026-06-12 (plan 02-01 Task 4)
**Method:** Context7 MCP was unavailable in this environment (no `ctx7` CLI on PATH), so both findings were cross-checked against the **resolved source in `~/.cargo/registry`** for the exact versions pinned in `Cargo.lock`:

- `nostr-sdk` 0.44.1
- `nostr-relay-pool` 0.44.1
- `nostr` 0.44.3

These are the versions the crate actually builds against (verified in `Cargo.lock`), so reading their vendored source is authoritative for this build — stronger than docs.rs, which describes the published API but not the exact reconnect arithmetic.

---

## RELAY-01: reconnect backoff/jitter

### Finding

nostr-relay-pool 0.44.1 auto-reconnect provides **LINEAR (not exponential) backoff WITH ±3s jitter and a 60s cap**. The defaults are `reconnect = true`, `retry_interval = 10s`, `adjust_retry_interval = true`.

The exact algorithm is in `RelayConnectionInner::calculate_retry_interval`:

```rust
// nostr-relay-pool-0.44.1/src/relay/inner.rs:613-650
fn calculate_retry_interval(&self) -> Duration {
    if self.opts.adjust_retry_interval {
        let diff: u32 = self.stats.attempts().saturating_sub(self.stats.success()) as u32;
        let multiplier: u32 = 1 + (diff / 2);                 // LINEAR growth
        let adaptive_interval: Duration = self.opts.retry_interval * multiplier;
        let mut interval = cmp::min(adaptive_interval, MAX_RETRY_INTERVAL); // capped
        let jitter: i8 = rand::thread_rng().gen_range(JITTER_RANGE);        // ±3s
        // ... saturating add/sub the jitter ...
        return interval;
    }
    self.opts.retry_interval
}
```

Constants (`nostr-relay-pool-0.44.1/src/relay/constants.rs`):
- `DEFAULT_RETRY_INTERVAL = 10s` (line 20)
- `MAX_RETRY_INTERVAL = 60s` (line 23)
- `JITTER_RANGE = -3..=3` (seconds) (line 24)

So with `adjust_retry_interval = true`, the delay grows as `retry_interval * (1 + (attempts - successes)/2)`, capped at 60s, with a uniform ±3s jitter applied after the cap. The growth is **linear in the failure count** (every 2 failed attempts adds one `retry_interval`), NOT exponential (no doubling, no `2^n`).

The jitter requirement of RELAY-01 **is** satisfied by the SDK default (the source comment explicitly states the jitter exists "to avoid situations where multiple relays reconnect simultaneously after a failure ... prevent synchronized retry storms" — exactly the Pitfall 8 thundering-herd concern). The **exponential** requirement is **NOT** satisfied — the SDK growth is linear.

### Source

- `nostr-relay-pool-0.44.1/src/relay/inner.rs:613-650` (`calculate_retry_interval`, the `1 + diff/2` linear multiplier + jitter)
- `nostr-relay-pool-0.44.1/src/relay/inner.rs:582-593` (the reconnect loop that sleeps `calculate_retry_interval()`)
- `nostr-relay-pool-0.44.1/src/relay/constants.rs:20,23,24` (`DEFAULT_RETRY_INTERVAL=10s`, `MAX_RETRY_INTERVAL=60s`, `JITTER_RANGE=-3..=3`)
- `nostr-relay-pool-0.44.1/src/relay/options.rs:21-42,102-116` (`reconnect`/`retry_interval`/`adjust_retry_interval` fields, defaults, builders)
- `Cargo.lock` — `nostr-relay-pool` resolved to `0.44.1`

### DECISION (plan 02-03 must implement)

**Do NOT mark RELAY-01 satisfied on the SDK default alone.** The SDK gives jitter + cap + linear growth, but RELAY-01 mandates *exponential* backoff.

Plan 02-03 Task 1 MUST layer an **app-side capped-exponential-with-jitter backoff** that governs *when the crawler re-arms a relay for fetching* after repeated connection failures, on top of nostr-sdk's own socket-level reconnect (which we keep enabled with its defaults for the low-level websocket churn).

Concrete approach to implement:
1. Keep nostr-sdk reconnect ON with defaults (`reconnect=true`, `retry_interval=10s`, `adjust_retry_interval=true`) — it owns the websocket lifecycle; do not fight it.
2. Add an **app-side per-relay backoff** in `src/relay/rate_limit.rs::backoff` (already stubbed) computing `delay = min(base * 2^failures, cap)` then applying full random jitter (e.g. `delay = random_between(0, delay)` "full jitter", or `delay ± jitter_frac`). Suggested params, config-overridable: `base = 1s`, `cap = 5min`, failures counted per-relay and reset on a successful fetch. This is the same backoff used for the RELAY-04 `rate-limited` notice path, so the two reuse one helper.
3. The exponential schedule applies to the **crawler's fetch re-arm decision** (how long this relay is parked before we try fetching from it again), distinct from the SDK's socket reconnect interval — both run; the app-side one is what makes RELAY-01's "exponential backoff with jitter" true at the acquisition layer.

This is a tracked implementation task, not an assumption: RELAY-01 is satisfied only once plan 02-03 ships the app-side exponential+jitter wrapper.

---

## RELAY-02: NIP-11 accessor

### Finding

There is **NO nostr-sdk / nostr-relay-pool 0.44 accessor** that fetches a relay's NIP-11 document. The `RelayInformationDocument` **type** exists in the `nostr` crate (`nostr-0.44.3/src/nips/nip11.rs:17`), but it is **parse-only**: it exposes `RelayInformationDocument::new()` and serde (de)serialization (`from_json`), and has **no** HTTP fetch method. A registry-wide grep for `RelayInformationDocument` / `fn document` / `nip11` across `nostr-relay-pool-0.44.1/src` and `nostr-sdk-0.44.1/src` returned **zero** hits — neither `Relay`, `RelayPool`, nor `Client` exposes a `.document()`-style accessor. In `nostr` 0.44.3, `reqwest` is only a **dev-dependency** (used by the bundled `examples/nip11.rs`), confirming the SDK deliberately does not ship the HTTP fetch.

The `Limitation` fields are `Option<i32>` (`nostr-0.44.3/src/nips/nip11.rs:75-83`):
- `max_subscriptions: Option<i32>` (line 79)
- `max_filters: Option<i32>` (line 81)
- `max_limit: Option<i32>` (line 83)

and `RelayInformationDocument.limitation` is itself `Option<Limitation>` (line 33) — a relay may omit the whole block.

### Source

- `nostr-0.44.3/src/nips/nip11.rs:17` (`pub struct RelayInformationDocument`), `:33` (`pub limitation: Option<Limitation>`), `:75-83` (`Limitation { max_subscriptions, max_filters, max_limit: Option<i32> }`), `:63` (`new()` — no fetch)
- `nostr-0.44.3/examples/nip11.rs` (the canonical fetch pattern: a raw `reqwest` GET with `Accept: application/nostr+json` then `RelayInformationDocument::from_json(&json)`)
- `nostr-0.44.3/Cargo.toml:281` (`reqwest` is a **dev-dependency** only — not available to library code)
- Registry grep: zero `RelayInformationDocument` / NIP-11 fetch accessor in `nostr-relay-pool-0.44.1/src` or `nostr-sdk-0.44.1/src`
- `Cargo.lock` — `nostr` resolved to `0.44.3`

### DECISION (plan 02-03 must implement)

Use the **`reqwest` GET fallback** — there is no SDK accessor.

Plan 02-03 Task 2 MUST:
1. **Add `reqwest` to `Cargo.toml` `[dependencies]`** (it is NOT currently a dependency of this crate; `nostr` only carries it as a dev-dep). Use `reqwest` with `rustls` TLS to match the project's `tls-rustls` posture (`sqlx` already uses rustls) and default features off where practical. (`reqwest` legitimacy must be confirmed via `gsd-tools query package-legitimacy check --ecosystem crates` before adding, per the Phase 2 threat model T-02-SC supply-chain boundary.)
2. Implement `src/relay/nip11.rs::fetch_limits` as: convert the relay's `wss://`/`ws://` url to its `https://`/`http://` origin, `GET` it with header `Accept: application/nostr+json`, read the body text, and `RelayInformationDocument::from_json(&body)` (exactly the bundled example pattern).
3. Read the three `limitation` fields and apply these **sane defaults when a relay omits them** (the whole `limitation` block or any individual field being `None`):
   - **`max_limit` default = `500`** — the de-facto common relay cap and the value RESEARCH Pitfall 1 cites; this feeds the pagination planner's effective per-window cap `min(requested_limit, relay_max_limit)`.
   - **`max_subscriptions` default = `20`** — a conservative lower bound so the crawler never assumes more concurrent REQs than a silent relay likely allows.
   - **`max_filters` default = `10`** — conservative; combined with the "one filter per REQ, author-chunked" design (RESEARCH Pattern 2) the crawler stays well under any real relay's filter cap.
   All three defaults are config-overridable; they exist so a relay that omits `limitation` is still crawled politely rather than crashing the planner. Negative/zero advertised values are treated as "use the default" (defensive against adversarial NIP-11 docs — never `unwrap()` relay-supplied numbers).

RELAY-02 is satisfied once plan 02-03 ships `fetch_limits` against this recorded decision and the pagination planner caps each filter's `limit` at the discovered (or defaulted) `max_limit`.
