---
phase: 2
slug: relay-acquisition-validation
status: draft
nyquist_compliant: false
wave_0_complete: false
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
| TBD | TBD | TBD | INGEST-01 | TBD | Forged/invalid-sig event rejected and counted | unit | `cargo test verify_gate` | ❌ W0 | ⬜ pending |
| TBD | TBD | TBD | INGEST-01 | TBD | Event of wrong kind/author dropped | unit | `cargo test verify_gate::unsolicited` | ❌ W0 | ⬜ pending |
| TBD | TBD | TBD | INGEST-02 | TBD | Same id from N relays processed once | unit/integration | `cargo test dedup` | ❌ W0 | ⬜ pending |
| TBD | TBD | TBD | INGEST-03 | TBD | Future-dated > clamp rejected | unit | `cargo test replaceable::future_clamp` | ❌ W0 | ⬜ pending |
| TBD | TBD | TBD | INGEST-03 | TBD | Newest-wins; same-ts → lowest id | unit | `cargo test replaceable::tie_break` | ❌ W0 | ⬜ pending |
| TBD | TBD | TBD | INGEST-04 | TBD | Malformed p-tags skipped | unit | `cargo test follow_list_bounds::malformed` | ❌ W0 | ⬜ pending |
| TBD | TBD | TBD | INGEST-04 | TBD | Oversized list bounded without panic | unit | `cargo test follow_list_bounds::cap` | ❌ W0 | ⬜ pending |
| TBD | TBD | TBD | INGEST-05 | TBD | kind:10002 newest-wins resolution | unit | `cargo test relay_list` | ❌ W0 | ⬜ pending |
| TBD | TBD | TBD | RELAY-03 | TBD | Capped response triggers another page; EOSE not trusted | integration | `cargo test pagination` | ❌ W0 | ⬜ pending |
| TBD | TBD | TBD | RELAY-02 | TBD | NIP-11 limits parsed + capped into filter | unit/integration | `cargo test nip11_limits` | ❌ W0 | ⬜ pending |
| TBD | TBD | TBD | RELAY-04 | TBD | Rate-limited notice triggers backoff | unit | `cargo test rate_limit_backoff` | ❌ W0 | ⬜ pending |
| TBD | TBD | TBD | RELAY-01 | TBD | Reconnect policy applied (+ jitter if app-layer added) | unit | `cargo test reconnect_policy` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

*Task IDs to be filled by the planner — the requirement→test mapping above is the contract.*

---

## Wave 0 Requirements

- [ ] Test fixtures: helpers to build signed `Event`s with known keys, plus a forged/invalid-sig event, plus same-ts variants for tie-break
- [ ] A mock or in-process relay (or recorded responses) returning capped result sets + EOSE for the pagination test (RELAY-03) — the single hardest fixture; pin down in Wave 0
- [ ] `tests/` files per the Per-Task Verification Map
- [ ] First `cargo build` confirms nostr-sdk 0.44.1 compiles on toolchain 1.94

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Live-relay politeness (real relay does not ban/throttle the crawler) | RELAY-04 | Requires sustained connection to a public relay; not CI-suitable | Run crawler against one curated relay for ~10 min; observe no `rate-limited` notices in logs and no disconnects |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 120s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
