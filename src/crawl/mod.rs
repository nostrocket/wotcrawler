//! Crawl driver: the DB-resident BFS frontier and (in 03-03) the bounded worker
//! loop that turns the anchor pubkey into a complete, continuously-fresh follow
//! graph.
//!
//! The frontier is NOT a new data structure (D-01): it IS the set of
//! `pubkeys.status = 'discovered'` rows. Discovery is a side effect of
//! [`crate::store::pubkeys::upsert_pubkey`] landing every newly-seen followee as
//! `discovered` (D-03), so reachability (CRAWL-02) holds structurally — a pubkey
//! is only ever a row because someone already crawled followed it. This module
//! ships only the queue/lease mechanics; the hard primitives (`upsert_pubkey`,
//! `set_fetch_status`, `apply_follow_list`) are consumed, not rebuilt.
//!
//! Phase requirements served here:
//! - CRAWL-01: crawl starts from a single configurable anchor (see
//!   [`frontier::seed_anchor`]).
//! - CRAWL-02: only pubkeys discovered through follows are ever enqueued
//!   (structural — `upsert_pubkey`-on-followee is the only insertion path
//!   besides the anchor seed; there is no reach-ability predicate or recursive
//!   CTE — that anti-pattern is forbidden, RESEARCH Pitfall 4).
//! - CRAWL-03: the frontier is DB-resident and crash-safe — completed
//!   (`fetched`) work is never re-claimed ([`frontier::claim_batch`] selects only
//!   `discovered`), and orphaned `in_progress` leases are reset at startup
//!   ([`frontier::reclaim_stale_on_startup`]).
//! - CRAWL-04: in-flight concurrency is bounded (the bounded worker loop lands in
//!   03-03; this plan only ships the [`DEFAULT_CONCURRENCY`] knob it consumes).
//! - FRESH-01: every terminal transition stamps `last_fetched_at`
//!   ([`frontier::requeue_or_fail`] routes the terminal `failed` write through a
//!   timestamp-stamping UPDATE).

pub mod apply;
pub mod frontier;

/// Default number of `discovered` authors a worker batch-claims at once (D-07).
///
/// Sized near a typical NIP-11 `max_limit` / author-chunk so one claimed batch
/// maps to roughly one author-chunked relay request set, minimizing round-trips.
/// This is an explicit-discretion Phase 3 default (RESEARCH Open Question 3); the
/// Phase 4 daemon sources the real value from config (OPS-01) and overrides it.
pub const DEFAULT_BATCH_SIZE: i64 = 64;

/// Default cap on the number of batch fetches in flight at once (CRAWL-04).
///
/// A small multiple of the curated relay count keeps relay load and process
/// memory bounded — the "queue" is `pubkeys.status` in the DB, so the in-process
/// footprint is `DEFAULT_CONCURRENCY * DEFAULT_BATCH_SIZE` authors in flight, not
/// an unbounded in-memory queue. Explicit-discretion default (RESEARCH Open
/// Question 3); the bounded worker loop that consumes it lands in 03-03 and the
/// Phase 4 daemon sources the real value from config.
pub const DEFAULT_CONCURRENCY: usize = 8;

/// Default cap on transient-error fetch attempts before a pubkey is marked
/// `failed` (D-09).
///
/// On a transient fetch error a pubkey is returned to `discovered` with
/// `fetch_attempts` bumped; once `fetch_attempts` reaches this cap it transitions
/// to the terminal `failed` state so a flaky relay can never make a single pubkey
/// bounce `discovered <-> in_progress` forever (RESEARCH Pitfall 7). Default 3
/// per D-09; the Phase 4 daemon sources the real value from config.
pub const DEFAULT_MAX_ATTEMPTS: i16 = 3;

use std::future::Future;
use std::sync::Arc;

use nostr_sdk::{Event, Kind, Timestamp};
use sqlx::PgPool;
use tokio::sync::Semaphore;

use crate::crawl::apply::process_batch;
use crate::crawl::frontier::{
    claim_batch, reclaim_stale_on_startup, seed_anchor, ClaimedAuthor,
};
use crate::error::{RelayError, StoreError};

/// Outcome of a crawl run: how many batches/authors were processed and how many
/// orphaned `in_progress` leases were reclaimed at startup. Used for crawl
/// bookkeeping and test assertions; the Phase 4 daemon turns these into metrics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CrawlStats {
    /// Orphaned `in_progress` leases reset to `discovered` at startup (D-06).
    pub reclaimed_on_startup: u64,
    /// Total authors claimed across all batches over the whole run.
    pub authors_claimed: usize,
    /// Total batches processed (one fan-out/ingest/apply cycle each).
    pub batches_processed: usize,
}

