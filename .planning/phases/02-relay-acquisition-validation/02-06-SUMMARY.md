---
phase: 02-relay-acquisition-validation
plan: 06
subsystem: ingest
tags: [security, ingest, dedup, CR-01, T-02-14, INGEST-02]
requires:
  - "src/ingest/mod.rs ingest_events (existing orchestrator, 02-02)"
  - "src/ingest/verify.rs accept (existing verify gate, 02-02)"
provides:
  - "Dedup-after-verify ordering: only verified event ids enter the cross-relay seen-set"
  - "tests/common id_squat_forgery fixture (forged event claiming a target's id)"
affects:
  - "src/ingest/mod.rs"
  - "tests/id_squat.rs"
  - "tests/common/mod.rs"
tech-stack:
  added: []
  patterns:
    - "Security-critical ordering: verification gates seen-set insertion (dedup must follow verify)"
key-files:
  created:
    - "tests/id_squat.rs"
  modified:
    - "src/ingest/mod.rs"
    - "tests/common/mod.rs"
decisions:
  - "Dedup follows verification in ingest_events so a forged id-squat copy cannot consume a genuine id in the seen-set (CR-01/T-02-14)."
metrics:
  duration_min: 1
  completed: 2026-06-13
  tasks: 1
  files: 3
---

# Phase 02 Plan 06: Dedup-After-Verify (CR-01) Summary

Closed CR-01 (BLOCKER): reordered the `ingest_events` loop so `verify::accept`
runs before `seen.insert(event.id)`, defeating the id-squat censorship attack
(T-02-14) — only verified ids now occupy the cross-relay seen-set.

## What Was Built

- **Reordered `ingest_events` loop (`src/ingest/mod.rs`)**: `verify::accept(&event, want_kind, requested)` is now called FIRST; `if !seen.insert(event.id)` runs only on the accepted branch. A security-critical-ordering comment cites CR-01/T-02-14, and the pipeline doc comment was updated to reflect verify -> dedup ordering. Genuine duplicate suppression (INGEST-02) is preserved — verified duplicates still collapse to one `ValidatedFollowList`.
- **`tests/id_squat.rs`** (`id_squat_does_not_suppress_genuine_event`): builds a genuine valid event G and a forged copy F that claims G's id but fails `verify()`, orders the batch `[F, G]`, and asserts exactly one `ValidatedFollowList` for G's author with G's id and followees survives.
- **`tests/common/mod.rs` `id_squat_forgery` fixture**: clones a tampered (content-mutated-after-signing) event and overwrites its stored `id` to a target event's id, so it still claims the genuine id while `verify()` rejects it on id recomputation.

## TDD Cycle

- RED (`62190dc`): test + fixture added; failed with old dedup-before-verify ordering (result len 0 — genuine event suppressed by the forgery).
- GREEN (`bc7bf53`): guards swapped; `id_squat` and `dedup` pass.
- REFACTOR: none needed (minimal two-guard reorder).

The test is load-bearing: it fails with the pre-fix ordering (proven by the RED commit) and passes after the reorder.

## Verification

- `cargo test --test id_squat --test dedup` exits 0 (1 + 1 passing).
- `cargo test --test verify_gate --test replaceable --test relay_list --test follow_list_bounds` all pass (no regression in the verified ingest gate).
- Full offline `cargo test --tests` suite green (all binaries pass).
- In `src/ingest/mod.rs`, `verify::accept` is invoked at a lower line number than `seen.insert(event.id)` within the loop.

## Must-Haves Satisfied

- An event id enters the seen-set only after `verify::accept` succeeds — forged id-squat copy cannot consume the id. ✓
- A hostile relay's forged copy carrying a genuine id, ordered first, no longer suppresses the genuine follow list. ✓ (id_squat test)
- Genuine duplicate ids from honest relays are still processed at most once. ✓ (dedup test preserved)

## Deviations from Plan

None - plan executed exactly as written.

## Threat Mitigation

T-02-14 (id-squat censorship, `mitigate`) closed: dedup-after-verify ensures only verified ids occupy the seen-set, so a forged copy claiming a genuine id cannot suppress the honest event.

## Self-Check: PASSED

All created/modified files present; RED (`62190dc`) and GREEN (`bc7bf53`) commits found.
