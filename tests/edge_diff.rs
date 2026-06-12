//! Edge-diff writer integration tests (D-15, D-08, GRAPH-02 idempotency).
//!
//! Each test spins an ephemeral Postgres via the shared [`common::start_postgres`]
//! fixture, runs the Phase 1 migration, then exercises the store write API
//! against the real schema. Requires a running Docker daemon.

mod common;

use chrono::Utc;
use web_of_trust::store::{self, follows::apply_follow_list, pubkeys::upsert_pubkey};

/// Deterministic 32-byte pubkey from a single seed byte.
fn pk(seed: u8) -> [u8; 32] {
    [seed; 32]
}

/// Count edges currently stored for a follower.
async fn edge_count(pool: &sqlx::PgPool, follower_id: i64) -> i64 {
    sqlx::query_scalar!(
        "SELECT COUNT(*) FROM follows WHERE follower_id = $1",
        follower_id
    )
    .fetch_one(pool)
    .await
    .unwrap()
    .unwrap_or(0)
}

/// upsert_pubkey is get-or-create: same key -> same id, no duplicate row.
/// (Also satisfies Task 1's `--test edge_diff upsert` verify hook.)
#[tokio::test]
async fn upsert_pubkey_is_idempotent() -> anyhow::Result<()> {
    let (_pg, url) = common::start_postgres().await?;
    let pool = store::connect(&url).await?;
    store::run_migrations(&pool).await?;

    let key = pk(1);
    let id1 = upsert_pubkey(&pool, &key).await?;
    let id2 = upsert_pubkey(&pool, &key).await?;
    assert_eq!(id1, id2, "same pubkey must return the same surrogate id");

    let rows: i64 = sqlx::query_scalar!("SELECT COUNT(*) FROM pubkeys WHERE pubkey = $1", &key[..])
        .fetch_one(&pool)
        .await?
        .unwrap_or(0);
    assert_eq!(rows, 1, "upserting the same key twice must not duplicate the row");

    // Non-32-byte input is rejected at the boundary (V5).
    let bad = upsert_pubkey(&pool, &[0u8; 16]).await;
    assert!(matches!(bad, Err(web_of_trust::StoreError::InvalidPubkey(16))));

    Ok(())
}

/// Applying a list inserts exactly the new edges; a second, changed list
/// inserts added and deletes removed, leaving follows equal to the new set.
#[tokio::test]
async fn edge_diff_writer() -> anyhow::Result<()> {
    let (_pg, url) = common::start_postgres().await?;
    let pool = store::connect(&url).await?;
    store::run_migrations(&pool).await?;

    let follower = upsert_pubkey(&pool, &pk(10)).await?;
    let a = upsert_pubkey(&pool, &pk(11)).await?;
    let b = upsert_pubkey(&pool, &pk(12)).await?;
    let c = upsert_pubkey(&pool, &pk(13)).await?;

    // First apply: follow {a, b}.
    assert_eq!(edge_count(&pool, follower).await, 0);
    let changed = apply_follow_list(&pool, follower, &pk(100), Utc::now(), &[a, b]).await?;
    assert!(changed, "first apply must report a change");
    assert_eq!(edge_count(&pool, follower).await, 2);

    // Second apply (NEW event id): follow {b, c} — add c, remove a, keep b.
    let changed = apply_follow_list(&pool, follower, &pk(101), Utc::now(), &[b, c]).await?;
    assert!(changed, "changed list must report a change");
    assert_eq!(edge_count(&pool, follower).await, 2);

    let mut followees: Vec<i64> = sqlx::query_scalar!(
        "SELECT followee_id FROM follows WHERE follower_id = $1 ORDER BY followee_id",
        follower
    )
    .fetch_all(&pool)
    .await?;
    followees.sort_unstable();
    let mut expected = vec![b, c];
    expected.sort_unstable();
    assert_eq!(followees, expected, "follows must equal the new set after the diff");

    // change_count incremented twice (two genuine changes).
    let change_count: i64 =
        sqlx::query_scalar!("SELECT change_count FROM pubkeys WHERE id = $1", follower)
            .fetch_one(&pool)
            .await?;
    assert_eq!(change_count, 2);

    Ok(())
}

/// Re-applying with the SAME applied_event_id touches zero edge rows but bumps
/// fetch_count / last_confirmed_at (GRAPH-02 idempotency).
#[tokio::test]
async fn same_event_id_zero_touch() -> anyhow::Result<()> {
    let (_pg, url) = common::start_postgres().await?;
    let pool = store::connect(&url).await?;
    store::run_migrations(&pool).await?;

    let follower = upsert_pubkey(&pool, &pk(20)).await?;
    let a = upsert_pubkey(&pool, &pk(21)).await?;
    let b = upsert_pubkey(&pool, &pk(22)).await?;

    let event = pk(200);
    assert!(apply_follow_list(&pool, follower, &event, Utc::now(), &[a, b]).await?);
    assert_eq!(edge_count(&pool, follower).await, 2);

    let (fetch_before, change_before): (i64, i64) =
        sqlx::query!("SELECT fetch_count, change_count FROM pubkeys WHERE id = $1", follower)
            .fetch_one(&pool)
            .await
            .map(|r| (r.fetch_count, r.change_count))?;

    // Re-apply with the SAME event id.
    let changed = apply_follow_list(&pool, follower, &event, Utc::now(), &[a, b]).await?;
    assert!(!changed, "unchanged event id must report no change");
    assert_eq!(edge_count(&pool, follower).await, 2, "zero edge rows touched");

    let (fetch_after, change_after): (i64, i64) =
        sqlx::query!("SELECT fetch_count, change_count FROM pubkeys WHERE id = $1", follower)
            .fetch_one(&pool)
            .await
            .map(|r| (r.fetch_count, r.change_count))?;

    assert_eq!(fetch_after, fetch_before + 1, "fetch_count must still bump on re-confirm");
    assert_eq!(change_after, change_before, "change_count must NOT bump on zero-touch");

    let confirmed: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar!("SELECT last_confirmed_at FROM pubkeys WHERE id = $1", follower)
            .fetch_one(&pool)
            .await?;
    assert!(confirmed.is_some(), "last_confirmed_at must be stamped on re-confirm");

    Ok(())
}

/// A follow list containing the follower's own pubkey produces no self-edge (D-08).
#[tokio::test]
async fn self_follow_dropped() -> anyhow::Result<()> {
    let (_pg, url) = common::start_postgres().await?;
    let pool = store::connect(&url).await?;
    store::run_migrations(&pool).await?;

    let follower = upsert_pubkey(&pool, &pk(30)).await?;
    let a = upsert_pubkey(&pool, &pk(31)).await?;

    // Include the follower's own id in the followee list.
    let changed = apply_follow_list(&pool, follower, &pk(33), Utc::now(), &[follower, a]).await?;
    assert!(changed);

    // Only the real edge survives; the self-edge was filtered before the diff.
    assert_eq!(edge_count(&pool, follower).await, 1);
    let self_edges: i64 = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM follows WHERE follower_id = $1 AND followee_id = $1",
        follower
    )
    .fetch_one(&pool)
    .await?
    .unwrap_or(0);
    assert_eq!(self_edges, 0, "no self-follow edge may be created (D-08)");

    Ok(())
}
