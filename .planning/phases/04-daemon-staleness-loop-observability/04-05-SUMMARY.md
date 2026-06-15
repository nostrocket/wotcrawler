---
phase: 04-daemon-staleness-loop-observability
plan: 05
subsystem: infra
tags: [tokio, axum, prometheus, cancellation-token, clap, daemon, graceful-shutdown, signals]

# Dependency graph
requires:
  - phase: 04-01
    provides: "daemon module scaffold, main.rs stub, [[bin]] crawler target, dep surface (tokio signal, axum, tokio-util, clap)"
  - phase: 04-02
    provides: "config::Config + load_config + validate (fail-fast, redacted Debug)"
  - phase: 04-03
    provides: "observe::init_tracing, install_metrics, AppState, router (/metrics + /health/*)"
  - phase: 04-04
    provides: "loop_::run_daemon_loop, sampler timers (sample_gauges/progress_summary/staleness_timer/in_run_reclaim_timer), frontier reclaim sweeps"
provides:
  - "src/daemon/mod.rs::run — the bootstrap-order orchestrator wiring tracing → recorder → DB → token → signals → fetch_union → axum → tasks → bounded drain"
  - "Production fetch_union closure: per-curated-relay raw fan-out concatenating events (D-08 single-ingest-over-union)"
  - "src/main.rs — clap --config entry with fail-fast non-zero exit on invalid config"
  - "A runnable, config-driven `crawler` daemon binary with live /metrics + /health/* and SIGTERM/SIGINT graceful shutdown"
affects: [spam-scoring-layer, deployment, operations]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Bootstrap order (Pitfall 1): tracing → install_metrics → connect+migrate → CancellationToken → signal task → fetch_union → axum → spawned tasks"
    - "Signal → shared CancellationToken → graceful drain; axum with_graceful_shutdown bound to the same token"
    - "Bounded shutdown join via tokio::time::timeout so a stuck task cannot hang exit (Pitfall 8)"
    - "Production fetch_union fans out fetch_complete_with_timeout per relay and concatenates RAW events — ingest runs ONCE over the union (D-08)"

key-files:
  created: []
  modified:
    - "src/daemon/mod.rs — added pub async fn run orchestrator + WANT_KIND/FUTURE_CLAMP_SECS/FOLLOW_CAP/MAX_AUTHORS_PER_REQ/SHUTDOWN_TIMEOUT consts"
    - "src/main.rs — replaced the 04-01 stub with the real clap entry (load → validate → run, ExitCode)"

key-decisions:
  - "Injected loop literals (want_kind=Kind::ContactList, future_clamp_secs=3600, follow_cap=10_000, max_authors=DEFAULT_MAX_LIMIT) are local consts in daemon/mod.rs — they have no config field or DEFAULT_* const; values mirror the Phase 3 crawl tests."
  - "fetch_union uses relay::fetch::fetch_complete_with_timeout (the RAW seam), NOT acquire_validated_lists_client, because process_batch must run ingest exactly once over the cross-relay union (D-08). Per-relay ingest would defeat newest-wins."
  - "Per-relay backoff reset (registry.reset) is called after each fully-successful per-relay fetch, mirroring acquire_validated_lists_client's behaviour over the raw seam."
  - "Anchor pubkey is parsed inside run() (PublicKey::parse, same path validate uses) to derive the seed bytes; main.rs validates first so this never fails in practice."

patterns-established:
  - "Daemon orchestrator pattern: one run(cfg) owns the full lifecycle; the binary is a thin clap+exit-code shell."
  - "Graceful-shutdown contract: stop claiming at the token boundary, drain in-flight workers to terminal status (zero orphaned in_progress), join all tasks under a total-time budget."

requirements-completed: [OPS-01, OPS-02, OBS-01, OBS-02, OBS-03, OBS-04, FRESH-02]

# Metrics
duration: 6min
completed: 2026-06-15
---

# Phase 4 Plan 5: Daemon Wiring & Observability Activation Summary

**The keystone `crawler` daemon: `main.rs` parses `--config` and fails fast on bad config; `daemon::run` boots tracing → Prometheus recorder → DB → signal-driven CancellationToken → a per-relay raw `fetch_union` → the axum `/metrics`+`/health/*` server → the continuous crawl + staleness + reclaim + sampler tasks, then drains cleanly on SIGTERM with zero orphaned leases.**

## Performance

- **Duration:** 6 min
- **Started:** 2026-06-15T04:37:36Z
- **Completed:** 2026-06-15T04:44:17Z
- **Tasks:** 2 of 2 autonomous tasks complete (Task 3 is a human-verify checkpoint — bounded smoke-tested, awaiting operator validation of a live-relay run)
- **Files modified:** 2

