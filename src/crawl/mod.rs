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
//! - CRAWL-02: only pubkeys reachable through follows are ever enqueued
//!   (structural — `upsert_pubkey`-on-followee is the only insertion path
//!   besides the anchor seed).
//! - CRAWL-03: the frontier is DB-resident and crash-safe — completed
//!   (`fetched`) work is never re-claimed ([`frontier::claim_batch`] selects only
//!   `discovered`), and orphaned `in_progress` leases are reset at startup
//!   ([`frontier::reclaim_stale_on_startup`]).
//! - CRAWL-04: in-flight concurrency is bounded (the bounded worker loop lands in
//!   03-03; this plan only ships the [`DEFAULT_CONCURRENCY`] knob it consumes).
//! - FRESH-01: every terminal transition stamps `last_fetched_at`
//!   ([`frontier::requeue_or_fail`] routes the terminal `failed` write through a
//!   timestamp-stamping UPDATE).

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
