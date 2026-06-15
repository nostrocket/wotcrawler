---
phase: 5
slug: nip-65-outbox-routing-relay-health
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-06-15
---

# Phase 5 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | `cargo test` / `cargo nextest` + testcontainers Postgres |
| **Config file** | none — workspace `Cargo.toml` |
| **Quick run command** | `SQLX_OFFLINE=true cargo test --test <suite> -- --test-threads=2` |
| **Full suite command** | `SQLX_OFFLINE=true cargo test -- --test-threads=2` |
| **Estimated runtime** | ~tens of seconds |

---

## Sampling Rate

- **After every task commit:** Run the quick test command for the touched suite
- **After every plan wave:** Run the full suite command
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** ~60 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|-----------|-------------------|-------------|--------|
| TBD | TBD | 0 | RELAY-05 (kind:10002 r-tags extracted + persisted newest-wins) | integration | `cargo test --test relay_lists_store` | ❌ W0 | ⬜ pending |
| TBD | TBD | — | RELAY-05 (not_found pubkey recovered via its write relay when curated misses) | integration | `cargo test --test nip65_fallback` | ❌ W0 | ⬜ pending |
| TBD | TBD | — | RELAY-05 (nip65_recovered_total increments on recovery) | integration | `cargo test --test nip65_fallback` | ❌ W0 | ⬜ pending |
| TBD | TBD | — | RELAY-06 (health score moves with failures/timeouts/rate-limit/latency) | unit | `cargo test --test relay_health` | ❌ W0 | ⬜ pending |
| TBD | TBD | — | RELAY-06 (degraded relay skipped below threshold, then probed back) | unit/integration | `cargo test --test relay_health` | ❌ W0 | ⬜ pending |
| TBD | TBD | — | RELAY-06 (degraded relay gets fewer per-relay permits than healthy) | unit | `cargo test --test relay_health` | ❌ W0 | ⬜ pending |
| TBD | TBD | — | OPS-01 (new config fields validated fail-fast) | unit | `cargo test --test daemon_config` | ❌ W0 | ⬜ pending |

*Planner refines from the Validation Architecture section of 05-RESEARCH.md. Test seams: relay-URL-aware + error-injecting ScriptedGraph (model "curated not_found, write-relay found" and `Err(RelayError::FetchTimeout)`); kind:10002 events with real r-tags; deadlock-safe acquisition-order assertion. Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky.*

---

## Wave 0 Requirements

- [ ] `tests/relay_lists_store.rs` — migration 0004 + r-tag extraction + apply_relay_list newest-wins stubs (RELAY-05)
- [ ] `tests/nip65_fallback.rs` — fallback recovery + recovery-counter stubs (RELAY-05)
- [ ] `tests/relay_health.rs` — EWMA score movement + skip/probe + per-relay permit scaling stubs (RELAY-06)
- [ ] Test seams: relay-URL-aware + error-injecting ScriptedGraph extension; kind:10002 r-tag event builder; daemon_config new-field validation cases

*Wave 0 gaps identified in 05-RESEARCH.md Validation Architecture section.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Real-world coverage gain from NIP-65 fallback | RELAY-05 | Requires live relays | Run daemon against real relays; observe `nip65_recovered_total` rising and curated-coverage gap |
| Health routing under real relay degradation | RELAY-06 | Requires real degraded relays | Observe per-relay health gauge + traffic shift in Grafana during a live run |

*All in-scope automatable behaviors have automated verification; the two above are operational and align with the Phase 4 operator-UAT items.*

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 60s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
