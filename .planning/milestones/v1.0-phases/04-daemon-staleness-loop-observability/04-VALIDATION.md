---
phase: 4
slug: daemon-staleness-loop-observability
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-06-15
---

# Phase 4 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | `cargo test` / `cargo nextest` (Rust built-in) + testcontainers Postgres |
| **Config file** | none — workspace `Cargo.toml` |
| **Quick run command** | `SQLX_OFFLINE=true cargo test --test <suite> -- --test-threads=2` |
| **Full suite command** | `SQLX_OFFLINE=true cargo test -- --test-threads=2` |
| **Estimated runtime** | ~tens of seconds (DB + in-process axum/loop tests) |

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
| TBD | TBD | 0 | OPS-01 (config load+validate+env override precedence) | unit | `cargo test --test config` | ❌ W0 | ⬜ pending |
| TBD | TBD | — | FRESH-02 (staleness scanner re-enqueues only stale rows, resets attempts) | integration | `cargo test --test staleness` | ❌ W0 | ⬜ pending |
| TBD | TBD | — | FRESH-03 (refresh records churn: fetch_count/change_count/last_changed_at) | integration | `cargo test --test staleness` | ❌ W0 | ⬜ pending |
| TBD | TBD | — | OPS-02 (graceful shutdown drains in-flight, no orphaned leases) | integration | `cargo test --test daemon_loop` | ❌ W0 | ⬜ pending |
| TBD | TBD | — | OBS-01 (/metrics exposes required series) | integration | `cargo test --test observability` | ❌ W0 | ⬜ pending |
| TBD | TBD | — | OBS-02 (tracing levels honored / EnvFilter) | unit | `cargo test --test observability` | ❌ W0 | ⬜ pending |
| TBD | TBD | — | OBS-03 (/health/live + /health/ready semantics) | integration | `cargo test --test observability` | ❌ W0 | ⬜ pending |
| TBD | TBD | — | OBS-04 (periodic progress summary emitted) | integration | `cargo test --test daemon_loop` | ❌ W0 | ⬜ pending |
| TBD | TBD | — | OBS-05 (Grafana dashboard JSON committed + valid) | unit | `cargo test --test observability` (JSON parse) | ❌ W0 | ⬜ pending |

*Planner refines this map from the Validation Architecture section of 04-RESEARCH.md. Test seams: inject a `CancellationToken` + mock `fetch_union` + testcontainers DB into the daemon loop; hit axum handlers in-process via `tower::ServiceExt::oneshot`; assert tracing via a capturing layer. Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky.*

---

## Wave 0 Requirements

- [ ] `tests/config.rs` — config load + env-override precedence + fail-fast validation stubs (OPS-01)
- [ ] `tests/staleness.rs` — staleness scanner + FRESH-03 churn stubs (FRESH-02, FRESH-03)
- [ ] `tests/daemon_loop.rs` — cancellation-aware loop + graceful drain + progress summary stubs (OPS-02, OBS-04)
- [ ] `tests/observability.rs` — /metrics, /health/live, /health/ready, tracing, dashboard-JSON-valid stubs (OBS-01/02/03/05)
- [ ] Test seam helpers: injectable `CancellationToken`, mock `fetch_union`, in-process axum router

*Wave 0 gaps identified in 04-RESEARCH.md Validation Architecture section.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Grafana dashboard renders against a live Prometheus | OBS-05 | Requires running Grafana+Prometheus | Import `ops/*.json` into Grafana pointed at the daemon's /metrics; confirm panels populate |
| Multi-day unattended crawl resource profile | (STATE concern) | Requires full-scale run | Run daemon against full relay set; watch progress summaries + metrics over days |

*All in-scope automated Phase 4 behaviors have automated verification; the two above are inherently operational.*

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 60s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
