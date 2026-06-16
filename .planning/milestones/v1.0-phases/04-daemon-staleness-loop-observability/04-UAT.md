---
status: deferred
phase: 04-daemon-staleness-loop-observability
source: [04-VERIFICATION.md]
started: 2026-06-15
updated: 2026-06-15
---

## Current Test

number: 1
name: Live relay crawl run against real curated relays
expected: |
  Running `crawler --config <real>.toml` against real curated relays with a real
  database_url + anchor_pubkey: /health/live and /health/ready respond correctly,
  /metrics populates coverage/staleness/relay-health/frontier-depth/fetch-rate and
  validation-failure counters with real data, database_url never appears in logs,
  and SIGTERM drains cleanly leaving zero rows in status='in_progress'.
awaiting: operator validation (deferred by user during autonomous run)

## Tests

### 1. Live relay crawl run
expected: Daemon serves health/metrics against real Postgres + relays; SIGTERM drains with zero orphaned in_progress leases; database_url never logged; validation-failure counters appear after real batches process.
result: [pending — operator]

### 2. Grafana dashboard rendering (OBS-05, optional)
expected: Importing `ops/grafana-dashboard.json` into a Grafana pointed at the daemon's /metrics renders all panels with data (note: counter panels use the exporter's `_total` exposition names, fixed in review WR-02).
result: [pending — operator]

## Summary

total: 2
passed: 0
issues: 0
pending: 2
skipped: 0
blocked: 0

## Gaps

(none — bounded smoke-test this session confirmed build, health/metrics serving, progress summaries, and SIGTERM graceful-drain with zero orphaned leases. These two items require real relay access + a live Grafana/Prometheus and were explicitly deferred to operator UAT by the user during the autonomous run. Re-run via `/gsd-verify-work 4` after the live run.)
