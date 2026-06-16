---
phase: 02-relay-acquisition-validation
plan: 08
subsystem: relay
tags: [rate-limiting, backoff, concurrency, politeness, RELAY-01, RELAY-04]
requires:
  - "src/relay/rate_limit.rs RateLimiterRegistry (from 02-03)"
provides:
  - "Shared Arc<DirectLimiter> per relay: concurrent acquire() calls obey one GCRA quota (CR-05)"
  - "backoff_delay_unjittered saturates at cap for failures >= 64, no zero-delay window (WR-01)"
affects:
  - "src/relay/rate_limit.rs"
  - "plan 02-09 (production wiring of acquire/record_notice depends on this correctness fix)"
tech-stack:
  added: []
  patterns:
    - "Arc-wrapped shared interior state awaited across an await point without holding the map lock"
    - "Early saturation guard to avoid integer-shift truncation in exponential backoff"
key-files:
  created: []
  modified:
    - "src/relay/rate_limit.rs"
    - "tests/rate_limit_backoff.rs"
decisions:
  - "Saturate backoff at failures >= 64 (not 128): any 2^64-ns factor already dwarfs any reasonable cap, and 64 closes the entire u128 checked_shl high-bit-truncation window (119..=127) regardless of base bit position."
  - "Store Arc<DirectLimiter> rather than making acquire() lock-free another way: governor limiters are not Clone, so the Arc clone-and-await is the minimal correct fix that keeps GCRA state shared and never discarded."
metrics:
  duration_min: 3
  completed: "2026-06-13"
  tasks: 2
  files: 2
---

# Phase 02 Plan 08: Rate Limiter Concurrency & Backoff Saturation Summary

Per-relay throttling is now correct under concurrency (one shared `Arc<DirectLimiter>` per relay, no quota multiplication) and the backoff schedule saturates at cap with no zero-delay window — closing CR-05 (BLOCKER) and WR-01 (WARNING) in the rate limiter, the "politely" half of the phase goal (RELAY-04).

## What Was Built

### Task 1 — Shared limiter per relay (CR-05, T-02-10)
`RateLimiterRegistry::acquire` previously `remove`d the relay's limiter from the map before awaiting `until_ready()`, so a concurrent caller found the slot vacant and minted a fresh full-burst limiter — multiplying the per-relay quota by concurrency (the politeness void). On return `or_insert` kept the wrong (stateless) entry, discarding accrued GCRA state.

Fix: the map now stores `Arc<DirectLimiter>`. `acquire()` locks the map, get-or-inserts the relay's shared `Arc` via `or_insert_with`, clones the `Arc` (a cheap refcount bump — GCRA state is shared, not copied), drops the lock, then awaits `until_ready()` on the clone. The limiter is never removed, so concurrent callers for the same relay drive the same GCRA state.

- `limiters: Mutex<HashMap<String, Arc<DirectLimiter>>>` (was `HashMap<String, DirectLimiter>`)
- Added `use std::sync::Arc`
- Removed the take-out/put-back logic and its misleading comments

### Task 2 — Backoff saturation at cap (WR-01, T-02-20)
`backoff_delay_unjittered` used `u128::checked_shl` guarded only by `failures >= 128`. At base=1s the factor's set bits start near bit 30; shift counts in 119..=127 pushed them past bit 127, so `checked_shl` returned `Some` with the high bits truncated to 0 — a ZERO delay after 119+ consecutive failures (a zero-delay retry storm, the opposite of saturation).

Fix: the guard is now `if failures >= 64 { return cap; }`. Any 2^64-ns factor already dwarfs any reasonable cap, so clamping at 64 is correct and closes the entire truncation window without depending on the base's bit position. The `failures < 64` capped-exponential path is unchanged. The comment now documents the bit-truncation reasoning.

## Tests

`tests/rate_limit_backoff.rs` (TDD, both tasks RED→GREEN):

- `concurrent_acquires_share_one_limiter` (`#[tokio::test(flavor = "multi_thread")]`): 4 parallel `acquire()` tasks on one relay at 1 req/sec must take >= ~(N-1) replenish intervals in aggregate, proving a single shared limiter. RED took 1.21s (independent bursts); GREEN throttles as required.
- `backoff_saturates_at_cap_for_high_failure_counts`: explicit `== cap` at failures=120, a sweep over `64..=127` asserting non-zero and `<= cap` (and `== cap`), plus low-count exponential growth (`d0==base`, `d1==2*base`, `d2==4*base`). RED returned `0ns` at 120; GREEN returns cap.

All 7 tests in the file pass; full crate test suite green (no regressions).

## Verification

- `cargo test --test rate_limit_backoff` — 7 passed, 0 failed.
- `cargo test` (full suite) — all green, no regressions.
- `grep -n "Arc<DirectLimiter>" src/relay/rate_limit.rs` — matches (line 135).
- `grep -n "failures >= 64" src/relay/rate_limit.rs` — matches (line 115); `failures >= 128` — no matches.
- `grep -n "remove(relay_url)" src/relay/rate_limit.rs` — only line 233 inside `reset` (failures map; not in `acquire`, as required).

## Threat Mitigations Applied

| Threat ID | Disposition | How |
|-----------|-------------|-----|
| T-02-10 (DoS politeness void / IP ban) | mitigate | Shared `Arc<DirectLimiter>` per relay — concurrent acquires obey one GCRA quota; no quota multiplication. |
| T-02-20 (DoS retry storm) | mitigate | Early saturation at `failures >= 64` — backoff monotonic to cap, no zero-delay window. |

## Deviations from Plan

None — plan executed exactly as written.

## Notes / Follow-ups

- Production-path WIRING of `acquire()`/`record_notice()`/`LimitCache` (CR-05's WR-03 component) is plan 02-09, which depends on this plan landing first. This plan fixes the limiter's internal correctness only.
- No new dependencies; `Arc` is from std.

## Commits

- `6a13f42` test(02-08): add failing concurrency test for shared per-relay limiter (RED)
- `683ce57` feat(02-08): share one Arc<DirectLimiter> per relay across concurrent acquires (GREEN)
- `5e4fd61` test(02-08): add failing backoff saturation test for high failure counts (RED)
- `cd8a766` feat(02-08): saturate backoff at cap for failures >= 64 (GREEN)

## Self-Check: PASSED

All modified files exist on disk; all four task commits present in git history.
