---
phase: 02-relay-acquisition-validation
plan: 01
subsystem: infra
tags: [nostr-sdk, governor, metrics, rust, thiserror, secp256k1, nip-11]

# Dependency graph
requires:
  - phase: 01-schema-data-contract
    provides: "store::follows::apply_follow_list(follower_id, event_id: &[u8], created_at: DateTime<Utc>, followee_ids) — the writer the Phase 2 output contract must feed"
provides:
  - "Phase 2 deps locked in Cargo.toml (nostr-sdk 0.44, governor 0.10, metrics 0.24)"
  - "relay module tree registered (mod, nip11, rate_limit, fetch) with compiling stub bodies"
  - "ingest module tree registered (mod, verify, replaceable, follow_list) with compiling stub bodies"
  - "RelayError + IngestError typed enums on the two Phase 2 boundaries"
  - "ValidatedFollowList output-contract type with the four fields apply_follow_list needs + Timestamp->DateTime<Utc> conversion in one place"
  - "Offline nostr event fixtures (keys, signed_event, forged_event, same_created_at_pair, future_dated_event) for every ingest test"
  - "02-SPIKES.md: sourced RELAY-01 reconnect + RELAY-02 NIP-11 decisions for plan 02-03"
affects: [02-02-ingest-validation, 02-03-relay-transport, 02-04-acquire-pipeline]

# Tech tracking
tech-stack:
  added: [nostr-sdk 0.44.1, nostr 0.44.3, nostr-relay-pool 0.44.1, governor 0.10.4, metrics 0.24.6]
  patterns:
    - "One thiserror enum per module boundary (StoreError / RelayError / IngestError); #[from] transparent wrap of the SDK error + value-carrying typed variants"
    - "count-and-skip vs genuine-error split: routine adversarial-input rejections return false + a metrics counter, IngestError is reserved for genuine failures"
    - "Shared output-contract struct (ValidatedFollowList) as the seam between the validation layer and the Phase 1 store writer; one-place Timestamp->DateTime<Utc> conversion"
    - "Stub-first module skeleton: plan owns shared files (Cargo.toml/lib.rs/error.rs/tests/common), Wave 2 fills disjoint module bodies"
    - "Deterministic seed-byte fixtures (keys(seed) mirroring edge_diff::pk(seed)); forge by mutating content post-sign so verify() fails"

key-files:
  created:
    - src/relay/mod.rs
    - src/relay/nip11.rs
    - src/relay/rate_limit.rs
    - src/relay/fetch.rs
    - src/ingest/mod.rs
    - src/ingest/verify.rs
    - src/ingest/replaceable.rs
    - src/ingest/follow_list.rs
    - .planning/phases/02-relay-acquisition-validation/02-SPIKES.md
  modified:
    - Cargo.toml
    - Cargo.lock
    - src/lib.rs
    - src/error.rs
    - tests/common/mod.rs

key-decisions:
  - "RELAY-01: nostr-relay-pool 0.44.1 reconnect is LINEAR (1 + diff/2) with +/-3s jitter + 60s cap, NOT exponential — plan 02-03 must layer an app-side capped-exponential-with-jitter backoff for fetch re-arm; SDK socket reconnect kept on with defaults"
  - "RELAY-02: no SDK accessor exists (RelayInformationDocument is parse-only, reqwest is dev-dep only) — plan 02-03 adds reqwest + GET Accept: application/nostr+json; defaults max_limit=500 / max_subscriptions=20 / max_filters=10 when a relay omits limitation"
  - "ValidatedFollowList stores created_at as chrono::DateTime<Utc> (already converted from nostr Timestamp) so the store boundary never re-derives it"
  - "Spikes cross-checked against vendored cargo-registry source for the exact Cargo.lock-resolved versions (Context7/ctx7 unavailable) — authoritative for this build"

patterns-established:
  - "Per-boundary thiserror enum with #[from] SDK wrap + value-carrying typed variants"
  - "count-and-skip (metrics counter, return false) vs IngestError (genuine failure) split"
  - "ValidatedFollowList contract seam feeding store::apply_follow_list"
  - "Offline deterministic nostr event fixtures in tests/common"

