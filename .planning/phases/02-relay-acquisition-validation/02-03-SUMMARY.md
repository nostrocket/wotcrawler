---
phase: 02-relay-acquisition-validation
plan: 03
subsystem: relay-transport
tags: [nostr-sdk, governor, reqwest, nip-11, pagination, backoff, rate-limit, rust]

# Dependency graph
requires:
  - phase: 02-relay-acquisition-validation
    provides: "plan 02-01 relay module stubs (mod/nip11/rate_limit/fetch), RelayError enum, Cargo deps (nostr-sdk 0.44, governor 0.10, metrics 0.24), and the sourced RELAY-01/RELAY-02 spike decisions in 02-SPIKES.md"
provides:
  - "connect_curated(relays, ReconnectPolicy) -> Client: signer-less pool wired with SDK socket reconnect (RELAY-01)"
  - "rate_limit::backoff_delay capped-exponential-with-jitter schedule satisfying RELAY-01's exponential requirement (SDK reconnect is linear)"
  - "rate_limit::RateLimiterRegistry: per-relay governor GCRA gate (acquire/until_ready) + rate-limited/blocked notice handling (RELAY-04)"
  - "nip11::fetch_limits + LimitCache: HTTP NIP-11 fetch (Accept: application/nostr+json) + per-relay limit cache with defaults 500/20/10 (RELAY-02)"
  - "fetch::fetch_complete + paginate_chunk + page_back: author-chunked until-window pagination, count-vs-cap page-back, EOSE never trusted, explicit per-fetch timeout (RELAY-03)"
affects: [02-04-acquire-pipeline]

# Tech tracking
tech-stack:
  added: [reqwest 0.12 (rustls-tls, no default openssl), rand 0.9]
  patterns:
    - "Inspectable policy wrapper (ReconnectPolicy) over a crate-private SDK type (RelayOptions fields are pub(super)) so the connect path is asserted without live sockets"
    - "Pure decision functions (page_back, backoff_delay_unjittered, classify_notice, limits_from_doc) split from I/O so adversarial/edge logic is unit-tested offline"
    - "Injected async fetch fn (paginate_chunk over F: FnMut(Filter) -> Fut) so the page-back loop is exercised with scripted windows instead of a full ws relay mock"
    - "Per-relay state registries (governor limiter + consecutive-failure counter) behind Mutex<HashMap>, cheap to share via Arc"
    - "Relay-supplied numbers clamped, never unwrapped: non-positive NIP-11 limits fall back to defaults (T-02-13)"

key-files:
  created:
    - tests/reconnect_policy.rs
    - tests/rate_limit_backoff.rs
    - tests/nip11_limits.rs
    - tests/pagination.rs
    - tests/mock_relay/mod.rs
  modified:
    - Cargo.toml
    - Cargo.lock
    - src/relay/mod.rs
    - src/relay/nip11.rs
    - src/relay/rate_limit.rs
    - src/relay/fetch.rs

key-decisions:
  - "RELAY-01 exponential backoff implemented app-side (backoff_delay = min(base*2^failures, cap) + full jitter over [delay/2, delay]); SDK socket reconnect kept on with defaults (linear+jitter) per the 02-SPIKES decision — the two coexist"
  - "RELAY-02 NIP-11 fetched via reqwest GET with Accept: application/nostr+json (no SDK accessor); defaults max_limit=500 / max_subscriptions=20 / max_filters=10; non-positive advertised values treated as default (adversarial-safe)"
  - "custom RelayOptions applied via client.pool().add_relay(url, opts) — Client::add_relay silently uses client-default opts and would not carry our reconnect policy"
  - "mock relay implemented as the documented injected-fetch-fn alternative (a full nostr-sdk 0.44 ws relay mock is impractical offline); page_back logic is the same code production runs"
  - "fetch_complete dedups raw events by EventId across chunks/windows and emits ONLY the unverified stream — ingest wiring is plan 02-04"

patterns-established:
  - "Inspectable policy wrapper over a crate-private SDK options type"
  - "Pure decision fn split from I/O for offline edge-case testing"
  - "Injected async fetch fn for testing transport loops without a live socket"
  - "Clamp-not-unwrap on all relay-supplied numbers"

requirements-completed: [RELAY-01, RELAY-02, RELAY-03, RELAY-04]

# Metrics
duration: 20min
completed: 2026-06-12
---

# Phase 2 Plan 03: Relay Acquisition Transport Summary

**The acquisition half of Phase 2: a signer-less curated-set connect with app-side capped-exponential-with-jitter backoff (RELAY-01), HTTP NIP-11 limit fetch + per-relay cache (RELAY-02), author-chunked until-window pagination that never trusts EOSE (RELAY-03), and a per-relay governor gate with rate-limited/blocked notice handling (RELAY-04) — all proven by four offline test suites.**

