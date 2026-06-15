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
