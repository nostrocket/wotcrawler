---
phase: 02-relay-acquisition-validation
plan: 02
subsystem: ingest
tags: [nostr-sdk, metrics, rust, secp256k1, nip-65, validation, adversarial-input]

# Dependency graph
requires:
  - phase: 02-relay-acquisition-validation
    plan: 01
    provides: "ValidatedFollowList contract, IngestError enum, ingest module stubs (verify/replaceable/follow_list/mod), and offline nostr event fixtures (keys, signed_event, forged_event, same_created_at_pair, future_dated_event)"
  - phase: 01-schema-data-contract
    provides: "store::follows::apply_follow_list — the Phase 1 writer the ValidatedFollowList output feeds (id-equality short-circuit GRAPH-02)"
provides:
  - "ingest::verify::accept — Event::verify() (id+sig) + kind/author gate; count-and-skip ingest_invalid_signature / ingest_unsolicited (INGEST-01)"
  - "ingest::ingest_events orchestrator — cross-relay HashSet<EventId> seen-set + per-author replaceable resolve + bounded extract, emitting ValidatedFollowList (INGEST-02)"
  - "ingest::replaceable::pick_winner — kind-agnostic future-clamp + newest-wins + lowest-id tie-break resolver; ingest_future_dated counter (INGEST-03, INGEST-05)"
  - "ingest::follow_list::followee_pubkeys — public_keys() extraction + dedup + self-drop + configurable cap; ingest_oversized_follow_list counter (INGEST-04)"
  - "16 offline ingest tests across verify_gate / dedup / replaceable / relay_list / follow_list_bounds"
affects: [02-04-acquire-pipeline]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Adversarial-input gate: Event::verify() (id recompute + secp256k1 sig) before any acceptance; never verify_signature() alone (catches relay id/content mismatch)"
    - "count-and-skip metrics on every reject path (ingest_invalid_signature / ingest_unsolicited / ingest_future_dated / ingest_oversized_follow_list); the function returns false/None, never an IngestError"
    - "Kind-agnostic replaceable resolver over &Event so kind:3 ContactList and kind:10002 RelayList share one pick_winner (INGEST-05)"
    - "max_by with .then_with(|b,a| b.id.cmp(&a.id)) to make the LOWEST EventId win on equal created_at (max_by returns the last maximum)"
    - "reject-not-truncate on oversized follow lists (silent truncation corrupts the graph)"

key-files:
  created:
    - tests/verify_gate.rs
    - tests/dedup.rs
    - tests/replaceable.rs
    - tests/relay_list.rs
    - tests/follow_list_bounds.rs
  modified:
    - src/ingest/verify.rs
    - src/ingest/replaceable.rs
    - src/ingest/follow_list.rs
    - src/ingest/mod.rs

key-decisions:
  - "ingest_events takes a &HashSet<PublicKey> requested-author set (added to the 02-01 stub signature) — the verify gate needs it to drop unsolicited authors; the stub omitted it"
  - "EventId derives Ord directly in nostr 0.44.3 (32-byte big-endian) — no as_bytes() comparison needed for the tie-break"
  - "Oversized follow lists default to REJECT + count (return None), not truncation — Open Question 4 resolution"
  - "Validation-map binary::module test paths resolve via `cargo test --test <binary> <module>` (cargo filters on the in-binary path, and the binary name is not a path prefix)"

patterns-established:
  - "Adversarial-input verify gate: Event::verify() then kind/author match, count-and-skip"
  - "Kind-agnostic replaceable resolver with deterministic lowest-id tie-break"
  - "reject-not-truncate bounded extraction with metrics counters"

requirements-completed: [INGEST-01, INGEST-02, INGEST-03, INGEST-04, INGEST-05]

# Metrics
duration: 6min
completed: 2026-06-12
---

# Phase 2 Plan 02: Ingest Validation Summary

**The adversarial-input gate that turns raw, untrusted relay events into a deduplicated, self-drop-filtered, newest-wins `ValidatedFollowList` — signature/kind/author verification, cross-relay id dedup, future-clamp + lowest-id tie-break replaceable resolution (kind:3 and kind:10002), and reject-not-truncate follow-list bounds — all proven offline by 16 tests.**

