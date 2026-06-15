//! The daemon's continuous, cancellation-aware crawl loop (OPS-02 / FRESH-02 /
//! CRAWL-04).
//!
//! [`run_daemon_loop`] is the long-running companion to
//! [`crate::crawl::run_crawl`]. It REUSES the proven Phase 3 primitives verbatim —
//! [`seed_anchor`], [`reclaim_stale_on_startup`], [`claim_batch`],
//! [`process_batch`], the `Semaphore` backpressure, and
//! [`crate::crawl::join_worker`] — and changes ONLY the loop control:
//!
//! - **Continuous (FRESH-02):** where `run_crawl` *terminates* on a drained
//!   frontier, this loop idle-polls. An empty claim drains in-flight workers (they
//!   may discover followees), then sleeps `idle_poll_interval` and re-claims. New
//!   `discovered` rows appear continuously as the FRESH-02 staleness scanner
//!   re-enqueues stale terminal rows, so the loop resumes work without restart.
//! - **Graceful shutdown (OPS-02):** cancellation is honored ONLY at the claim
//!   boundary — `token.is_cancelled()` before each claim and a `select!` on
//!   `token.cancelled()` inside the idle sleep. After the loop breaks, EVERY
//!   in-flight worker is drained via [`crate::crawl::join_worker`] so each leased
//!   row reaches a terminal status — zero orphaned `in_progress` leases (Pitfall 3
//!   / T-04-08).
//!
//! The spawned `process_batch` future is NEVER wrapped in `select!` (Pitfall 4 /
//! T-04-09): aborting it mid-`apply_follow_list` would leave a half-applied
//! transaction. Cancel always lands cleanly at the claim boundary; the in-flight
//! batch is allowed to finish during the drain.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use nostr_sdk::{Event, Kind, PublicKey, Timestamp};
use sqlx::PgPool;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::crawl::apply::process_batch;
use crate::crawl::frontier::{claim_batch, reclaim_stale_on_startup, seed_anchor, ClaimedAuthor};
use crate::crawl::{join_worker, CrawlStats};
use crate::error::{RelayError, StoreError};
use crate::relay::health::RelayHealthRegistry;

