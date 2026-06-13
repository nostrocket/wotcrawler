//! BFS frontier verification scaffold (Wave 0).
//!
//! These tests prove the DB-resident, crash-safe BFS frontier built on
//! `pubkeys.status`: reachability-gated discovery (CRAWL-01/02), crash-resume
//! via the startup reclaim of orphaned `in_progress` leases (CRAWL-03), bounded
//! concurrency with `FOR UPDATE SKIP LOCKED` no-double-claim (CRAWL-04), and
//! terminal-status `last_fetched_at` stamping (FRESH-01).
//!
//! This file is a Wave 0 scaffold: the test names below map 1:1 to the
//! RESEARCH Test Map and are the contract plan 03-03 must satisfy. Bodies land
//! in 03-03 — the value today is that the names exist and the suite compiles.
//! Each stub is `#[ignore]` so `cargo test` reports them as deliberately
//! skipped rather than empty passes.
//!
//! Imports are limited to surfaces that exist today (`store`, `apply_follow_list`,
//! `upsert_pubkey`); the `web_of_trust::crawl` module (claim/sweep/seed + worker
//! loop) lands in 03-02 and is intentionally NOT referenced here so the scaffold
//! compiles now.
//!
//! Requires a running Docker daemon (testcontainers Postgres) when un-ignored.

mod common;

#[allow(unused_imports)]
use web_of_trust::store::{self, follows::apply_follow_list, pubkeys::upsert_pubkey};

/// Deterministic 32-byte pubkey from a single seed (mirrors concurrency::pk).
#[allow(dead_code)]
fn pk(seed: u16) -> [u8; 32] {
    let mut k = [0u8; 32];
    k[0] = (seed & 0xff) as u8;
    k[1] = (seed >> 8) as u8;
    k
}

/// CRAWL-01: a crawl from a configured anchor discovers every reachable pubkey
/// via BFS — the anchor and all multi-hop followees end `fetched`.
#[tokio::test]
#[ignore] // Wave 0 scaffold — body lands in 03-03
async fn bfs_reaches_full_component() -> anyhow::Result<()> {
    Ok(())
}

/// CRAWL-02: an isolated spam-island pubkey that nobody reachable follows is
/// never inserted, claimed, or fetched (structural reachability gate).
#[tokio::test]
#[ignore] // Wave 0 scaffold — body lands in 03-03
async fn spam_island_never_crawled() -> anyhow::Result<()> {
    Ok(())
}

/// CRAWL-03: after a crash (orphaned `in_progress` rows) and restart, a row that
/// was already `fetched` before the crash is never re-fetched on the second pass.
#[tokio::test]
#[ignore] // Wave 0 scaffold — body lands in 03-03
async fn crash_resume_no_redo() -> anyhow::Result<()> {
    Ok(())
}

/// CRAWL-03: the startup reclaim resets orphaned `in_progress` leases back to
/// `discovered` (clearing `claimed_at`) so a crashed-mid-fetch pubkey is retried.
#[tokio::test]
#[ignore] // Wave 0 scaffold — body lands in 03-03
async fn startup_reclaims_in_progress() -> anyhow::Result<()> {
    Ok(())
}

/// CRAWL-04: with concurrency cap K, at most K batches are ever in flight.
#[tokio::test]
#[ignore] // Wave 0 scaffold — body lands in 03-03
async fn bounded_concurrency() -> anyhow::Result<()> {
    Ok(())
}

/// CRAWL-04: two concurrent workers never claim the same row
/// (`FOR UPDATE SKIP LOCKED`), and neither blocks the other.
#[tokio::test]
#[ignore] // Wave 0 scaffold — body lands in 03-03
async fn skip_locked_no_double_claim() -> anyhow::Result<()> {
    Ok(())
}

/// FRESH-01: every terminal transition (`fetched`, `not_found`, `failed`) stamps
/// `last_fetched_at`.
#[tokio::test]
#[ignore] // Wave 0 scaffold — body lands in 03-03
async fn last_fetched_at_stamped_on_terminal() -> anyhow::Result<()> {
    Ok(())
}
