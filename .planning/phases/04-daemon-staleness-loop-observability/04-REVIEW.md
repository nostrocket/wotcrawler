---
phase: 04-daemon-staleness-loop-observability
reviewed: 2026-06-15T00:00:00Z
depth: standard
files_reviewed: 11
files_reviewed_list:
  - src/main.rs
  - src/daemon/mod.rs
  - src/daemon/config.rs
  - src/daemon/observe.rs
  - src/daemon/loop_.rs
  - src/daemon/sampler.rs
  - src/crawl/frontier.rs
  - src/crawl/mod.rs
  - src/lib.rs
  - migrations/0003_staleness.sql
  - config.example.toml
  - ops/grafana-dashboard.json
findings:
  critical: 2
  warning: 4
  info: 1
  total: 7
status: resolved
resolution:
  fixed: [CR-01, CR-02, WR-01, WR-02, WR-03, WR-04]
  skipped: [IN-01]
  skipped_reason: "IN-01 is Info-tier (out of Critical+Warning fix scope) — an optional clarifying comment; no behavioral change."
  fixed_at: 2026-06-15
---

# Phase 4: Code Review Report

**Reviewed:** 2026-06-15
**Depth:** standard
**Files Reviewed:** 11
**Status:** issues_found

## Summary

This phase implements the daemon orchestrator, continuous crawl loop, TTL staleness refresh, Prometheus observability, and graceful shutdown. The overall design is sound: the cancellation-token drain pattern is correct, the DB primitives are safe parameterized SQL, the `Config::Debug` redaction is solid, and the signal-task fire-and-forget pattern is appropriate (the task returns `()`, so dropping its `JoinHandle` is not the CR-01 class bug from Phase 3).

Two critical issues were found: a `Timestamp::now()` frozen at daemon-spawn time corrupts `last_fetched_at` for `not_found` and `failed` rows across multi-day runs, and a `concurrency = 0` config value deadlocks the crawl loop permanently. Additionally, the `fetch_duration_seconds` histogram is configured with buckets but never recorded anywhere, and Grafana queries use bare counter names without the `_total` suffix that `metrics-exporter-prometheus` 0.18 appends, producing empty panels. Two structural concerns round out the warnings.

---

## Critical Issues

### CR-01: `Timestamp::now()` frozen at daemon spawn — corrupts `last_fetched_at` for `not_found`/`failed` rows  [RESOLVED — commit caf5124]

**File:** `src/daemon/mod.rs:255`

**Issue:** `Timestamp::now()` is called once when `run_daemon_loop` is spawned and passed as the `now` parameter. This single snapshot is reused for every batch processed over the entire daemon lifetime. Inside `process_batch`, the `stamp` used for `last_fetched_at` writes is derived from this frozen value:

- `apply_follow_list` (the success path) uses `now()` directly in SQL — correct, unaffected.
- `set_fetch_status` (the `not_found` path, `apply.rs:181/194`) uses `stamp`, which is `timestamp_to_datetime(now)` — **stale after the first hours of operation**.
- `requeue_or_fail` (the `failed` path, `apply.rs:156`) uses `stamp` — **stale**.

After a 3-day run, a pubkey that resolves to `not_found` will be written with `last_fetched_at = daemon_start_time`. With a 24h TTL this row is immediately re-enqueued on the next staleness scan, causing continuous spurious re-fetching of every `not_found`/`failed` pubkey and defeating the FRESH-02 freshness guarantee for those status classes.

**Fix:** Call `Timestamp::now()` inside `run_daemon_loop` per batch iteration, or pass a clock factory. The simplest minimal fix is to move the call inside the spawned batch closure in `loop_.rs`:

```rust
// In loop_.rs, inside the tokio::spawn async move block:
let handle = tokio::spawn(async move {
    let _permit = permit;
    let now = nostr_sdk::Timestamp::now();   // fresh per-batch clock
    let fut = fetch_union(batch.clone());
    process_batch(
        &pool,
        &batch,
        want_kind,
        now,
        future_clamp_secs,
        follow_cap,
        max_attempts,
        || fut,
    )
    .await
    .map(|_applied| ())
});
```

