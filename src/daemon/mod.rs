//! Phase 4 daemon: the long-running, signal-driven orchestrator that turns the
//! library-only crawler into a single configurable daemon binary (OPS-01).
//!
//! Requirement map (filled across Phase 4 plans):
//! - OPS-01 — single `crawler` daemon binary wiring the existing modules.
//! - OPS-02 — graceful shutdown (SIGTERM/SIGINT → `CancellationToken`) that drains
//!   in-flight workers and leaves the DB with no orphaned `in_progress` leases,
//!   plus a periodic in-run stale-lease reclaim sweep
//!   ([`crate::crawl::frontier::reclaim_in_progress_older_than`]).
//! - FRESH-02 — the TTL-driven staleness scanner re-enqueues stale rows into the
//!   same `pubkeys.status='discovered'` frontier
//!   ([`crate::crawl::frontier::reclaim_stale_by_ttl`]).
//! - OBS-01..05 — Prometheus metrics, structured logging, `/health/live` +
//!   `/health/ready` endpoints, periodic crawl-progress summaries, and a committed
//!   Grafana dashboard.
//!
//! Submodules (`config`, `observe`, `sampler`, `loop_`) are registered as their
//! owning Phase 4 plans land; this is the module root keystone (04-01).

/// Daemon configuration: layered TOML + `WOT__*` env load and fail-fast
/// validation (OPS-01). See [`config::Config`], [`config::load_config`],
/// [`config::validate`].
pub mod config;

/// Observability surface (OBS-01/02/03): Prometheus recorder install, `tracing`
/// init with human/JSON format selection, and the axum router serving `/metrics`
/// + `/health/live` + `/health/ready`. See [`observe::install_metrics`],
/// [`observe::init_tracing`], [`observe::router`].
pub mod observe;

/// The continuous, cancellation-aware crawl loop (OPS-02 / FRESH-02 / CRAWL-04):
/// reuses the Phase 3 crawl primitives and replaces `run_crawl`'s break-on-empty
/// with idle-poll + a claim-boundary cancellation drain. See
/// [`loop_::run_daemon_loop`]. (Named `loop_` because `loop` is a keyword.)
pub mod loop_;

/// Periodic observability + maintenance timers (OBS-01/04, FRESH-02, OPS-02):
/// the gauge sampler, progress-summary logger, TTL staleness scan, and in-run
/// stale-lease reclaim, each a coarse-interval task stopping on cancellation.
/// See [`sampler::sample_gauges`], [`sampler::progress_summary`],
/// [`sampler::staleness_timer`], [`sampler::in_run_reclaim_timer`].
pub mod sampler;

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use nostr_sdk::{Kind, PublicKey};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::daemon::config::Config;
use crate::relay::health::RelayHealthRegistry;
use crate::relay::nip11::{LimitCache, DEFAULT_MAX_LIMIT};
use crate::relay::rate_limit::{
    RateLimiterRegistry, DEFAULT_BACKOFF_BASE, DEFAULT_BACKOFF_CAP,
};
use crate::relay::{connect_curated, fetch, spawn_notice_consumer, ReconnectPolicy};

/// The kind-3 follow-list event the crawler fetches (CRAWL / D-08). The whole
/// project is built around kind:3 contact lists; this is the only kind solicited.
const WANT_KIND: Kind = Kind::ContactList;

/// Reject any follow-list event dated more than this many seconds into the
/// future (ingest future-clamp, T-02-…): a relay cannot post-date an event to
/// win newest-wins forever. One hour of clock skew is generous. Mirrors the
/// value the Phase 3 crawl tests exercise.
const FUTURE_CLAMP_SECS: u64 = 3_600;

/// Upper bound on the number of followees accepted from a single follow list
/// (ingest follow-cap). Mirrors the Phase 3 crawl tests; far above any honest
/// follow count, but bounds a hostile oversized list.
const FOLLOW_CAP: usize = 10_000;

/// Max authors solicited per relay REQ window (author-chunking, RELAY-03). The
/// per-relay fetch chunks the batch's authors under this cap; sized at the
/// default NIP-11 `max_limit` so one batch maps to roughly one window set.
const MAX_AUTHORS_PER_REQ: usize = DEFAULT_MAX_LIMIT;

/// Total wall-clock budget for the graceful-shutdown drain (Pitfall 8 / T-04-12):
/// once cancellation fires, the loop + timers are joined under this timeout so a
/// single stuck task can never hang the process forever.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