## Performance

- **Duration:** ~20 min
- **Started:** 2026-06-12
- **Completed:** 2026-06-12
- **Tasks:** 3
- **Files modified:** 12 (5 created, 7 modified — Cargo.lock counted)

## Accomplishments

- **RELAY-01:** `connect_curated` builds a read-only `Client` and adds each curated relay through `client.pool().add_relay(url, opts)` with a `ReconnectPolicy`-derived `RelayOptions` (SDK socket reconnect on). The exponential requirement the SDK does not meet (its reconnect is linear, per 02-SPIKES) is satisfied at the acquisition layer by `rate_limit::backoff_delay` — `min(base * 2^failures, cap)` with full jitter over `[delay/2, delay]`, desynchronizing relays so a shared outage cannot cause a reconnect storm (Pitfall 8 / T-02-09).
- **RELAY-04:** `RateLimiterRegistry` keeps one governor GCRA limiter per relay (`acquire` awaits `until_ready` before every REQ) and a per-relay consecutive-failure counter. A `rate-limited` notice escalates the backoff and fires the `relay_rate_limited` metric; `blocked`/`restricted` stops the relay and fires `relay_blocked`. Backoff is reset on a successful fetch.
- **RELAY-02:** `nip11::fetch_limits` GETs the relay's `http(s)` origin with `Accept: application/nostr+json` and parses `RelayInformationDocument` (no SDK accessor exists). `LimitCache` memoizes per-relay `RelayLimits`; a fetch failure degrades to defaults (`500/20/10`) so a bad/absent doc never blocks the crawl. Non-positive advertised values fall back to defaults (T-02-13). The cached `max_limit` is the effective pagination cap.
- **RELAY-03:** `paginate_chunk` loops each author chunk, building `Filter::authors().kind().limit(cap).until(until)`; `page_back` compares the returned count against the effective cap (`min(requested, relay max_limit)`) and pages to `oldest - 1`, stopping only when a window returns strictly fewer than the cap. EOSE is never consulted. `fetch_complete` chunks authors under `max_authors`, dedups by `EventId`, and calls `client.fetch_events` with an explicit timeout — a timed-out window surfaces as `FetchTimeout` for requeue (Pitfall 9 / T-02-12). It emits only the raw unverified stream (ingest wiring is plan 02-04).

## Task Commits

1. **Task 1: connect curated set + governor + backoff (RELAY-01, RELAY-04)** — `77787b9` (feat)
2. **Task 2: NIP-11 limit fetch + per-relay cache (RELAY-02)** — `b634180` (feat)
3. **Task 3: author-chunked until-window pagination (RELAY-03)** — `dd0cc66` (feat)

**Plan metadata:** _(final docs commit)_

## Files Created/Modified

- `Cargo.toml` / `Cargo.lock` — added `reqwest 0.12` (rustls-tls, no default openssl) for the NIP-11 fetch and `rand 0.9` for backoff jitter
- `src/relay/mod.rs` — `ReconnectPolicy` + `connect_curated`
- `src/relay/rate_limit.rs` — `RateLimiterRegistry`, `backoff_delay` / `backoff_delay_unjittered`, `classify_notice` / `NoticeKind`
- `src/relay/nip11.rs` — `RelayLimits`, `limits_from_doc` / `limits_from_json`, `fetch_limits`, `LimitCache`
- `src/relay/fetch.rs` — `page_back`, `paginate_chunk`, `fetch_complete` / `fetch_complete_with_timeout`, id dedup
- `tests/reconnect_policy.rs`, `tests/rate_limit_backoff.rs`, `tests/nip11_limits.rs`, `tests/pagination.rs` — the four required suites
- `tests/mock_relay/mod.rs` — scripted-window injected-fetch harness

## Decisions Made

