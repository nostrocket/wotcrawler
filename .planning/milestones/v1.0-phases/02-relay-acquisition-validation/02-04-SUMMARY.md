---
phase: 02-relay-acquisition-validation
plan: 04
subsystem: acquire-pipeline
tags: [nostr-sdk, rust, pagination, adversarial-input, pipeline-seam, validation]

# Dependency graph
requires:
  - phase: 02-relay-acquisition-validation
    plan: 02
    provides: "ingest::ingest_events orchestrator + ValidatedFollowList contract + ingest::timestamp_to_datetime (the verify/dedup/replaceable/bounds gate this seam routes through)"
  - phase: 02-relay-acquisition-validation
    plan: 03
    provides: "relay::fetch::fetch_complete (production raw-stream source) + relay::fetch::paginate_chunk (RELAY-03 page-back loop) + tests/mock_relay scripted-window fetch fn + relay::connect_curated Client"
provides:
  - "relay::acquire_validated_lists: the fetch -> ingest seam — composes a raw paged event stream through ingest_events to emit Vec<ValidatedFollowList>; holds NO validation logic of its own"
  - "relay::acquire_validated_lists_client: production entry point driving a connected Client via fetch_complete through the seam"
  - "tests/acquire_pipeline.rs: end-to-end proof — mock relay (two paged windows, adversarially polluted) -> wired pipeline -> asserted deduped/newest-wins/self-drop-filtered ValidatedFollowList"
affects: []

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Composition-only pipeline seam: the acquire fn owns zero validation logic (grep-gated: no verify()/then_with/public_keys in src/relay/mod.rs); all gating stays in src/ingest. T-02-14 enforced by routing EVERY raw event through ingest_events (no direct fetch->ValidatedFollowList path)."
    - "Injected-fetch-fn seam (F: FnOnce() -> Fut) so the E2E test drives the 02-03 scripted mock relay through the real paginate_chunk page-back loop; the production wrapper closes over fetch_complete + a live Client. Same seam, swapped leaf source."
    - "Full-union-before-resolution: the seam consumes the complete paged output before a single ingest resolution pass, so a relay cannot split a pubkey's events across window boundaries to defeat newest-wins (T-02-15)."

key-files:
  created:
    - tests/acquire_pipeline.rs
  modified:
    - src/relay/mod.rs

key-decisions:
  - "acquire_validated_lists is generic over an injected FnOnce fetch source (not hardcoded to fetch_complete) so the E2E test can drive the 02-03 scripted mock relay through the real paginate_chunk page-back loop offline; acquire_validated_lists_client is the thin production wrapper that closes over fetch_complete + a live Client."
  - "src/ingest/mod.rs left UNTOUCHED — 02-02 already exported ingest_events as pub, so no visibility tweak was needed (the plan permitted a pub use only if it had been left private)."
  - "The seam's requested-author set for the production path is exactly the `authors` slice handed to fetch_complete (the set actually solicited), satisfying the INGEST-01 unsolicited-author drop without a separate parameter."

patterns-established:
  - "Composition-only seam with grep-enforced absence of validation logic"
  - "Injected-fetch-fn seam reused from the transport layer for offline E2E wiring proof"
  - "Consume full paged union before a single resolution pass"

requirements-completed: [RELAY-03, INGEST-01, INGEST-02, INGEST-03, INGEST-04, INGEST-05]

# Metrics
duration: 4min
completed: 2026-06-12
---

# Phase 2 Plan 04: Acquire Pipeline Summary

**The seam that connects Phase 2's two halves: `acquire_validated_lists` routes the raw, deduplicated, still-unverified event stream from the RELAY-03 pagination loop through the ingest gate (`ingest_events`) so that — and only then — a `ValidatedFollowList` emerges. An end-to-end test drives an adversarially-polluted, two-window mock-relay stream through the wired pipeline and proves exactly the correct deduped / newest-wins / self-drop-filtered follow list comes out.**