/// Boot and run the full crawler daemon to graceful shutdown (OPS-01 / OPS-02).
///
/// Bootstrap order is load-bearing (RESEARCH §Architecture, Pitfall 1):
/// 1. [`observe::init_tracing`] — install the subscriber before the first span.
/// 2. [`observe::install_metrics`] — install the Prometheus recorder BEFORE any
///    `metrics::*!` fires; the six existing counter sites become live here.
/// 3. [`crate::store::connect`] + [`crate::store::run_migrations`] — open the
///    pool and bring the schema current. The `database_url` is NEVER logged
///    (T-04-13).
/// 4. A shared [`CancellationToken`] + `loop_alive` flag.
/// 5. A signal task (SIGTERM/SIGINT → `token.cancel()`) — RESEARCH shutdown wiring.
/// 6. The production `fetch_union`: a connected curated pool + a shared
///    [`RateLimiterRegistry`] + a [`LimitCache`], whose closure fans out the RAW
///    [`fetch::fetch_complete`] per curated relay and CONCATENATES the raw events
///    (D-08 single-ingest-over-union — NEVER ingest per relay; `process_batch`
///    runs one ingest pass over the whole union).
/// 7. The axum `/metrics` + `/health/*` server, bound to `cfg.metrics_addr`, with
///    `with_graceful_shutdown` tied to the same token.
/// 8. The long-running tasks (continuous loop + sampler + progress summary +
///    staleness scan + in-run reclaim), each holding a clone of the token.
///
/// On cancel the loop stops claiming and drains in-flight workers (zero orphaned
/// `in_progress` leases — OPS-02); the server shuts down gracefully; the timers
/// stop; all are joined under [`SHUTDOWN_TIMEOUT`] so a stuck task cannot hang
/// shutdown (Pitfall 8). Returns `Ok(())` on a clean exit.
pub async fn run(cfg: Config) -> anyhow::Result<()> {
    // (1) Tracing first so every subsequent line is structured (OBS-02).
    observe::init_tracing(&cfg.log_level, cfg.log_format);

    // Redacted config echo — `Config`'s Debug redacts `database_url` (T-04-13).
    tracing::info!(config = ?cfg, "crawler daemon starting");

    // (2) Install the Prometheus recorder BEFORE any metric fires (Pitfall 1).
    let handle = observe::install_metrics();

    // (3) Connect + migrate. The DB URL is never logged (T-04-13).
    let pool = crate::store::connect(&cfg.database_url).await?;
    crate::store::run_migrations(&pool).await?;
    tracing::info!("database connected and migrations applied");

    // (4) Shared cancellation token + loop-alive readiness flag.
    let token = CancellationToken::new();
    let loop_alive = Arc::new(AtomicBool::new(false));

    // (5) Signal listener: SIGTERM/SIGINT → cancel the shared token (OPS-02).
    {
        let token = token.clone();
        tokio::spawn(async move {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{signal, SignalKind};
                let mut term =
                    signal(SignalKind::terminate()).expect("install SIGTERM handler");
                let mut int =
                    signal(SignalKind::interrupt()).expect("install SIGINT handler");
                tokio::select! {
                    _ = term.recv() => tracing::info!("received SIGTERM"),
                    _ = int.recv() => tracing::info!("received SIGINT"),
                }
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
                tracing::info!("received Ctrl-C");
            }
            tracing::info!("cancellation requested — beginning graceful shutdown");
            token.cancel();
        });
    }

    // (6) Production fetch_union. Connect the curated pool with the reconnect
    // policy (RELAY-01), build the shared rate-limiter registry + NIP-11 cache,
    // and spawn the notice consumer so rate-limited/blocked notices escalate the
    // SAME per-relay counters the fetch gate consults (RELAY-04).
    let client = connect_curated(&cfg.relays, ReconnectPolicy::crawler_default()).await?;
    // `reqs_per_second > 0` is guaranteed by `config::validate` (OPS-01 fail-fast,
    // checked at startup before any DB/relay setup), so this NonZeroU32 build is
    // infallible here; the expect documents that invariant rather than re-checking.
    let reqs_per_second = std::num::NonZeroU32::new(cfg.reqs_per_second)
        .expect("config::validate guarantees reqs_per_second > 0");
    let registry = Arc::new(RateLimiterRegistry::with_params(
        reqs_per_second,
        DEFAULT_BACKOFF_BASE,
        DEFAULT_BACKOFF_CAP,
    ));
    // RELAY-06: the parallel per-relay health registry, built once beside the
    // rate-limiter registry and shared behind an `Arc`. The NOTICE consumer
    // degrades a relay's health on a `rate-limited` notice; 05-04 extends this
    // exact `health` binding into the health-driven fan-out + per-relay
    // concurrency admission (do not rename or duplicate it).
    let health = Arc::new(RelayHealthRegistry::new(cfg.health_alpha));
    let _notice_consumer =
        spawn_notice_consumer(client.clone(), Arc::clone(&registry), Arc::clone(&health));
    let limit_cache = Arc::new(LimitCache::new());

    // RELAY-06 per-relay admission semaphores: one FIXED-SIZE
    // `Semaphore::new(per_relay_concurrency)` per curated relay url, built once
    // and cloned into the fan-out closure. `tokio::Semaphore` cannot shrink
    // (Pitfall 5), so this is the hard ceiling; the health score scales EFFECTIVE
    // concurrency down via the `in_use` admission gate in
    // [`crate::relay::health::admit_per_relay`], never by resizing the semaphore.
    let per_relay_sems: Arc<HashMap<String, Arc<Semaphore>>> = Arc::new(
        cfg.relays
            .iter()
            .map(|r| {
                (
                    r.clone(),
                    Arc::new(Semaphore::new(cfg.per_relay_concurrency)),
                )
            })
            .collect(),
    );

    // The closure fans out the RAW fetch per curated relay and CONCATENATES the
    // raw events (D-08): `process_batch` runs ONE ingest pass over the whole
    // cross-relay union, so a relay cannot split a pubkey's events across relays
    // to defeat newest-wins. We use `fetch::fetch_complete` (the raw seam that
    // `acquire_validated_lists_client` wraps) so NO ingest happens per relay.
    let fetch_union = {
        let client = client.clone();
        let relays = cfg.relays.clone();
        let registry = Arc::clone(&registry);
        let limit_cache = Arc::clone(&limit_cache);
        let health = Arc::clone(&health);
        let per_relay_sems = Arc::clone(&per_relay_sems);
        let fetch_timeout = cfg.fetch_timeout;
        let relay_health_threshold = cfg.relay_health_threshold;
        let per_relay_concurrency = cfg.per_relay_concurrency;
        move |batch: Vec<crate::crawl::frontier::ClaimedAuthor>| {
            let client = client.clone();
            let relays = relays.clone();
            let registry = Arc::clone(&registry);
            let limit_cache = Arc::clone(&limit_cache);
            let health = Arc::clone(&health);
            let per_relay_sems = Arc::clone(&per_relay_sems);
            async move {
                // The set of authors this batch solicited.
                let authors: Vec<PublicKey> = batch
                    .iter()
                    .filter_map(|c| PublicKey::from_slice(&c.pubkey).ok())
                    .collect();
                if authors.is_empty() {
                    return Ok(Vec::new());
                }

                // Fan out per curated relay, concatenating raw events (D-08 single
                // ingest over the union). RELAY-06 makes this health-driven:
                //
                // DEADLOCK-SAFE ACQUISITION ORDER (fixed EVERYWHERE — Pitfall 1):
                //   global crawl permit (already held — acquired in
                //   loop_.rs before this batch was spawned)
                //     -> per-relay permit (admit_per_relay)
                //       -> GCRA token (inside fetch_complete_with_timeout)
                //         -> fetch
                // A global permit is NEVER acquired while a per-relay permit is
                // held, so the gates never form a cycle.
                //
                // (1) skip a relay below the health threshold unless a probe is due
                //     (route_allowed), re-admitting recovered relays via the probe;
                // (2) admit through the fixed per-relay Semaphore + health-scaled
                //     in-use gate (admit_per_relay);
                // (3) gate each window REQ behind the per-relay GCRA token
                //     (fetch_complete_with_timeout);
                // (4) record the per-relay outcome into the health registry at the
                //     Ok/Err arms BEFORE propagating, so a per-relay error still
                //     requeues the whole batch (D-09) but is observed first.
                let mut union: Vec<nostr_sdk::Event> = Vec::new();
                for relay_url in &relays {
                    // (1) Skip a degraded relay unless a probe is due; mark the
                    // attempt so the next skip window starts now.
                    if !health.route_allowed(relay_url, relay_health_threshold) {
                        continue;
                    }
                    health.mark_attempt(relay_url);

                    let max_limit = limit_cache.get_or_fetch(relay_url).await.max_limit;
                    let sem = per_relay_sems
                        .get(relay_url)
                        .expect("every curated relay has a per-relay semaphore");

                    // (2)+(3) Admit through the per-relay gate, then run the
                    // GCRA-gated fetch. Time the per-relay round-trip for the health
                    // success-latency sample.
                    let t0 = std::time::Instant::now();
                    let outcome = crate::relay::health::admit_per_relay(
                        &health,
                        sem,
                        relay_url,
                        per_relay_concurrency,
                        || {
                            fetch::fetch_complete_with_timeout(
                                &client,
                                relay_url,
                                &authors,
                                WANT_KIND,
                                max_limit,
                                MAX_AUTHORS_PER_REQ,
                                fetch_timeout,
                                &registry,
                            )
                        },
                    )
                    .await;
                    let latency = t0.elapsed();

                    // (4) Record health at the Ok/Err arms BEFORE propagating.
                    fetch::record_fetch_health(&health, relay_url, latency, &outcome);

                    let events = outcome?;
                    // A fully-successful per-relay fetch clears that relay's backoff.
                    registry.reset(relay_url);
                    union.extend(events);
                }
                Ok::<_, crate::error::RelayError>(union)
            }
        }
    };

    // (7) axum observability server bound to cfg.metrics_addr (OBS-01/03), with
    // graceful shutdown tied to the same token (Pitfall 8 — handlers are fast and
    // the /health/ready DB ping is timeout-bounded).
    let state = observe::AppState::new(handle, Arc::clone(&loop_alive), pool.clone());
    let listener = tokio::net::TcpListener::bind(cfg.metrics_addr).await?;
    tracing::info!(addr = %cfg.metrics_addr, "observability server listening");
    let server = {
        let token = token.clone();
        axum::serve(listener, observe::router(state)).with_graceful_shutdown(async move {
            token.cancelled().await;
        })
    };

    // (8) Spawn the long-running tasks, each holding a clone of the token.
    let anchor_bytes = PublicKey::parse(&cfg.anchor_pubkey)
        .map_err(|e| anyhow::anyhow!("invalid anchor_pubkey: {e}"))?
        .to_bytes()
        .to_vec();

    let loop_task = {
        let pool = pool.clone();
        let token = token.clone();
        let loop_alive = Arc::clone(&loop_alive);
        let batch_size = cfg.batch_size;
        let concurrency = cfg.concurrency;
        let max_attempts = cfg.max_attempts;
        let idle_poll_interval = cfg.idle_poll_interval;
        tokio::spawn(async move {
            loop_::run_daemon_loop(
                &pool,
                &anchor_bytes,
                batch_size,
                concurrency,
                WANT_KIND,
                FUTURE_CLAMP_SECS,
                FOLLOW_CAP,
                max_attempts,
                idle_poll_interval,
                token,
                loop_alive,
                fetch_union,
            )
            .await
        })
    };

    let sampler_task = {
        let pool = pool.clone();
        let registry = Arc::clone(&registry);
        let relays = cfg.relays.clone();
        let token = token.clone();
        let interval = cfg.progress_interval;
        tokio::spawn(async move {
            sampler::sample_gauges(pool, registry, relays, token, interval).await
        })
    };

    let progress_task = {
        let pool = pool.clone();
        let token = token.clone();
        let interval = cfg.progress_interval;
        tokio::spawn(async move { sampler::progress_summary(pool, token, interval).await })
    };

    let staleness_task = {
        let pool = pool.clone();
        let ttl_secs = cfg.ttl.as_secs() as i64;
        let interval = cfg.staleness_scan_interval;
        let token = token.clone();
        tokio::spawn(
            async move { sampler::staleness_timer(pool, ttl_secs, interval, token).await },
        )
    };

    let reclaim_task = {
        let pool = pool.clone();
        let age_secs = cfg.reclaim_age.as_secs() as i64;
        let interval = cfg.reclaim_interval;
        let token = token.clone();
        tokio::spawn(async move {
            sampler::in_run_reclaim_timer(pool, age_secs, interval, token).await
        })
    };

    // Await the axum server (it returns once graceful shutdown completes on
    // token-cancel). A server bind/serve error is surfaced; the loop's crawl
    // result is checked after the bounded drain.
    server.await?;
    tracing::info!("observability server shut down; draining tasks");

    // Bounded graceful drain (Pitfall 8 / T-04-12): join every task under a total
    // timeout so a stuck task cannot hang shutdown forever. The loop's drain leaves
    // zero orphaned `in_progress` leases (OPS-02).
    let drain = async {
        match loop_task.await {
            Ok(Ok(stats)) => tracing::info!(
                batches = stats.batches_processed,
                authors = stats.authors_claimed,
                reclaimed_on_startup = stats.reclaimed_on_startup,
                "crawl loop drained cleanly"
            ),
            Ok(Err(e)) => tracing::error!(error = %e, "crawl loop ended with error"),
            Err(e) => tracing::error!(error = %e, "crawl loop task panicked"),
        }
        // The timer tasks return `()`; join them so they stop cleanly.
        let _ = sampler_task.await;
        let _ = progress_task.await;
        let _ = staleness_task.await;
        let _ = reclaim_task.await;
    };

    match tokio::time::timeout(SHUTDOWN_TIMEOUT, drain).await {
        Ok(()) => tracing::info!("graceful shutdown complete"),
        Err(_) => tracing::warn!(
            timeout_secs = SHUTDOWN_TIMEOUT.as_secs(),
            "shutdown drain exceeded its budget — exiting anyway (a task was stuck)"
        ),
    }

    Ok(())
}