## Performance

- **Duration:** ~6 min
- **Started:** 2026-06-12
- **Completed:** 2026-06-12
- **Tasks:** 3 (all TDD)
- **Files modified:** 9 (5 created, 4 modified)

## Accomplishments

- **INGEST-01 (verify gate):** `verify::accept` calls `Event::verify()` (id recomputation + secp256k1 signature — never `verify_signature()` alone, so a relay returning an event whose id≠content is caught), then asserts `kind == want_kind && pubkey ∈ requested`. Forged events count `ingest_invalid_signature`; wrong-kind/wrong-author count `ingest_unsolicited`; both return `false`. No `unwrap()` on any event field.
- **INGEST-02 (dedup):** The `ingest_events` orchestrator runs a cross-call `HashSet<EventId>` seen-set so the same event id from two relays is processed at most once.
- **INGEST-03 (replaceable):** `pick_winner` filters `created_at > now + future_clamp_secs` (saturating add) so future-dated junk can never win, takes the highest `created_at`, and breaks ties to the LOWEST `EventId` (deterministic NIP-01 rule, no flapping). `future_clamp_secs` is a parameter, not hardcoded.
- **INGEST-04 (bounds):** `followee_pubkeys` extracts via `Tags::public_keys()` (skips malformed/non-standard p-tags), drops self-follows (D-08), dedups repeated p-tags, and rejects+counts (`ingest_oversized_follow_list`) any list over the configurable `follow_cap` — no truncation, no panic.
- **INGEST-05 (kind:10002):** The resolver is kind-agnostic over `&Event`; `tests/relay_list.rs` proves the identical `pick_winner` selects the newest valid kind:10002 RelayList and clamps future-dated ones.
- The orchestrator assembles `ValidatedFollowList { follower_pubkey, event_id, created_at, followee_pubkeys }` from the winning event via the single-place `from_event` conversion — the Phase 3 / 02-04 store-writer seam.

## Task Commits

Each task was committed atomically (TDD RED → GREEN where applicable):

1. **Task 1 RED: failing verify-gate + dedup tests** - `740beef` (test)
2. **Task 1 GREEN: verify gate + dedup orchestrator (+ replaceable/follow_list impls for end-to-end)** - `576d372` (feat)
3. **Task 2: replaceable resolution + kind:10002 tests** - `595e5f8` (test)
4. **Task 3: follow-list bounds tests** - `038d87e` (test)

**Plan metadata:** _(final docs commit)_

Note: the three submodule implementations (`verify`, `replaceable`, `follow_list`) all landed in the Task 1 GREEN commit because the orchestrator's end-to-end dedup test requires the full pipeline to compile and run. Tasks 2 and 3 then added their dedicated focused tests against the already-implemented resolver and extractor.

## Files Created/Modified

- `src/ingest/verify.rs` — `accept` gate: `Event::verify()` + kind/author match + count-and-skip metrics.
- `src/ingest/replaceable.rs` — `pick_winner`: future clamp + newest-wins + lowest-id tie-break, kind-agnostic.
- `src/ingest/follow_list.rs` — `followee_pubkeys`: `public_keys()` + dedup + self-drop + reject-not-truncate cap.
- `src/ingest/mod.rs` — `ingest_events` orchestrator: seen-set dedup → gate → group-by-author → resolve → extract → emit `ValidatedFollowList`.
- `tests/verify_gate.rs`, `tests/dedup.rs`, `tests/replaceable.rs`, `tests/relay_list.rs`, `tests/follow_list_bounds.rs` — 16 offline tests.

## Decisions Made