## Performance

- **Duration:** ~4 min
- **Started:** 2026-06-12
- **Completed:** 2026-06-12
- **Tasks:** 1
- **Files modified:** 2 (1 created, 1 modified)

## Accomplishments

- **The fetch -> ingest seam (the phase goal made observable).** `relay::acquire_validated_lists` consumes the raw paged event stream and hands the FULL union to `ingest::ingest_events`, returning `Vec<ValidatedFollowList>`. It holds no validation logic of its own — verify (INGEST-01), cross-relay dedup (INGEST-02), replaceable resolution (INGEST-03/05), and follow-list bounds (INGEST-04) all stay in `src/ingest`. This closes the RESEARCH anti-pattern "building the two halves but never connecting them" and the checker's `key_links_planned` warning.
- **Production entry point.** `relay::acquire_validated_lists_client` closes over `fetch::fetch_complete` (the RELAY-03 author-chunked until-window loop) driving a connected `Client`, then composes it through the seam. The requested-author set is exactly the solicited `authors`.
- **End-to-end proof (`tests/acquire_pipeline.rs`).** Splits the follower's older + newer kind-3 across a CAPPED first window and a SHORT second window served through the 02-03 scripted mock relay under the real `paginate_chunk` page-back loop, and injects a forged event, an unsolicited wrong-author event, and a future-dated event into the stream. Asserts: (a) exactly one `ValidatedFollowList` emerges; (b) `followee_pubkeys == {a,b,c}` (duplicate p-tag collapsed, self-follow dropped, all three adversarial events excluded); (c) `event_id`/`created_at` are the NEWER event's from the SECOND window — proving the resolver ran across BOTH paged windows; and that the relay saw two REQs with the second paging back to an older `until`.

## Task Commits

1. **Task 1: wire fetch -> ingest seam + E2E proof (RELAY-03 + INGEST-01..05)** — `e185789` (feat)

**Plan metadata:** _(final docs commit)_

## Files Created/Modified

- `src/relay/mod.rs` — added `acquire_validated_lists` (the composition-only seam, generic over an injected `FnOnce` fetch source) and `acquire_validated_lists_client` (the production wrapper over `fetch_complete` + a live `Client`); added the `use` of `ingest::{self, ValidatedFollowList}` and the nostr types the signatures need.
- `tests/acquire_pipeline.rs` — the end-to-end seam proof (created).

## Decisions Made

- **Generic injected fetch source.** `acquire_validated_lists` takes `F: FnOnce() -> Fut` for the raw-stream source rather than hardcoding `fetch_complete`. This lets the E2E test drive the 02-03 scripted mock relay through the real `paginate_chunk` loop offline (no live websocket), while `acquire_validated_lists_client` is the thin production wrapper that closes over `fetch_complete` and a connected `Client`. The page-back logic the test exercises is byte-for-byte production's.
- **`src/ingest/mod.rs` untouched.** Plan 02-02 already exported `ingest_events` as `pub`, so the plan's conditional "add a `pub use`/visibility tweak if the orchestrator was left private" did not apply. The seam compiles against the existing contract with no change to `ValidatedFollowList`, `ingest_events`, or `fetch_complete`.
- **Solicited-author set = the fetched `authors`.** The production wrapper builds the ingest gate's `requested` set directly from the `authors` slice it hands to `fetch_complete`, so the unsolicited-author drop (INGEST-01 / Pitfall 4) is wired with no extra parameter.

## Deviations from Plan

None — the plan executed exactly as written. No bugs, no missing-functionality additions, no blocking issues, no architectural changes. `src/ingest/mod.rs` was listed in `files_modified` but required no change (the plan explicitly permitted leaving it untouched when 02-02 already exported the orchestrator); this is a no-op against the plan's allowance, not a deviation.

## Threat Model Coverage

