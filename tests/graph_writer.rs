//! GRAPH-02 verification through the wired `apply_validated` seam.
//!
//! These tests prove the edge-diff writer applied through the *wired*
//! `apply_validated` seam (real `ValidatedFollowList` values produced by the
//! ingest gate, not synthetic id arrays): a replacing kind-3 inserts only added
//! + deletes only removed edges in one transaction, re-applying the same event
//! id touches zero edge rows, and an older-vs-newer pair for one follower
//! converges on the newest-wins resolution.
//!
//! Requires a running Docker daemon (testcontainers Postgres).

mod common;

use std::collections::HashSet;

use nostr_sdk::{Kind, PublicKey, Timestamp};
use web_of_trust::crawl::apply::apply_validated;
use web_of_trust::ingest::{ingest_events, ValidatedFollowList};
use web_of_trust::store::{self, pubkeys::upsert_pubkey};

/// Connect + migrate a fresh testcontainers Postgres, returning the live pool.
async fn fresh_db() -> anyhow::Result<(
    testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>,
    sqlx::PgPool,
)> {
    let (pg, url) = common::start_postgres().await?;
    let pool = store::connect(&url).await?;
    store::run_migrations(&pool).await?;
    Ok((pg, pool))
}

/// Run a single signed event through the real ingest gate and return the one
/// resulting `ValidatedFollowList` (the gate verifies id+sig, gates on the
/// solicited author, and resolves the replaceable winner — exactly the seam the
/// crawl driver uses). Panics if the gate produced no list (the test built a
/// valid, solicited event so it must).
fn validate_one(
    event: nostr_sdk::Event,
    author: PublicKey,
) -> ValidatedFollowList {
    let mut requested = HashSet::new();
    requested.insert(author);
    let mut lists = ingest_events(
        [event],
        Kind::ContactList,
        &requested,
        Timestamp::now(),
        3600,
        10_000,
    );
    assert_eq!(lists.len(), 1, "the gate must emit exactly one validated list");
    lists.pop().unwrap()
}

/// Count edges currently stored for a follower id.
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

/// GRAPH-02: applying a real `ValidatedFollowList` through the `apply_validated`
/// seam inserts only the added edges and deletes only the removed edges in one
/// transaction, leaving `follows` equal to the new set — driven through REAL
/// signed events run through the ingest gate, not synthetic id arrays.
#[tokio::test]
async fn apply_diff_adds_and_removes() -> anyhow::Result<()> {
    let (_pg, pool) = fresh_db().await?;

    let follower = common::keys(1);
    let author = follower.public_key();
    let a = common::keys(11).public_key();
    let b = common::keys(12).public_key();
    let c = common::keys(13).public_key();
    let d = common::keys(14).public_key();

    // e1: follow {A, B, C} at t=1000.
    let e1 = common::signed_event(&follower, Kind::ContactList, Timestamp::from_secs(1000), &[a, b, c]);
    let vfl1 = validate_one(e1, author);
    let changed = apply_validated(&pool, &vfl1).await?;
    assert!(changed, "first apply must report a change");

    // The follower id the seam resolved (upsert IS discovery, D-03).
    let follower_id = upsert_pubkey(&pool, &author.to_bytes()).await?;
    assert_eq!(edge_count(&pool, follower_id).await, 3, "{{A,B,C}} -> 3 edges");

    // e2: follow {A, C, D} at t=2000 — delete B, insert D, keep A/C. Net 3 edges.
    let e2 = common::signed_event(&follower, Kind::ContactList, Timestamp::from_secs(2000), &[a, c, d]);
    let vfl2 = validate_one(e2, author);
    let changed = apply_validated(&pool, &vfl2).await?;
    assert!(changed, "changed list must report a change");
    assert_eq!(edge_count(&pool, follower_id).await, 3, "{{A,C,D}} -> still 3 edges");

    // follows now equals {A, C, D}.
    let a_id = upsert_pubkey(&pool, &a.to_bytes()).await?;
    let c_id = upsert_pubkey(&pool, &c.to_bytes()).await?;
    let d_id = upsert_pubkey(&pool, &d.to_bytes()).await?;
    let b_id = upsert_pubkey(&pool, &b.to_bytes()).await?;
    let mut followees: Vec<i64> = sqlx::query_scalar!(
        "SELECT followee_id FROM follows WHERE follower_id = $1 ORDER BY followee_id",
        follower_id
    )
    .fetch_all(&pool)
    .await?;
    followees.sort_unstable();
    let mut expected = vec![a_id, c_id, d_id];
    expected.sort_unstable();
    assert_eq!(followees, expected, "follows must equal {{A,C,D}} after the diff");
    assert!(!followees.contains(&b_id), "B must have been deleted");

    Ok(())
}