## Accomplishments
- `daemon::run` orchestrator: correct bootstrap order (Pitfall 1 — recorder installed before any metric fires), signal → token → graceful drain, axum graceful shutdown, and a bounded join so a stuck task cannot hang exit (Pitfall 8 / T-04-12).
- Production `fetch_union` fans out `fetch::fetch_complete_with_timeout` per curated relay and concatenates the RAW events so `process_batch` ingests ONCE over the cross-relay union (D-08); per-relay success resets that relay's backoff.
- `main.rs`: clap `--config`, load + validate with fail-fast non-zero exit BEFORE any DB connect or relay traffic (OPS-01 / T-04-04); `database_url` never printed (T-04-13).
- Bounded live smoke test (throwaway Postgres + a deliberately-unreachable relay) confirmed `/health/live`=200, `/health/ready`=200, `/metrics` renders `frontier_depth`/`crawl_coverage_ratio`/`relay_active_count`/`staleness_reenqueued_total`/`in_run_reclaimed_total`, periodic progress summaries (OBS-04), no DB-URL leak, and a real SIGTERM draining the one claimed lease to terminal `not_found` with **zero** `in_progress` rows remaining (OPS-02).

## Task Commits

Each autonomous task was committed atomically:

1. **Task 1: daemon::run orchestrator — bootstrap, fetch_union, signal, tasks, shutdown** - `3e2c83f` (feat)
2. **Task 2: main.rs entry — clap --config, load+validate, fail-fast exit** - `4ba9521` (feat)

**Plan metadata:** committed separately (docs: complete plan).

_Task 3 is a `checkpoint: human-verify` — no code commit; it requires an operator to validate a live multi-relay run + Grafana dashboard rendering._

## Files Created/Modified
- `src/daemon/mod.rs` - Added `pub async fn run(cfg: Config)` orchestrator and the injected-literal consts (`WANT_KIND`, `FUTURE_CLAMP_SECS`, `FOLLOW_CAP`, `MAX_AUTHORS_PER_REQ`, `SHUTDOWN_TIMEOUT`). Wires tracing/recorder/DB/token/signals/fetch_union/axum/tasks and the bounded drain.
- `src/main.rs` - Replaced the 04-01 no-op stub with the real clap-derive entry: `Args { --config }`, `#[tokio::main] -> ExitCode`, load → validate → `daemon::run`, fail-fast on each error.

## Decisions Made
- The four per-batch ingest/fetch literals (`want_kind`, `future_clamp_secs`, `follow_cap`, `max_authors`) are NOT config-sourced (no field exists) — they are local `const`s in `daemon/mod.rs` matching the values the Phase 3 `run_crawl` tests exercise. Documented inline.
- `fetch_union` deliberately uses the RAW `fetch::fetch_complete_with_timeout` seam (not `acquire_validated_lists_client`) to honour D-08 single-ingest-over-union; ingesting per relay would let a relay split a pubkey's events across relays to defeat newest-wins.

## Deviations from Plan

None - plan executed exactly as written. The plan's Task 1 action explicitly anticipated the raw-fetch seam choice (`fetch_complete` over `acquire_validated_lists_client`); that was followed, not a deviation.

## Issues Encountered
None during the autonomous tasks. The bounded live smoke test used a single deliberately-unreachable relay (`ws://127.0.0.1:65500`) because `validate` rejects an empty relay set; this still exercises the full claim → fetch-fail → terminal-status → drain path without real external relay traffic.

## User Setup Required
None for the autonomous work. For the live operator validation (checkpoint), the operator must supply a real `config.toml` (real `database_url` + `anchor_pubkey` + curated relays) and, optionally, import `ops/grafana-dashboard.json` into a Grafana pointed at the daemon's `/metrics`.

## Next Phase Readiness
- The `crawler` binary is fully wired and self-contained: `cargo build --bin crawler` exits 0, the full test suite (27 binaries) is green, clippy reports only the 5 pre-existing test-file warnings logged in `deferred-items.md` (no new warnings).
- Ready for operator validation of a real multi-relay crawl (the human-verify checkpoint) and for the downstream spam-scoring layer to begin reading the shared graph.

## Checkpoint Status (Task 3 — human-verify)

**Bounded automated smoke test PASSED** (build, full suite, in-process live run against a throwaway DB + unreachable relay, SIGTERM drain). **What remains for the operator:** a live run against real curated relays to confirm coverage/staleness metrics populate with real data, the `not_found`/`failed` validation-failure counters appear after real batches, and (optional OBS-05) the Grafana dashboard panels render against live Prometheus.

---
*Phase: 04-daemon-staleness-loop-observability*
*Completed: 2026-06-15*

## Self-Check: PASSED

- FOUND: src/daemon/mod.rs (modified)
- FOUND: src/main.rs (modified)
- FOUND: .planning/phases/04-daemon-staleness-loop-observability/04-05-SUMMARY.md
- FOUND commit: 3e2c83f (Task 1)
- FOUND commit: 4ba9521 (Task 2)
