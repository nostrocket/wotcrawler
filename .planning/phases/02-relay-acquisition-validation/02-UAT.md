---
status: testing
phase: 02-relay-acquisition-validation
source: [02-VERIFICATION.md]
started: 2026-06-13T11:20:25Z
updated: 2026-06-13T11:20:25Z
---

## Current Test

number: 1
name: Live-Relay Politeness Verification
expected: |
  Running against two real relays simultaneously for 60+ seconds, each relay is
  throttled independently at ≤ 4 req/sec (the GCRA per-relay quota, not a shared
  pool-wide quota). Rate-limited NOTICE messages produce per-relay escalating
  backoff visible in logs.
awaiting: user response

## Tests

### 1. Live-Relay Politeness Verification
expected: Running against two real relays simultaneously for 60+ seconds, each relay is throttled independently at ≤ 4 req/sec (per-relay GCRA quota, not shared pool-wide). Rate-limited NOTICE messages produce per-relay escalating backoff visible in logs.
result: [pending]

## Summary

total: 1
passed: 0
issues: 0
pending: 1
skipped: 0
blocked: 0

## Gaps