requirements-completed: [RELAY-01, RELAY-02]

# Metrics
duration: 18min
completed: 2026-06-12
---

# Phase 2 Plan 01: Relay/Ingest Foundation Summary

**Compiling crate with the relay + ingest module skeletons, RelayError/IngestError enums, the ValidatedFollowList output contract, offline nostr event fixtures, and sourced RELAY-01/RELAY-02 spike decisions for plan 02-03.**

## Performance

- **Duration:** ~18 min
- **Started:** 2026-06-12 (plan 02-01 execution)
- **Completed:** 2026-06-12
- **Tasks:** 4
- **Files modified:** 14 (9 created, 5 modified)

## Accomplishments
- Added the three locked Phase 2 dependencies (nostr-sdk 0.44, governor 0.10, metrics 0.24); first `cargo build` confirms nostr-sdk 0.44.1 compiles on the pinned toolchain 1.94 (RESEARCH A1).
- Registered both module trees (`relay/*`, `ingest/*`) with documented stub bodies so Wave 2 plans 02-02/02-03/02-04 fill disjoint bodies without touching the shared files this plan owns.
- Defined `RelayError` and `IngestError` mirroring the Phase 1 `StoreError` convention, with the count-and-skip vs genuine-error split documented in the `IngestError` doc comment.
- Defined the `ValidatedFollowList` output contract with the four fields `apply_follow_list` needs, plus the single-place `Timestamp` -> `DateTime<Utc>` conversion.
- Added offline (no network, no Postgres) nostr fixtures every later ingest test depends on: deterministic keys, signed events, a forged event whose `verify()` fails, a same-`created_at` tie-break pair, and a future-dated event.
- Resolved both MEDIUM-risk spikes with concrete sourced decisions: RELAY-01 reconnect is linear+jitter (not exponential, so an app-side wrapper is required) and RELAY-02 has no SDK accessor (so a `reqwest` NIP-11 fetch with documented defaults is required).

## Task Commits

Each task was committed atomically:

1. **Task 1: Add deps, register relay tree, define error enums** - `bd9d334` (feat)
2. **Task 2: ValidatedFollowList contract + ingest module tree** - `483c7dc` (feat)
3. **Task 3: signed/forged/tie-break/future-dated fixtures** - `5bf62f3` (test)
4. **Task 4: resolve RELAY-01 + RELAY-02 spikes** - `e679217` (docs)

**Plan metadata:** _(this commit)_

## Files Created/Modified
- `Cargo.toml` / `Cargo.lock` - Added nostr-sdk 0.44, governor 0.10, metrics 0.24
- `src/lib.rs` - Registered `pub mod relay; pub mod ingest;`; re-export `IngestError, RelayError, StoreError`
- `src/error.rs` - Added `RelayError` (wraps `nostr_sdk::client::Error` + relay-not-found/NIP-11/timeout) and `IngestError` (InvalidSignature/UnsolicitedEvent/FutureDated/OversizedFollowList)
- `src/relay/mod.rs` + `nip11.rs` + `rate_limit.rs` + `fetch.rs` - Relay acquisition module tree, stub bodies for plan 02-03
- `src/ingest/mod.rs` - `ValidatedFollowList` contract + `timestamp_to_datetime` + `ingest_events` orchestrator stub
- `src/ingest/verify.rs` + `replaceable.rs` + `follow_list.rs` - Validation gate stubs for plan 02-02
- `tests/common/mod.rs` - Offline nostr event fixtures (start_postgres left unchanged)
- `.planning/phases/02-relay-acquisition-validation/02-SPIKES.md` - Sourced RELAY-01 + RELAY-02 decisions

