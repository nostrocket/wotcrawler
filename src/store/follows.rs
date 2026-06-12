//! Transactional edge-diff writer (D-15).
//!
//! [`apply_follow_list`] applies a replacing kind-3 follow list as the diff
//! against the current edges: DELETE removed edges + INSERT added edges, plus an
//! atomic freshness/churn update, all in ONE transaction (RESEARCH Pattern 3,
//! Pitfall 4 — a crash mid-diff must never leave a half-applied follow list).
//!
//! The writer is idempotent on an unchanged event id: re-applying the same
//! `applied_event_id` touches ZERO edge rows and only bumps the confirm
//! counters (GRAPH-02 idempotency property).

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::error::StoreError;

/// Apply a replacing kind-3 follow list for `follower_id`.
///
/// `followee_ids` are the surrogate ids of the followed pubkeys; the caller
/// resolves followee pubkeys to ids via
/// [`crate::store::pubkeys::upsert_pubkey`] *before* calling this. The signature
/// deliberately accepts only ids — kind-3 p-tag relay hints and petnames are
/// discarded at the ingest boundary and never reach the store layer (D-06).
///
/// Behavior:
/// 1. If `event_id` equals the follower's current `applied_event_id`, this is a
///    re-confirmation of the already-applied list: bump `fetch_count` and
///    `last_confirmed_at`, touch zero edge rows, and return `Ok(false)`
///    (GRAPH-02 idempotency).
/// 2. Self-follows are dropped — any followee id equal to `follower_id` is
///    filtered out before the diff (D-08).
/// 3. The diff (`added = new - current`, `removed = current - new`) is computed
///    in Rust, then DELETE-removed + INSERT-added + the freshness/churn UPDATE
///    run inside a single `pool.begin()` / `tx.commit()` transaction (Pitfall 4).
///
/// Returns `Ok(true)` if the edge set or applied event changed, `Ok(false)` on
/// the unchanged-event-id short circuit.
pub async fn apply_follow_list(
    pool: &PgPool,
    follower_id: i64,
    event_id: &[u8],
    created_at: DateTime<Utc>,
    followee_ids: &[i64],
) -> Result<bool, StoreError> {
    // (1) Idempotency short circuit: same applied event id -> zero edge rows.
    let current_event: Option<Vec<u8>> = sqlx::query_scalar!(
        "SELECT applied_event_id FROM pubkeys WHERE id = $1",
        follower_id
    )
    .fetch_one(pool)
    .await?;

    if current_event.as_deref() == Some(event_id) {
        // Re-confirmation of the already-applied list: bump confirm counters,
        // touch no edges (GRAPH-02). Status is set to 'fetched' since a fetch
        // confirmed the current list is still current.
        sqlx::query!(
            "UPDATE pubkeys \
             SET status = 'fetched', \
                 last_fetched_at = now(), \
                 last_confirmed_at = now(), \
                 fetch_count = fetch_count + 1 \
             WHERE id = $1",
            follower_id
        )
        .execute(pool)
        .await?;
        return Ok(false);
    }

    // (2) Drop self-follows (D-08) and dedup the incoming set.
    let new_set: HashSet<i64> = followee_ids
        .iter()
        .copied()
        .filter(|&id| id != follower_id)
        .collect();

    // (3) Read current edges and compute the diff in Rust.
    let current_rows = sqlx::query_scalar!(
        "SELECT followee_id FROM follows WHERE follower_id = $1",
        follower_id
    )
    .fetch_all(pool)
    .await?;
    let current_set: HashSet<i64> = current_rows.into_iter().collect();

    let added: Vec<i64> = new_set.difference(&current_set).copied().collect();
    let removed: Vec<i64> = current_set.difference(&new_set).copied().collect();
    let changed = !added.is_empty() || !removed.is_empty();

    // (4) Apply the whole diff + freshness/churn update atomically (Pitfall 4).
    let mut tx = pool.begin().await?;

    for &followee_id in &removed {
        sqlx::query!(
            "DELETE FROM follows WHERE follower_id = $1 AND followee_id = $2",
            follower_id,
            followee_id
        )
        .execute(&mut *tx)
        .await?;
    }

    for &followee_id in &added {
        sqlx::query!(
            "INSERT INTO follows (follower_id, followee_id) VALUES ($1, $2) \
             ON CONFLICT DO NOTHING",
            follower_id,
            followee_id
        )
        .execute(&mut *tx)
        .await?;
    }

    // Freshness + churn (D-09/D-10): always bump fetch counters and record the
    // newly applied event; only bump the change counters when the edge set
    // actually changed.
    sqlx::query!(
        "UPDATE pubkeys \
         SET status = 'fetched', \
             applied_event_id = $2, \
             applied_created_at = $3, \
             last_fetched_at = now(), \
             last_confirmed_at = now(), \
             fetch_count = fetch_count + 1, \
             last_changed_at = CASE WHEN $4 THEN now() ELSE last_changed_at END, \
             change_count = change_count + CASE WHEN $4 THEN 1 ELSE 0 END \
         WHERE id = $1",
        follower_id,
        event_id,
        created_at,
        changed
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(true)
}
