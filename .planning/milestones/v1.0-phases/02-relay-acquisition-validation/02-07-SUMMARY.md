---
phase: 02-relay-acquisition-validation
plan: 07
subsystem: relay
tags: [nip11, relay, security, dos, pagination]
requires:
  - "src/relay/nip11.rs fetch_limits (RELAY-02 NIP-11 acquisition)"
  - "RelayError::Nip11Fetch error variant"
provides:
  - "Deadline-bounded NIP-11 fetch (request + connect timeout) on a shared client"
  - "MAX_NIP11_BYTES body bound rejecting oversized payloads before parse"
  - "MAX_ADVERTISED_LIMIT upper clamp on advertised max_limit"
  - "limits_from_bytes testable seam for the body bound"
affects:
  - "Pagination planner consumes the now-trustworthy clamped max_limit"
tech-stack:
  added: []
  patterns:
    - "Process-shared LazyLock<reqwest::Client> built once with .timeout + .connect_timeout (Pitfall 9)"
    - "Stream-and-bail body size enforcement via resp.chunk() with an accumulated cap"
    - "Upper-clamp adversarial relay-advertised limits to an internal ceiling"
key-files:
  created: []
  modified:
    - "src/relay/nip11.rs"
    - "tests/nip11_limits.rs"
decisions:
  - "MAX_NIP11_BYTES = 64 KiB: generous for a small JSON information document"
  - "MAX_ADVERTISED_LIMIT = 5000: 10x DEFAULT_MAX_LIMIT, above honest relay caps, low enough to defeat the Pitfall-1 EOSE-completeness trap"
  - "Upper clamp applies to max_limit only; max_subscriptions/max_filters keep default-on-non-positive behavior (they shape the crawler's own requests, not relay completeness)"
  - "fetch_limits streams chunks and bails before fully buffering an oversized body, not resp.bytes() (true memory bound, T-02-19)"
metrics:
  duration_min: 3
  tasks: 2
  files: 2
  completed: "2026-06-13"
---

# Phase 02 Plan 07: NIP-11 Timeout, Body Bound & Advertised-Limit Clamp Summary

Made RELAY-02 NIP-11 acquisition deadline-bounded, memory-bounded, and immune to an absurd advertised `max_limit` — closing CR-06 (BLOCKER) and WR-02 (WARNING).

## What Shipped

- **CR-06 (timeout):** `fetch_limits` now sends via a process-shared `static NIP11_CLIENT: LazyLock<reqwest::Client>` built once with `.timeout(10s)` and `.connect_timeout(5s)`, replacing the per-call `reqwest::Client::new()` that carried no deadline. A relay that accepts the connection and never responds (or never finishes the TLS handshake) can no longer hang the crawler (T-02-18, Pitfall 9).
- **CR-06 (memory):** `MAX_NIP11_BYTES = 64 KiB` bounds the body. `fetch_limits` streams the response chunk-by-chunk via `resp.chunk()` and returns `RelayError::Nip11Fetch` as soon as the accumulated length would exceed the bound — so a hostile relay streaming an arbitrarily large payload is never fully buffered into memory (T-02-19). The pure `limits_from_bytes(relay_url, &[u8])` seam enforces the same bound before any UTF-8 decode / JSON parse, making the rejection unit-testable offline.
- **WR-02 (clamp):** `MAX_ADVERTISED_LIMIT = 5000` upper-bounds an advertised `max_limit`. New `clamp_max_limit` caps the value so a relay advertising `2_000_000_000` produces a per-window cap of 5000 — small enough that the count-vs-cap pagination loop never treats one EOSE window as complete (T-02-13, Pitfall 1). `max_subscriptions` / `max_filters` keep their existing default-on-non-positive behavior.

## Tasks

| Task | Name | RED | GREEN | Files |
| ---- | ---- | --- | ----- | ----- |
| 1 | Timeout + connect-timeout + body-size bound + shared client (CR-06) | 28fca89 | 6a6e707 | src/relay/nip11.rs, tests/nip11_limits.rs |
| 2 | Upper-clamp advertised max_limit (WR-02) | dc10877 | d53b718 | src/relay/nip11.rs, tests/nip11_limits.rs |

## Verification

- `cargo test --test nip11_limits` — 9 passed, 0 failed (4 new: oversized-body-rejected, bounded-body-parses, advertised-upper-clamp, below-ceiling-preserved).
- `grep -n "reqwest::Client::new()" src/relay/nip11.rs` — no match.
- `grep -n "connect_timeout" src/relay/nip11.rs` — matches (2).
- `MAX_NIP11_BYTES` and `MAX_ADVERTISED_LIMIT` both defined (`pub const`) and used.
- `cargo build` succeeds.

## Threat Model Coverage

| Threat ID | Disposition | Status |
|-----------|-------------|--------|
| T-02-18 (DoS hang) | mitigate | Closed — request + connect timeouts on shared client |
| T-02-19 (DoS memory) | mitigate | Closed — stream-and-bail at MAX_NIP11_BYTES |
| T-02-13 (completeness defeat) | mitigate | Closed — MAX_ADVERTISED_LIMIT upper clamp |

## Deviations from Plan

**1. [Rule 2 - Missing critical functionality] Stream-and-bail instead of `resp.bytes()`**
- **Found during:** Task 1
- **Issue:** The plan's action text suggested `resp.bytes().await` then checking `body.len()`. But `resp.bytes()` fully buffers the body into memory before the length check runs — which does not satisfy the stated behavior ("WITHOUT buffering the oversized payload") nor truly mitigate T-02-19 against a streaming attacker.
- **Fix:** `fetch_limits` reads the body via `resp.chunk()` in a loop, rejecting as soon as the accumulated length would exceed `MAX_NIP11_BYTES`, so an oversized body is never fully buffered. The pure `limits_from_bytes` seam (used by the offline test and as the final parse step) still performs the slice-length check for testability.
- **Files modified:** src/relay/nip11.rs
- **Commit:** 6a6e707

## Known Stubs

None.

## Self-Check: PASSED

- FOUND: src/relay/nip11.rs
- FOUND: tests/nip11_limits.rs
- FOUND commit: 28fca89, 6a6e707, dc10877, d53b718
- All 9 nip11_limits tests pass; cargo build succeeds.
