//! Pubkey identity + freshness-lifecycle store API.
//!
//! `upsert_pubkey` is the get-or-create entry point that turns a 32-byte nostr
//! x-only pubkey into the stable surrogate `bigint` id used everywhere else
//! (D-03). `set_fetch_status` drives the per-pubkey freshness lifecycle
//! (`discovered` -> `fetched` / `not_found` / `failed`, D-09).

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::error::StoreError;

/// Length, in bytes, of a nostr x-only pubkey.
const PUBKEY_LEN: usize = 32;

/// Get-or-create the surrogate id for a 32-byte pubkey.
///
/// Returns the stable `bigint` id for `pubkey`, inserting a new row with status
/// `discovered` if the pubkey is unseen, or returning the existing id if it has
/// been seen before. Calling this twice with the same key returns the SAME id
/// and never creates a duplicate `pubkeys` row.
///
/// The `ON CONFLICT (pubkey) DO UPDATE SET pubkey = EXCLUDED.pubkey` no-op makes
/// `RETURNING id` fire on conflict so the existing id comes back in one round
/// trip (rather than a SELECT-then-INSERT race).
///
/// # Errors
/// Returns [`StoreError::InvalidPubkey`] if `pubkey` is not exactly 32 bytes
/// (V5 input validation, T-03-02 — the store boundary rejects malformed keys).
pub async fn upsert_pubkey(pool: &PgPool, pubkey: &[u8]) -> Result<i64, StoreError> {
    if pubkey.len() != PUBKEY_LEN {
        return Err(StoreError::InvalidPubkey(pubkey.len()));
    }

    let id = sqlx::query_scalar!(
        "INSERT INTO pubkeys (pubkey) VALUES ($1) \
         ON CONFLICT (pubkey) DO UPDATE SET pubkey = EXCLUDED.pubkey \
         RETURNING id",
        pubkey
    )
    .fetch_one(pool)
    .await?;

    Ok(id)
}

/// Transition a pubkey's fetch status and stamp the relevant freshness timestamp.
///
/// `status` is the TEXT representation chosen in the migration (D-09 /
/// RESEARCH Pitfall 5): one of `discovered`, `fetched`, `not_found`, `failed`.
/// `ts` is written to `last_fetched_at` (the time of the fetch attempt that
/// produced this status). The CHECK constraint on `pubkeys.status` rejects any
/// out-of-domain value at the database boundary.
pub async fn set_fetch_status(
    pool: &PgPool,
    id: i64,
    status: &str,
    ts: DateTime<Utc>,
) -> Result<(), StoreError> {
    sqlx::query!(
        "UPDATE pubkeys SET status = $2, last_fetched_at = $3 WHERE id = $1",
        id,
        status,
        ts
    )
    .execute(pool)
    .await?;

    Ok(())
}