/// Drive the continuous, cancellation-aware crawl loop (OPS-02 / FRESH-02).
///
/// Startup mirrors [`crate::crawl::run_crawl`]: [`seed_anchor`] roots the BFS at
/// the anchor (CRAWL-01) and [`reclaim_stale_on_startup`] resets crash-orphaned
/// `in_progress` leases (D-06). Once seeding succeeds, `loop_alive` is set `true`
/// so `/health/ready` (OBS-03) can report the loop is up.
///
/// The loop then claims/processes batches exactly as `run_crawl` — owned-permit
/// backpressure before each spawn bounds in-flight fetches to
/// `concurrency × batch_size` authors (CRAWL-04) — with two control changes:
/// 1. `token.is_cancelled()` is checked BEFORE each claim; once set, the loop
///    stops claiming (OPS-02 — it never leases new rows during shutdown).
/// 2. an empty claim does NOT terminate: in-flight workers are drained (they may
///    have discovered followees), then a `select!` on `token.cancelled()` vs a
///    `idle_poll_interval` sleep either breaks (shutdown) or loops to re-claim.
///
/// After the loop breaks, all remaining workers are drained via [`join_worker`]
/// so every leased row reaches a terminal status — the OPS-02 zero-orphan
/// guarantee. Returns [`CrawlStats`] for the run.
///
/// The injected `fetch_union` closure has the SAME bounds as `run_crawl`'s
/// (`Clone + Send + Sync + 'static`): each spawned worker gets its own handle.
#[allow(clippy::too_many_arguments)]
pub async fn run_daemon_loop<F, Fut, FB, FutB, RL, FutR>(
    pool: &PgPool,
    anchor_pubkey: &[u8],
    batch_size: i64,
    concurrency: usize,
    want_kind: Kind,
    future_clamp_secs: u64,
    follow_cap: usize,
    max_attempts: i16,
    idle_poll_interval: Duration,
    token: CancellationToken,
    loop_alive: Arc<AtomicBool>,
    fetch_union: F,
    fallback_enabled: bool,
    nip65_max_write_relays: usize,
    health: Arc<RelayHealthRegistry>,
    fallback_fetch: FB,
    relay_list_fetch: RL,
) -> Result<CrawlStats, StoreError>
where
    F: Fn(Vec<ClaimedAuthor>) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<Vec<Event>, RelayError>> + Send + 'static,
    FB: Fn(PublicKey, Vec<String>) -> FutB + Clone + Send + Sync + 'static,
    FutB: Future<Output = Result<Vec<Event>, RelayError>> + Send + 'static,
    RL: Fn(PublicKey) -> FutR + Clone + Send + Sync + 'static,
    FutR: Future<Output = Result<Vec<Event>, RelayError>> + Send + 'static,
{
    // Startup: seed the anchor (D-03) then reclaim crash orphans (D-06) — reused
    // verbatim from run_crawl.
    seed_anchor(pool, anchor_pubkey).await?;
    let reclaimed = reclaim_stale_on_startup(pool).await?;

    // Seeding succeeded: announce the loop is alive so /health/ready (OBS-03) can
    // report up. Set AFTER the startup writes so readiness only flips once the
    // crawl is genuinely able to make progress.
    loop_alive.store(true, Ordering::Relaxed);

    let mut stats = CrawlStats {
        reclaimed_on_startup: reclaimed,
        ..Default::default()
    };

    let sem = Arc::new(Semaphore::new(concurrency));
    let mut workers: Vec<tokio::task::JoinHandle<Result<(), StoreError>>> = Vec::new();

    // RELAY-05/06: the live NIP-65 fallback wiring is now threaded in from the
    // daemon (05-04) — `fallback_enabled`/`nip65_max_write_relays` config, the
    // SHARED `health` registry (same binding the fan-out + notice consumer use),
    // and the live-Client `fallback_fetch`/`relay_list_fetch` closures. Each
    // spawned batch clones these so `process_batch` can recover a curated-miss
    // author from its write relays (apply.rs stays Client-free).

    // Run the loop + drain in an inner block so `loop_alive` is flipped back to
    // `false` on EVERY exit — clean shutdown OR an early `?` error (WR-03 / OBS-03:
    // `/health/ready` returns 200 only while the loop is genuinely alive). Without
    // this, readiness stays 200 through the drain and after the loop stops.
    let result: Result<CrawlStats, StoreError> = async {
        loop {
        // Cancel at the claim boundary (OPS-02): once shutdown is requested we stop
        // CLAIMING new rows. In-flight workers are drained after the loop. We never
        // wrap process_batch in a select! (Pitfall 4) — only claiming is cancelled.
        if token.is_cancelled() {
            break;
        }

        let batch = claim_batch(pool, batch_size).await?;
        if batch.is_empty() {
            // Frontier drained for now. Join ALL in-flight workers first — their
            // followee upserts may create new `discovered` rows. Every handle is
            // joined before propagating (WR-04): a panicking worker must not drop
            // the remaining handles' joins (their background writes are still in
            // flight), so we await all and surface the first error after.
            drain_all(&mut workers).await?;
            // Idle: sleep the poll interval, but wake immediately on cancellation so
            // shutdown is prompt even on a long idle. Cancel here breaks the loop;
            // the (now empty) worker set drains to a no-op below.
            tokio::select! {
                _ = token.cancelled() => break,
                _ = tokio::time::sleep(idle_poll_interval) => continue,
            }
        }

        stats.authors_claimed += batch.len();
        stats.batches_processed += 1;

        // Acquire a permit BEFORE spawning — blocks at the cap, so at most
        // `concurrency` batches are ever in flight (CRAWL-04 backpressure). Reused
        // verbatim from run_crawl.
        let permit = Arc::clone(&sem)
            .acquire_owned()
            .await
            .expect("daemon crawl semaphore is never closed");

        let pool = pool.clone();
        let fetch_union = fetch_union.clone();
        let fallback_fetch = fallback_fetch.clone();
        let relay_list_fetch = relay_list_fetch.clone();
        let health = Arc::clone(&health);
        let handle = tokio::spawn(async move {
            // Hold the permit for the whole batch; dropping it frees a slot. The
            // spawned future runs to completion — NEVER aborted by cancellation
            // (Pitfall 4 / T-04-09: an aborted apply_follow_list would half-commit).
            let _permit = permit;
            // Capture a FRESH wall-clock per batch (CR-01): the terminal stamps
            // written by `set_fetch_status` (not_found) and `requeue_or_fail`
            // (failed) derive `last_fetched_at` from this `now`. A single snapshot
            // taken once at daemon spawn would freeze `last_fetched_at` at the
            // start time, immediately re-enqueueing every not_found/failed row on
            // the next staleness scan and defeating FRESH-02. The success path
            // (`apply_follow_list`) uses SQL `now()` and is unaffected.
            let now = Timestamp::now();
            // Time the per-batch relay fetch and record it into the
            // `fetch_duration_seconds` histogram (WR-01): the buckets are
            // configured in observe::configured_builder but were never recorded,
            // leaving the Grafana p95 panel permanently empty. We wrap the fetch
            // future so the recorded span is exactly the relay round-trip (the
            // closure `process_batch` invokes to obtain the fetch future), not the
            // subsequent ingest/apply work.
            let inner = fetch_union(batch.clone());
            let fetch_timed = async move {
                let t0 = std::time::Instant::now();
                let result = inner.await;
                metrics::histogram!(crate::daemon::observe::METRIC_FETCH_DURATION)
                    .record(t0.elapsed().as_secs_f64());
                result
            };
            process_batch(
                &pool,
                &batch,
                want_kind,
                now,
                future_clamp_secs,
                follow_cap,
                max_attempts,
                fallback_enabled,
                nip65_max_write_relays,
                &health,
                || fetch_timed,
                // Live-Client NIP-65 fallback closures (RELAY-05/06): kind-3 from
                // write relays + the on-demand plain curated kind:10002 resolve.
                fallback_fetch,
                relay_list_fetch,
            )
            .await
            .map(|_applied| ())
        });
        workers.push(handle);

        // Opportunistically reap finished workers (same rationale as run_crawl:
        // bound the join-handle vector AND surface a worker's StoreError/panic so
        // its claimed rows never stay silently `in_progress`).
        let mut still_running = Vec::with_capacity(workers.len());
        let mut finished = Vec::new();
        for handle in workers.drain(..) {
            if handle.is_finished() {
                finished.push(handle);
            } else {
                still_running.push(handle);
            }
        }
        workers = still_running;
        for handle in finished {
            join_worker(handle).await?;
        }
        }

        // Graceful drain (OPS-02 / T-04-08): the loop has stopped claiming; join
        // EVERY remaining in-flight worker so each leased row reaches a terminal
        // status. A panicking worker must not short-circuit the join of the others
        // (WR-04): their process_batch writes are still in flight, and early-return
        // on the first error would leave those handles detached while reporting a
        // single error that masks the rest. drain_all awaits all, then surfaces the
        // first error — preserving the OPS-02 zero-orphan-lease guarantee.
        drain_all(&mut workers).await?;

        Ok(stats)
    }
    .await;

    // The loop has stopped (clean shutdown or error): readiness must no longer
    // report up (WR-03 / OBS-03). Set BEFORE returning so a probe racing the
    // return never sees 200 after the loop is gone.
    loop_alive.store(false, Ordering::Relaxed);

    result
}

/// Join EVERY handle in `workers`, awaiting all of them before returning, and
/// surface the FIRST error encountered (WR-04).
///
/// Unlike a `for handle in workers.drain(..) { join_worker(handle).await?; }`
/// loop — where a `?` on the first failing handle drops the remaining handles
/// without joining them — this awaits all handles so no in-flight worker is left
/// detached when one fails. A dropped `JoinHandle` does not abort its task, so an
/// early return would let background `process_batch` writes continue silently
/// while masking the other workers' errors. Draining all of them preserves the
/// OPS-02 zero-orphan-lease guarantee (every leased row reaches a terminal
/// status) and reports a deterministic first error.
async fn drain_all(
    workers: &mut Vec<tokio::task::JoinHandle<Result<(), StoreError>>>,
) -> Result<(), StoreError> {
    let mut first_err: Option<StoreError> = None;
    for handle in workers.drain(..) {
        if let Err(e) = join_worker(handle).await {
            if first_err.is_none() {
                first_err = Some(e);
            }
        }
    }
    match first_err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