| Threat ID | Disposition | Where mitigated |
|-----------|-------------|-----------------|
| T-02-14 (validation bypass at the seam) | mitigate | The seam routes EVERY raw event through `ingest::ingest_events` — there is no direct fetch -> `ValidatedFollowList` path. Grep gate confirms no `verify()`/`then_with`/`public_keys` in `src/relay/mod.rs`. The E2E test injects forged/unsolicited/future-dated events and asserts all are excluded from the emerged list. (Task 1) |
| T-02-15 (completeness loss across the seam) | mitigate | The seam consumes the FULL paged `fetch` output before a single `ingest_events` resolution pass; the E2E test splits the follower's events across a capped first window and a short second window and asserts the newest (second-window) event wins. (Task 1) |
| T-02-SC (crates.io installs) | accept | No new dependencies added (composition only); package-legitimacy posture unchanged from 02-01. |

No new security surface beyond the plan's threat register.

## Issues Encountered

- `cargo` is rustup-managed at `~/.cargo/bin` and not on the non-interactive shell PATH; each invocation is prefixed `PATH="$HOME/.cargo/bin:$PATH"` (consistent with plans 02-01/02-02/02-03). `gsd-tools` is not on PATH, so STATE.md / ROADMAP.md / REQUIREMENTS.md are updated by direct file edit rather than via the SDK query handlers.
- One transient clippy warning (`redundant_pattern_matching` on an over-defensive page-back assertion fallback) was tightened to the single correct `matches!` arm before commit — caught pre-commit, not a post-merge fix.

## Known Stubs

None. The seam is fully implemented and exercised end-to-end; no placeholder or empty-return paths remain in the acquire path.

## Threat Flags

None — all new surface (the two seam functions) maps to the plan's existing threat register (T-02-14, T-02-15, both `mitigate`, both asserted by the E2E test).

## Verification

- `cargo test --test acquire_pipeline` → 1 passed, 0 failed (mock relay -> wired pipeline -> asserted ValidatedFollowList).
- `cargo test --lib` + all offline relay/ingest suites (`verify_gate`, `dedup`, `replaceable`, `relay_list`, `follow_list_bounds`, `reconnect_policy`, `rate_limit_backoff`, `nip11_limits`, `pagination`) → all green, no regression.
- `cargo test --test edge_diff` (Docker/Postgres) → 4 passed (Phase 1 store tests, no regression).
- Acceptance greps: `grep -c 'acquire_validated_lists' src/relay/mod.rs` = 5 (>=1); fetch-path ref = 3 (>=1); ingest ref (`ingest_events|ValidatedFollowList`) = 8 (>=1); validation-logic gate (`verify()|then_with|public_keys`) = 0 (==0); `grep -c 'ValidatedFollowList' tests/acquire_pipeline.rs` = 2 (>=1).
- `cargo clippy --test acquire_pipeline --lib` clean.

## Next Phase Readiness

- Phase 2 is complete: a `ValidatedFollowList` now emerges end-to-end from the relay acquisition path, satisfying the phase goal that only correct, deduplicated, newest-wins follow lists emerge.
- Phase 3 (orchestration/persistence) can drive `relay::connect_curated` -> `relay::acquire_validated_lists_client` per author batch, resolve each `ValidatedFollowList.followee_pubkeys` to surrogate ids via `store::pubkeys::upsert_pubkey`, and call `store::follows::apply_follow_list` (the `event_id`/`created_at`/`followee_pubkeys` fields map directly onto its arguments).
- No blockers.

---
*Phase: 02-relay-acquisition-validation*
*Completed: 2026-06-12*

## Self-Check: PASSED

Both files (`tests/acquire_pipeline.rs`, `src/relay/mod.rs`) and the SUMMARY present on disk; the Task 1 commit `e185789` present in git history. `cargo test --test acquire_pipeline` (1/1) and the full offline relay/ingest suites + the Postgres `edge_diff` suite (4/4) all pass.
