//! Wave 0 scaffold for the NIP-65 outbox-fallback integration tests
//! (RELAY-05/06).
//!
//! The relay-URL-aware + error-injecting `ScriptedGraph` seam and the
//! `relay_list_event` fixture (plan 05-01) exist so these tests can model
//! "author absent on curated relay A, recovered via write relay B". The bodies
//! are intentionally deferred to 05-03 (fallback recovery/miss) and 05-04
//! (deadlock-safe per-relay concurrency), which build the `process_batch`
//! `fallback_fetch` seam + the per-relay `Semaphore`. These functions are
//! `#[ignore]`d so `cargo test` compiles them (catching seam drift) without
//! running an unimplemented path.
//!
//! Requires a running Docker daemon when un-ignored. Run with
//! `-- --test-threads=2`; re-run once on a testcontainers container/port flake.

mod common;

/// RELAY-05: a kind:3 author absent on the curated relays is recovered by
/// fetching from its NIP-65 write relay; `nip65_recovered_total` increments.
/// Body lands in 05-03.
#[tokio::test]
#[ignore = "Wave 0 scaffold; body lands in 05-03/05-04"]
async fn fallback_recovers_via_write_relay() {
    unimplemented!("05-03: fallback recovery via write relay");
}

/// RELAY-05: an author that misses on BOTH the curated relays and its write
/// relays is stamped terminal `not_found`. Body lands in 05-03.
#[tokio::test]
#[ignore = "Wave 0 scaffold; body lands in 05-03/05-04"]
async fn fallback_miss_stamps_not_found() {
    unimplemented!("05-03: fallback miss stamps not_found");
}

/// RELAY-06: the global -> per-relay -> GCRA acquisition order is deadlock-free
/// even at `per_relay_concurrency = 1`. Body lands in 05-04.
#[tokio::test]
#[ignore = "Wave 0 scaffold; body lands in 05-03/05-04"]
async fn no_deadlock_single_permit() {
    unimplemented!("05-04: deadlock-free fan-out at per_relay_concurrency=1");
}