Remove the `now: Timestamp` parameter from `run_daemon_loop`'s signature and from the call site in `mod.rs:255`.

---

### CR-02: `concurrency = 0` config deadlocks the crawl loop permanently  [RESOLVED — commit c682dfc]

**File:** `src/daemon/config.rs:186` (validate), `src/daemon/loop_.rs:99`

**Issue:** `validate()` does not check `concurrency > 0`. `Semaphore::new(0)` creates a semaphore with zero permits; `acquire_owned()` on it blocks forever unless `close()` is called, and `close()` is never called in this code path. A misconfigured `concurrency = 0` in `config.toml` silently produces a daemon that seeds the anchor, sets `loop_alive = true` (so `/health/ready` reports 200), then hangs indefinitely on the first `acquire_owned()` call — processing no batches, emitting no errors, appearing healthy to all probes.

The same applies to `batch_size <= 0`: `LIMIT` with a negative value is a PostgreSQL error (`ERROR: LIMIT must not be negative`), crashing the crawler on the first `claim_batch` call instead of failing fast at startup.

**Fix:** Add these guards to `validate()`:

```rust
anyhow::ensure!(c.concurrency > 0, "concurrency must be > 0");
anyhow::ensure!(c.batch_size > 0, "batch_size must be > 0");
// Also move the reqs_per_second check here from run():
anyhow::ensure!(c.reqs_per_second > 0, "reqs_per_second must be > 0");
```

The `reqs_per_second` guard already exists in `run()` at `mod.rs:158–159` but fires after DB connection and relay pool setup, violating the OPS-01 fail-fast requirement. Move it into `validate()` as well.

---

## Warnings

### WR-01: `fetch_duration_seconds` histogram configured but never recorded — Grafana panel always empty  [RESOLVED — commit 04f0ff6]

**File:** `src/daemon/observe.rs:61`, `ops/grafana-dashboard.json` panel 3

**Issue:** `METRIC_FETCH_DURATION` (`"fetch_duration_seconds"`) has its buckets configured in `configured_builder()` (observe.rs:103–107) but `metrics::histogram!(METRIC_FETCH_DURATION).record(...)` is called nowhere in the codebase. The Grafana dashboard panel 3 ("Fetch rate / latency") queries `histogram_quantile(0.95, sum(rate(fetch_duration_seconds_bucket[5m])) by (le))` — this panel is permanently empty. The constant and the bucket registration are dead code.

**Fix:** Record the histogram at the batch fetch boundary in `loop_.rs`, wrapping the `fetch_union` call:

```rust
let t0 = std::time::Instant::now();
let fut = fetch_union(batch.clone());
// After process_batch returns (or the fetch completes):
metrics::histogram!(crate::daemon::observe::METRIC_FETCH_DURATION)
    .record(t0.elapsed().as_secs_f64());
```

Alternatively, remove the constant, the bucket registration, and the Grafana panel if fetch latency telemetry is deferred.

---

### WR-02: Grafana counter queries missing `_total` suffix — relay health and ingest panels produce no data  [RESOLVED — commit 2cfc7e1]

**File:** `ops/grafana-dashboard.json:137,143,160,166,172,178`

**Issue:** `metrics-exporter-prometheus` 0.18.x appends `_total` to every counter name in the Prometheus text exposition (per OpenMetrics convention). The Grafana panel 5 queries `relay_rate_limited` and `relay_blocked`; the actual exported names are `relay_rate_limited_total` and `relay_blocked_total`. The panel 6 queries for all four `ingest_*` counters have the same mismatch. All six panels will show no data.

Additionally, `sampler.rs:241` names the counter `"staleness_reenqueued_total"` — after the exporter appends `_total` the metric becomes `staleness_reenqueued_total_total`. Similarly `"in_run_reclaimed_total"` → `in_run_reclaimed_total_total`. These are ugly but functional (the Grafana dashboard does not reference them directly, so no broken panel), but the doubled suffix is incorrect naming.

**Fix:** In `grafana-dashboard.json`, add `_total` to the six counter names:

```json
"expr": "sum(rate(relay_rate_limited_total[5m]))"
"expr": "sum(rate(relay_blocked_total[5m]))"
"expr": "sum(rate(ingest_invalid_signature_total[5m]))"
// ... etc
```

Rename the sampler counters to drop the manually-appended `_total`:

```rust
metrics::counter!("staleness_reenqueued").increment(n);
metrics::counter!("in_run_reclaimed").increment(n);
```

---

### WR-03: `loop_alive` never reset to `false` on shutdown — `/health/ready` stays 200 after the loop stops  [RESOLVED — commit a7c4d2a]

**File:** `src/daemon/loop_.rs:92`, `src/daemon/observe.rs:193`

**Issue:** `loop_alive` is set `true` once in `run_daemon_loop` after seeding (line 92) and never set back to `false` when the loop exits (either on cancellation or on error). After `token.cancel()` fires and the loop drains, `/health/ready` continues to return `200 OK` for the duration of the drain and beyond (until the axum server shuts down). During the graceful shutdown window this could give a load balancer or orchestrator a false signal that the crawler is still processing work, delaying traffic re-routing to a replacement instance.

The severity is mild in a single-instance self-hosted deployment but is a semantic contract violation (OBS-03 says "200 only when the crawl loop is alive").

**Fix:** Set `loop_alive` to `false` on exit from `run_daemon_loop`, before returning:

```rust
// At the end of run_daemon_loop, before Ok(stats):
loop_alive.store(false, Ordering::Relaxed);
Ok(stats)
```

---

### WR-04: Early drain abort on first `join_worker` error leaves remaining JoinHandles detached  [RESOLVED — commit 38754d7]

**File:** `src/daemon/loop_.rs:183–185`

**Issue:** The post-cancellation drain (lines 183–185) is:

```rust
for handle in workers.drain(..) {
    join_worker(handle).await?;
}
```

If `join_worker` returns `Err` for handle N (a panicked task wrapped as `StoreError::Sqlx(Protocol(...))`), the `?` returns early from `run_daemon_loop`. The remaining handles in `workers.drain()` are dropped without being joined. A dropped `JoinHandle` does NOT abort the task — the tasks continue running detached. Their `process_batch` calls will complete and write their own terminal status to the DB, so there are no orphaned `in_progress` rows. However, `run_daemon_loop` has returned `Err` and the caller logs it as `"crawl loop ended with error"`, while background tasks are silently making DB writes. The error from the first failed worker also masks subsequent worker errors.

This is the same structural issue as the Phase 3 CR-01 class bug, here in the drain path rather than the main loop.

**Fix:** Drain all handles before returning an error, collecting all errors and returning the first:

```rust
let mut first_err: Option<StoreError> = None;
for handle in workers.drain(..) {
    if let Err(e) = join_worker(handle).await {
        if first_err.is_none() {
            first_err = Some(e);
        }
    }
}
if let Some(e) = first_err {
    return Err(e);
}
```

Apply the same fix to the `idle-drain` path at lines 116–118 if you want symmetric behavior (that path has the same structure).

---

## Info

### IN-01: Signal task JoinHandle silently dropped — intentional but worth documenting  [SKIPPED — Info-tier, out of fix scope]

**File:** `src/daemon/mod.rs:128–151`

**Issue:** The signal-listening task spawned at line 130 has its `JoinHandle` dropped immediately (the block ends at line 151 with no binding). This is the correct design — the task's job is purely to call `token.cancel()` on signal arrival and then exit — but it superficially resembles the Phase 3 CR-01 class bug (dropped JoinHandle on a `Result`-returning worker). The task returns `()`, not `Result<(), StoreError>`, so there is nothing to observe from the join. The distinction from the problematic pattern should be noted in a comment.

**Fix:** Add a comment clarifying the intentional fire-and-forget:

```rust
// Fire-and-forget: the signal task returns `()` (no Result to observe),
// so dropping its JoinHandle is safe. It will run until a signal arrives,
// call token.cancel(), and exit naturally when the runtime shuts down.
let _signal_task = tokio::spawn(async move { ... });
```

---

_Reviewed: 2026-06-15_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
