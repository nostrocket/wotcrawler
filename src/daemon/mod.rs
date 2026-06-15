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
//! Submodules (`config`, `observe`, `sampler`, `loop_`) are registered by later
//! Phase 4 plans (04-02..04-05); this is the module root keystone (04-01).
