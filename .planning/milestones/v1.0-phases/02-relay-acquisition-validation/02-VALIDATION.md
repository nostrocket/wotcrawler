---
phase: 2
slug: relay-acquisition-validation
status: planned
nyquist_compliant: true
wave_0_complete: true
created: 2026-06-12
---

# Phase 2 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | Rust built-in harness (`#[test]` / `#[tokio::test]`); follows Phase 1 conventions |
| **Config file** | none required; `SQLX_OFFLINE=true` in CI (only if store tests run) |
| **Quick run command** | `cargo test --lib` |
| **Full suite command** | `cargo test` |
| **Estimated runtime** | ~30 seconds (lib); ~120 seconds (full, incl. mock-relay integration) |

---

## Sampling Rate

- **After every task commit:** Run `cargo test --lib`
- **After every plan wave:** Run `cargo test`
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** 120 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 02-02-T1 | 02-02 | 2 | INGEST-01 | T-02-02 | Forged/invalid-sig event rejected and counted | unit | `cargo test --test verify_gate` | ❌ W0→02-01/02-02 | ⬜ pending |
| 02-02-T1 | 02-02 | 2 | INGEST-01 | T-02-03 | Event of wrong kind/author dropped | unit | `cargo test verify_gate::unsolicited` | ❌ W0→02-01/02-02 | ⬜ pending |
| 02-02-T1 | 02-02 | 2 | INGEST-02 | T-02-08 | Same id from N relays processed once | unit/integration | `cargo test --test dedup` | ❌ W0→02-01/02-02 | ⬜ pending |
| 02-02-T2 | 02-02 | 2 | INGEST-03 | T-02-04 | Future-dated > clamp rejected | unit | `cargo test replaceable::future_clamp` | ❌ W0→02-01/02-02 | ⬜ pending |
| 02-02-T2 | 02-02 | 2 | INGEST-03 | T-02-05 | Newest-wins; same-ts → lowest id | unit | `cargo test replaceable::tie_break` | ❌ W0→02-01/02-02 | ⬜ pending |
| 02-02-T3 | 02-02 | 2 | INGEST-04 | T-02-07 | Malformed p-tags skipped | unit | `cargo test follow_list_bounds::malformed` | ❌ W0→02-01/02-02 | ⬜ pending |
| 02-02-T3 | 02-02 | 2 | INGEST-04 | T-02-06 | Oversized list bounded without panic | unit | `cargo test follow_list_bounds::cap` | ❌ W0→02-01/02-02 | ⬜ pending |
| 02-02-T2 | 02-02 | 2 | INGEST-05 | T-02-04 | kind:10002 newest-wins resolution | unit | `cargo test --test relay_list` | ❌ W0→02-01/02-02 | ⬜ pending |
| 02-03-T3 | 02-03 | 2 | RELAY-03 | T-02-11 | Capped response triggers another page; EOSE not trusted | integration | `cargo test --test pagination` | ❌ W0→02-01/02-03 | ⬜ pending |
| 02-03-T2 | 02-03 | 2 | RELAY-02 | T-02-13 | NIP-11 limits parsed + capped into filter | unit/integration | `cargo test --test nip11_limits` | ❌ W0→02-01/02-03 | ⬜ pending |
| 02-03-T1 | 02-03 | 2 | RELAY-04 | T-02-10 | Rate-limited notice triggers backoff | unit | `cargo test --test rate_limit_backoff` | ❌ W0→02-01/02-03 | ⬜ pending |
| 02-03-T1 | 02-03 | 2 | RELAY-01 | T-02-09 | Reconnect policy applied (+ jitter if app-layer added) | unit | `cargo test --test reconnect_policy` | ❌ W0→02-01/02-03 | ⬜ pending |
| 02-04-T1 | 02-04 | 3 | RELAY-03, INGEST-01..05 | T-02-14 | fetch→ingest pipeline emits ValidatedFollowList end-to-end (only validated lists emerge) | integration | `cargo test --test acquire_pipeline` | ❌ W0→02-01..03 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

*Wave 0 (fixtures, deps, module skeleton, mock-relay harness) is delivered by plan 02-01 (event fixtures) and the first task of 02-03 (mock relay). Pure-logic ingest tests need no Postgres and no network. The Wave 3 pipeline test (02-04) composes the Wave 2 halves and reuses their existing fixtures/mock-relay harness — it adds no new Wave 0 dependency.*

---

## Wave 0 Requirements

- [x] Test fixtures: helpers to build signed `Event`s with known keys, plus a forged/invalid-sig event, plus same-ts variants for tie-break → **plan 02-01 Task 2**
- [x] A mock or in-process relay (or recorded responses) returning capped result sets + EOSE for the pagination test (RELAY-03) → **plan 02-03 Task 3 (tests/mock_relay/mod.rs)**
- [x] `tests/` files per the Per-Task Verification Map → **plans 02-02, 02-03, and 02-04**
- [x] First `cargo build` confirms nostr-sdk 0.44.1 compiles on toolchain 1.94 → **plan 02-01 Task 1**

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Live-relay politeness (real relay does not ban/throttle the crawler) | RELAY-04 | Requires sustained connection to a public relay; not CI-suitable | Run crawler against one curated relay for ~10 min; observe no `rate-limited` notices in logs and no disconnects |

---

## Validation Sign-Off

- [x] All tasks have `<automated>` verify or Wave 0 dependencies
- [x] Sampling continuity: no 3 consecutive tasks without automated verify
- [x] Wave 0 covers all MISSING references
- [x] No watch-mode flags
- [x] Feedback latency < 120s
- [x] `nyquist_compliant: true` set in frontmatter

**Approval:** planner-approved 2026-06-12 (revised: wave_0_complete corrected to true; added 02-04 pipeline-wiring task)