/// Drive the BFS crawl from `anchor_pubkey` to component exhaustion with a
/// bounded worker pool (CRAWL-01/02/03/04, FRESH-01).
///
/// Startup (CRAWL-03): [`seed_anchor`] roots the BFS at the single configurable
/// anchor (D-03), then [`reclaim_stale_on_startup`] resets any orphaned
/// `in_progress` lease left by a crash back to `discovered` (D-06) — no in-run
/// reclaim sweep (deferred to Phase 4).
///
/// Loop (CRAWL-01/04): repeatedly [`claim_batch`] up to `batch_size` `discovered`
/// authors; an EMPTY claim means the frontier is drained, so we join all in-flight
/// workers and terminate (CRAWL-01 termination). Otherwise acquire one of
/// `concurrency` [`Semaphore`] permits — this BLOCKS at the cap, which is the
/// backpressure that bounds in-flight fetches to `concurrency × batch_size`
/// authors (CRAWL-04); the "queue" is `pubkeys.status` in the DB, never an
/// in-memory channel. Each permitted batch is spawned onto a worker that fans out,
/// runs a single cross-relay ingest pass, and applies per-author
/// ([`process_batch`]). Discovery of new followees happens inside `process_batch`
/// (the `upsert_pubkey`-on-followee lands them `discovered`), so the next claim
/// picks them up — that is the BFS frontier expanding (CRAWL-02 structural).
///
/// `fetch_union` is the injected raw-event source: given an owned batch it returns
/// the RAW `Vec<Event>` union across all curated relays (production fans out per
/// relay; tests inject a deterministic offline scripted graph). It is `Clone` so
/// each spawned worker gets its own handle, and `Send + Sync + 'static` so it can
/// cross the spawn boundary. There is NO status/reach-ability filter predicate
/// and NO recursive CTE anywhere — discovery is purely structural (Pitfall 4).
///
/// `now` / `future_clamp_secs` / `follow_cap` / `max_attempts` are passed straight
/// through to [`process_batch`].
#[allow(clippy::too_many_arguments)]
pub async fn run_crawl<F, Fut>(
    pool: &PgPool,
    anchor_pubkey: &[u8],
    batch_size: i64,
    concurrency: usize,
    want_kind: Kind,
    now: Timestamp,
    future_clamp_secs: u64,
    follow_cap: usize,
    max_attempts: i16,
    fetch_union: F,
) -> Result<CrawlStats, StoreError>
where
    F: Fn(Vec<ClaimedAuthor>) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<Vec<Event>, RelayError>> + Send + 'static,
{
    // Startup: seed the anchor (D-03) then reclaim crash orphans (D-06).
    seed_anchor(pool, anchor_pubkey).await?;
    let reclaimed = reclaim_stale_on_startup(pool).await?;

    let mut stats = CrawlStats {
        reclaimed_on_startup: reclaimed,
        ..Default::default()
    };

    let sem = Arc::new(Semaphore::new(concurrency));
    let mut workers: Vec<tokio::task::JoinHandle<Result<(), StoreError>>> = Vec::new();

    loop {
        let batch = claim_batch(pool, batch_size).await?;
        if batch.is_empty() {
            // The frontier is drained for now. New `discovered` rows are only
            // created by in-flight workers' followee upserts, so once those join
            // we re-check: a second empty claim after the join means real
            // exhaustion (CRAWL-01 termination).
            if workers.is_empty() {
                break;
            }
            // Join all in-flight workers, surfacing the first error, then loop to
            // re-claim any followees they just discovered.
            for handle in workers.drain(..) {
                join_worker(handle).await?;
            }
            continue;
        }

        stats.authors_claimed += batch.len();
        stats.batches_processed += 1;

        // Acquire a permit BEFORE spawning — this blocks at the cap, so at most
        // `concurrency` batches are ever in flight (CRAWL-04 backpressure).
        let permit = Arc::clone(&sem)
            .acquire_owned()
            .await
            .expect("crawl semaphore is never closed");

        let pool = pool.clone();
        let fetch_union = fetch_union.clone();
        let handle = tokio::spawn(async move {
            // Hold the permit for the whole batch; dropping it on completion frees
            // a slot for the next claim.
            let _permit = permit;
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
        workers.push(handle);

        // Opportunistically reap finished workers so `workers` does not grow
        // without bound across many claim iterations (the permit cap bounds
        // *concurrency*; this bounds the join-handle vector). We JOIN each
        // finished handle rather than dropping it (CR-01): a worker that returned
        // `Err(StoreError)` — a dropped DB connection mid-apply, an upsert failure,
        // a panic — must surface here, otherwise its claimed authors stay
        // `in_progress` forever (invisible to the `discovered`-only claim scan)
        // until the next restart's reclaim. Partition off the finished handles,
        // keep the still-running ones, then join the finished set so the first
        // error aborts the run (the same propagation the drain-on-empty path uses).
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

    Ok(stats)
}

/// Join one spawned worker, flattening the JoinError (panic/cancel) and the
/// inner `StoreError` into a single `StoreError`. A worker panic is surfaced as a
/// store error rather than silently swallowed.
pub(crate) async fn join_worker(
    handle: tokio::task::JoinHandle<Result<(), StoreError>>,
) -> Result<(), StoreError> {
    match handle.await {
        Ok(inner) => inner,
        Err(join_err) => {
            // A panicked/cancelled worker is a genuine failure; wrap it as a
            // sqlx protocol error so it propagates through the StoreError boundary
            // without inventing a new crawl-error enum for a should-not-happen case.
            Err(StoreError::Sqlx(sqlx::Error::Protocol(format!(
                "crawl worker task failed: {join_err}"
            ))))
        }
    }
}
