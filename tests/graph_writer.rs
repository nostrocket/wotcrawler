//! GRAPH-02 verification scaffold (Wave 0).
//!
//! These tests prove the edge-diff writer applied through the *wired*
//! `apply_validated` seam (real `ValidatedFollowList` values, not synthetic id
//! arrays): a replacing kind-3 inserts only added + deletes only removed edges
//! in one transaction, re-applying the same event id touches zero edge rows, and
//! concurrent applies for one follower converge on the newest-wins resolution.
//!
//! This file is a Wave 0 scaffold: the test names below are the contract that
//! plan 03-03 must satisfy. Bodies land in 03-03 — the value of this file today
//! is that the names exist and the suite compiles. Each stub is `#[ignore]` so
//! `cargo test` reports them as deliberately-skipped rather than empty passes.
//!
//! Imports are limited to surfaces that exist today (`store`, `apply_follow_list`,
//! `upsert_pubkey`); the `web_of_trust::crawl` module lands in 03-02 and is
//! intentionally NOT referenced here so the scaffold compiles now.
//!
//! Requires a running Docker daemon (testcontainers Postgres) when un-ignored.

mod common;

#[allow(unused_imports)]
use web_of_trust::store::{self, follows::apply_follow_list, pubkeys::upsert_pubkey};

/// Deterministic 32-byte pubkey from a single seed byte (mirrors edge_diff::pk).
#[allow(dead_code)]
fn pk(seed: u8) -> [u8; 32] {
    [seed; 32]
}

/// GRAPH-02: applying a real `ValidatedFollowList` through the `apply_validated`
/// seam inserts only the added edges and deletes only the removed edges in one
/// transaction, leaving `follows` equal to the new set.
#[tokio::test]
#[ignore] // Wave 0 scaffold — body lands in 03-03
async fn apply_diff_adds_and_removes() -> anyhow::Result<()> {
    Ok(())
}

/// GRAPH-02: re-applying the SAME validated event id touches zero edge rows
/// (fetch_count bumps, change_count does not) — idempotency through the seam.
#[tokio::test]
#[ignore] // Wave 0 scaffold — body lands in 03-03
async fn same_event_zero_touch() -> anyhow::Result<()> {
    Ok(())
}

/// GRAPH-02 / INGEST-03 boundary: two concurrent applies for one follower
/// (older + newer event) converge on the newest-wins resolved edge set under
/// MVCC, with the idempotency short-circuit preventing redundant writes.
#[tokio::test]
#[ignore] // Wave 0 scaffold — body lands in 03-03
async fn newest_wins_under_concurrent_apply() -> anyhow::Result<()> {
    Ok(())
}