- **Exponential backoff is app-side (RELAY-01).** Per 02-SPIKES, nostr-relay-pool's reconnect is linear (`1 + diff/2`) with jitter; that keeps the websocket lifecycle healthy but does not meet RELAY-01's *exponential* mandate. `backoff_delay` provides the exponential+jitter schedule for the fetch re-arm / rate-limited path; the SDK socket reconnect stays on with defaults. Both run, distinct concerns.
- **NIP-11 via reqwest, not the SDK (RELAY-02).** `RelayInformationDocument` is parse-only; reqwest is added (rustls, matching the project's tls-rustls posture). Defaults `500/20/10` when a relay omits `limitation`; zero/negative advertised values are treated as "use the default" so a hostile NIP-11 doc cannot shrink the cap to a denial (T-02-13).
- **Custom options require the pool API.** `Client::add_relay` uses the client-default `RelayOptions`; to attach our `ReconnectPolicy` the connect path goes through `client.pool().add_relay(url, opts)`.
- **Mock relay = injected fetch fn.** The plan sanctioned this when a full ws mock is impractical against nostr-sdk 0.44. `paginate_chunk` is generic over the fetch fn, so the page-back loop the test drives is byte-for-byte what `fetch_complete` runs in production; only the leaf `client.fetch_events` call is swapped.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] backoff_delay_unjittered truncated the exponential factor to ~0 at high failure counts**
- **Found during:** Task 1 (`app_side_backoff_saturates_at_cap` test failed: returned `0ns`, expected `300s` at `failures=63`).
- **Issue:** The first implementation computed `2^failures` as `u64` then cast to `u32` for `Duration::checked_mul`. At `failures >= 32` the cast wrapped to a small (or zero) value, so the "saturate at cap" path instead produced a near-zero delay — a relay that failed many times would have been retried almost immediately (the exact thundering-herd / hammering behavior RELAY-01 must prevent).
- **Fix:** Reworked the schedule to compute `base_nanos << failures` in `u128` with saturation, short-circuit `failures >= 128` to `cap`, and clamp to `cap` before converting back to `Duration`. Never truncates.
- **Files modified:** src/relay/rate_limit.rs
- **Verification:** `cargo test --test reconnect_policy` — `saturates_at_cap` now passes at `failures` 20/63/255.
- **Committed in:** `77787b9` (Task 1 commit)

**2. [Rule 3 - Blocking] Corrected the RelayInformationDocument import path**
- **Found during:** Task 2 (build error E0432: `no RelayInformationDocument in the root`).
- **Issue:** Imported `nostr_sdk::RelayInformationDocument`; the type re-exports under `nostr_sdk::nips::nip11::RelayInformationDocument`, and `JsonUtil` (the trait providing `from_json`) is at the root.
- **Fix:** Split the import into `nostr_sdk::nips::nip11::RelayInformationDocument` + `nostr_sdk::JsonUtil`.
- **Files modified:** src/relay/nip11.rs
- **Verification:** `cargo test --test nip11_limits` compiles and passes.
- **Committed in:** `b634180` (Task 2 commit)

---

**Total deviations:** 2 auto-fixed (1 bug, 1 blocking). No architectural changes, no scope creep, no checkpoints.

## Threat Model Coverage

| Threat ID | Disposition | Where mitigated |
|-----------|-------------|-----------------|
| T-02-09 (reconnect storm) | mitigate | `backoff_delay` exponential+full-jitter desynchronizes relays (Task 1) |
| T-02-10 (IP-ban / impolite) | mitigate | per-relay governor gate + rate-limited backoff + blocked stop (Task 1) |
| T-02-11 (completeness loss) | mitigate | `page_back` count-vs-cap; EOSE never trusted (Task 3) |
| T-02-12 (silent stall) | mitigate | explicit per-fetch timeout → `FetchTimeout` requeue (Task 3) |
| T-02-13 (hostile NIP-11) | accept | non-positive limits clamped to defaults; one-relay blast radius (Task 2) |

No new security surface beyond the plan's threat model. The one outbound network surface added — the reqwest NIP-11 GET — is exactly the RELAY-02 boundary already in the threat register (T-02-13).

## Issues Encountered

- `cargo` is not on the non-interactive shell PATH (rustup-managed at `~/.cargo/bin`); each invocation is prefixed with `PATH="$HOME/.cargo/bin:$PATH"`. No project change. (`gsd-tools` / `ctx7` were also unavailable; API facts were verified directly against the vendored cargo-registry source for the Cargo.lock-pinned versions, consistent with the 02-01 spike method.)

## Known Stubs

None. All four relay module bodies are fully implemented; no placeholder/empty-return paths remain.

## User Setup Required

None — all four test suites run offline (no network, no Postgres). Live-relay politeness (RELAY-04) remains the documented manual-only verification in 02-VALIDATION (sustained run against one curated relay; not in CI).

## Next Phase Readiness

- Plan 02-04 (Wave 3) can wire `fetch::fetch_complete` → `ingest::ingest_events`: the fetch loop emits the raw deduplicated `Vec<Event>` stream the ingest gate validates, `LimitCache` feeds the effective `max_limit` per relay, and `connect_curated` returns the `Client` to drive.
- `RateLimiterRegistry::acquire` is ready to gate every REQ in the wired pipeline; `RateLimiterRegistry::reset` should be called on a relay's successful fetch.
- No blockers. RELAY-01..04 are satisfied by the shipped code against the recorded spike decisions.

---
*Phase: 02-relay-acquisition-validation*
*Completed: 2026-06-12*

## Self-Check: PASSED

All 10 listed files (4 relay sources, 4 test suites, mock-relay fixture, SUMMARY) present on disk; all 3 task commits (77787b9, b634180, dd0cc66) present in git history. `cargo build --tests` is warning-free and all four relay test suites (17 tests) pass.