- **`ingest_events` requested-author set:** Added a `requested: &HashSet<PublicKey>` parameter to the 02-01 orchestrator stub. The verify gate needs the solicited-author set to drop unsolicited authors (Pitfall 4 / INGEST-01); the stub signature omitted it. Documented as deviation Rule 3 (blocking — the gate is incorrect without it).
- **`EventId` Ord:** nostr 0.44.3 derives `Ord`/`PartialOrd` on `EventId` directly (over its 32-byte big-endian id), so the tie-break is `b.id.cmp(&a.id)` inside `max_by` with no `as_bytes()` comparison. `max_by` returns the last maximum, so ordering equal-timestamp events by descending id makes the lowest id the winner.
- **Reject-not-truncate:** An oversized follow list returns `None` (rejected + counted), never a silently truncated set — truncation would drop real follows and corrupt the graph (Open Question 4).
- **Validation-map command form:** The plan's `binary::module` test paths (e.g. `verify_gate::unsolicited`) resolve via `cargo test --test verify_gate unsolicited`. A bare `cargo test verify_gate::unsolicited` matches nothing because cargo filters on the in-binary test path and the test-binary name is not part of that path. The named submodules (`unsolicited`, `future_clamp`, `tie_break`, `malformed`, `cap`) all exist and pass; only the invocation form differs from the literal string in the validation map.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Added `requested: &HashSet<PublicKey>` parameter to `ingest_events`**
- **Found during:** Task 1 (orchestrator wiring)
- **Issue:** The 02-01 `ingest_events` stub signature took no requested-author set, but `verify::accept` requires it to drop unsolicited authors (INGEST-01 / Pitfall 4). Without it the gate cannot enforce author solicitation.
- **Fix:** Added `requested: &HashSet<PublicKey>` as the third parameter; updated `tests/dedup.rs` to pass it. This matches the plan prose ("consuming raw events + a requested-author set").
- **Files modified:** src/ingest/mod.rs, tests/dedup.rs
- **Commit:** 576d372

**2. [Rule 3 - Blocking] Bound candidate arrays to locals in tests to satisfy borrow checker**
- **Found during:** Task 2 (replaceable/relay_list tests)
- **Issue:** `pick_winner` returns `Option<&'a Event>` borrowing from the input iterator's backing storage; passing an inline `[a, b].iter()` created a temporary freed before the borrow was used (E0716).
- **Fix:** Bound each candidate array to a `let candidates = [...]` local before `.iter()`.
- **Files modified:** tests/replaceable.rs, tests/relay_list.rs
- **Commit:** 595e5f8

---

**Total deviations:** 2 auto-fixed (both blocking). No architectural changes; no scope creep.

## Issues Encountered

- `gsd-tools` is not on PATH in this environment, so STATE.md / ROADMAP.md / REQUIREMENTS.md were updated by direct file edit rather than via the SDK query handlers. `cargo` is rustup-managed at `~/.cargo/bin` (prefixed `PATH` on each invocation, as in plan 02-01).

## Known Stubs

None in this plan's scope. (The relay module tree `src/relay/*` remains stubbed — owned by plan 02-03, not 02-02.)

## Threat Flags

None — all new surface maps to the plan's existing threat register (T-02-02 through T-02-08, all `mitigate`, all implemented).

## Verification

- `cargo test --lib` green.
- `cargo test --test verify_gate --test dedup --test replaceable --test relay_list --test follow_list_bounds` → 16 passed, 0 failed.
- Named subtests resolve: `--test verify_gate unsolicited` (2), `--test replaceable future_clamp` (2), `--test replaceable tie_break` (1), `--test follow_list_bounds malformed` (1), `--test follow_list_bounds cap` (2).
- `cargo clippy --tests` clean for the ingest module.
- No real `.unwrap()` call in `src/ingest/` (only doc-comment mentions).

## Next Phase Readiness

- The full validation gate is implemented and tested offline. Plan 02-04 can wire the relay fetch path (02-03) into `ingest_events`, then resolve each `ValidatedFollowList.followee_pubkeys` to surrogate ids and call `store::follows::apply_follow_list`.
- `ingest_events` now requires a `requested: &HashSet<PublicKey>` argument — 02-04 must pass the set of authors it actually fetched.
- No blockers.

---
*Phase: 02-relay-acquisition-validation*
*Completed: 2026-06-12*

## Self-Check: PASSED

All 5 created test files and 4 modified source files present on disk; all 4 task commits (740beef, 576d372, 595e5f8, 038d87e) present in git history. `cargo test --lib` and all 5 ingest test binaries pass (16/16).