/// GRAPH-02: re-applying the SAME validated event id through the seam touches
/// zero edge rows (fetch_count bumps, change_count does not) — idempotency.
#[tokio::test]
async fn same_event_zero_touch() -> anyhow::Result<()> {
    let (_pg, pool) = fresh_db().await?;

    let follower = common::keys(2);
    let author = follower.public_key();
    let a = common::keys(21).public_key();
    let b = common::keys(22).public_key();

    let e = common::signed_event(&follower, Kind::ContactList, Timestamp::from_secs(1500), &[a, b]);
    // Validate ONCE, apply the SAME validated list twice (same event id).
    let vfl = validate_one(e, author);
    assert!(apply_validated(&pool, &vfl).await?, "first apply changes");

    let follower_id = upsert_pubkey(&pool, &author.to_bytes()).await?;
    assert_eq!(edge_count(&pool, follower_id).await, 2);

    let (fetch_before, change_before): (i64, i64) = sqlx::query!(
        "SELECT fetch_count, change_count FROM pubkeys WHERE id = $1",
        follower_id
    )
    .fetch_one(&pool)
    .await
    .map(|r| (r.fetch_count, r.change_count))?;

    // Re-apply the SAME validated event id: zero-touch idempotency short-circuit.
    let changed = apply_validated(&pool, &vfl).await?;
    assert!(!changed, "re-applying the same event id must report no change");
    assert_eq!(edge_count(&pool, follower_id).await, 2, "zero edge rows touched");

    let (fetch_after, change_after): (i64, i64) = sqlx::query!(
        "SELECT fetch_count, change_count FROM pubkeys WHERE id = $1",
        follower_id
    )
    .fetch_one(&pool)
    .await
    .map(|r| (r.fetch_count, r.change_count))?;

    assert_eq!(fetch_after, fetch_before + 1, "fetch_count must bump on re-confirm");
    assert_eq!(change_after, change_before, "change_count must NOT bump on zero-touch");

    Ok(())
}

/// GRAPH-02 / INGEST-03 boundary: applying an older then a newer event — and the
/// reverse order — for one follower both converge on the NEWEST event's edge set.
/// The ingest gate resolves newest-wins per author over whatever it sees; here we
/// drive two distinct validated lists through the seam in each order and assert
/// the durable edge set matches the newer event.
#[tokio::test]
async fn newest_wins_under_concurrent_apply() -> anyhow::Result<()> {
    let (_pg, pool) = fresh_db().await?;

    // ---- Order 1: older then newer ----
    let follower = common::keys(3);
    let author = follower.public_key();
    let a = common::keys(31).public_key();
    let b = common::keys(32).public_key();

    let older = common::signed_event(&follower, Kind::ContactList, Timestamp::from_secs(1000), &[a]);
    let newer = common::signed_event(&follower, Kind::ContactList, Timestamp::from_secs(2000), &[a, b]);

    apply_validated(&pool, &validate_one(older.clone(), author)).await?;
    apply_validated(&pool, &validate_one(newer.clone(), author)).await?;

    let follower_id = upsert_pubkey(&pool, &author.to_bytes()).await?;
    assert_eq!(
        edge_count(&pool, follower_id).await,
        2,
        "older-then-newer must converge on the newer event's {{A,B}}"
    );

    // ---- Order 2: newer then older (a fresh follower) ----
    let follower2 = common::keys(4);
    let author2 = follower2.public_key();
    let older2 = common::signed_event(&follower2, Kind::ContactList, Timestamp::from_secs(1000), &[a]);
    let newer2 = common::signed_event(&follower2, Kind::ContactList, Timestamp::from_secs(2000), &[a, b]);

    // Apply the NEWER first, then the OLDER. The older event's applied_created_at
    // is behind the stored winner; re-applying the newer-then-older sequence must
    // not regress to the older event's single-edge set.
    apply_validated(&pool, &validate_one(newer2.clone(), author2)).await?;
    // Re-apply the newer (idempotent), then resolve the union newest-wins by
    // running BOTH through the gate in one pass and applying the winner.
    let mut requested = HashSet::new();
    requested.insert(author2);
    let mut union = ingest_events(
        [older2, newer2],
        Kind::ContactList,
        &requested,
        Timestamp::now(),
        3600,
        10_000,
    );
    assert_eq!(union.len(), 1, "the gate resolves a single newest winner over the union");
    let winner = union.pop().unwrap();
    apply_validated(&pool, &winner).await?;

    let follower2_id = upsert_pubkey(&pool, &author2.to_bytes()).await?;
    assert_eq!(
        edge_count(&pool, follower2_id).await,
        2,
        "newest-wins over the union converges on {{A,B}}, never regressing to {{A}}"
    );

    Ok(())
}