## Decisions Made
- **RELAY-01 (spike):** nostr-relay-pool 0.44.1 `calculate_retry_interval` is `retry_interval * (1 + (attempts-successes)/2)` capped at 60s with ±3s jitter — LINEAR, not exponential. The jitter requirement is met by the SDK; the exponential requirement is not. Plan 02-03 must add an app-side capped-exponential-with-jitter backoff for the fetch re-arm decision while keeping SDK socket reconnect on. RELAY-01 is NOT satisfied on the SDK default alone.
- **RELAY-02 (spike):** No SDK NIP-11 accessor exists; `RelayInformationDocument` is parse-only and `reqwest` is a dev-dep only. Plan 02-03 must add `reqwest` (rustls) and fetch via `GET` with `Accept: application/nostr+json`, then `from_json`. Defaults when a relay omits `limitation`: `max_limit=500`, `max_subscriptions=20`, `max_filters=10` (all config-overridable; negative/zero advertised values fall back to default).
- **Contract conversion:** `ValidatedFollowList.created_at` is stored as the already-converted `DateTime<Utc>` so the store boundary never re-derives it; the conversion lives only in `ingest::timestamp_to_datetime` / `from_event`.
- **Spike method:** Context7/ctx7 was unavailable in this environment, so both findings were cross-checked against the vendored cargo-registry source for the exact `Cargo.lock`-resolved versions (nostr-sdk 0.44.1, nostr-relay-pool 0.44.1, nostr 0.44.3) — authoritative for this build.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Used `Timestamp::as_secs()` instead of deprecated `as_u64()`**
- **Found during:** Task 2 (ValidatedFollowList conversion helper)
- **Issue:** The RESEARCH example used `Timestamp::as_u64()`, which is `#[deprecated]` in nostr 0.44.3 and emitted a build warning.
- **Fix:** Switched the conversion in `timestamp_to_datetime` to `as_secs()`.
- **Files modified:** src/ingest/mod.rs
- **Verification:** `cargo build` clean, zero warnings.
- **Committed in:** 483c7dc (Task 2 commit)

**2. [Rule 3 - Blocking] Added `#![allow(dead_code)]` to tests/common shared fixture module**
- **Found during:** Task 3 (event fixtures)
- **Issue:** `tests/common/mod.rs` is a shared `mod common;` included by every integration-test binary; each binary uses only a subset, so the not-yet-consumed ingest fixtures (forged_event, same_created_at_pair, future_dated_event) produced dead-code warnings across all five test binaries until plan 02-02's tests land.
- **Fix:** Added a module-level `#![allow(dead_code)]` with an explaining doc comment (standard for shared test-fixture modules).
- **Files modified:** tests/common/mod.rs
- **Verification:** `cargo build --tests` clean, zero warnings.
- **Committed in:** 5bf62f3 (Task 3 commit)

---

**Total deviations:** 2 auto-fixed (1 bug, 1 blocking)
**Impact on plan:** Both are minor correctness/cleanliness fixes keeping the build warning-free. No scope creep; no behavior changes to the stub APIs.

## Issues Encountered
- `cargo` was not on the non-interactive shell PATH (rustup-managed at `~/.cargo/bin`); resolved by prefixing `PATH="$HOME/.cargo/bin:$PATH"` on each cargo invocation. No project change required.

## User Setup Required
None - no external service configuration required. (Plan 02-03 will add a `reqwest` dependency per the RELAY-02 spike; supply-chain legitimacy check is flagged there.)

## Next Phase Readiness
- The crate compiles with both module trees registered, both error enums, and the `ValidatedFollowList` contract — Wave 2 (02-02 ingest, 02-03 relay) can run in parallel filling disjoint module bodies without touching Cargo.toml/lib.rs/error.rs/tests/common.
- Plan 02-02 has the verify/replaceable/follow_list gate stubs + the fixtures it needs.
- Plan 02-03 has the relay stubs + the two recorded spike decisions (must add `reqwest`, must add app-side exponential backoff).
- Plan 02-04 has the `ValidatedFollowList` type + `ingest_events` orchestrator seam to wire fetch -> ingest.
- No blockers. RELAY-01/RELAY-02 requirement *satisfaction* is gated on plan 02-03 shipping the recorded spike decisions (the spikes resolve the open API questions; the code lands in 02-03).

---
*Phase: 02-relay-acquisition-validation*
*Completed: 2026-06-12*

## Self-Check: PASSED

All 9 created files present on disk; all 4 task commits (bd9d334, 483c7dc, 5bf62f3, e679217) present in git history. `cargo build`, `cargo build --tests`, and `cargo test --lib` all succeed.
