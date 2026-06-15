//! Transactional NIP-65 relay-list writer (Phase 5, RELAY-05).
//!
//! [`apply_relay_list`] persists a winning kind:10002 (NIP-65) event's
//! advertised relays as a newest-wins FULL REPLACE in one transaction: DELETE
//! the pubkey's prior relay rows, then INSERT the winner's, all in a single
//! `pool.begin()` / `tx.commit()` (a crash mid-replace must never leave a
//! half-applied relay list — same discipline as
//! [`crate::store::follows::apply_follow_list`]).
//!
//! Unlike the kind-3 edge writer, this is a wholesale replace rather than a
//! diff: a pubkey's relay list is a handful of rows, so the GRAPH-02
//! touch-zero-on-unchanged idempotency concern that motivated the follows diff
//! does not apply to this small, non-hot table (RESEARCH Pattern 3 / A2).
//!
//! [`lookup_write_relays`] is the fallback path's read: it returns a pubkey's
//! write relays, where a bare NIP-65 r-tag (stored as the `'both'` marker)
//! counts as a write relay (RESEARCH Pitfall 2 — a bare r-tag advertises both
//! read and write).

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::error::StoreError;

/// Persist a pubkey's NIP-65 advertised relays as a newest-wins full replace.
///
/// `pubkey_id` is the surrogate id resolved by the caller via
/// [`crate::store::pubkeys::upsert_pubkey`]. `relays` are the `(url, marker)`
/// pairs from the winning kind:10002 event (see
/// [`crate::ingest::relay_list::extract_relay_pairs`]); `marker` is one of
/// `"read"`, `"write"`, `"both"`. `seen_at` is the winning event's
/// `created_at` (the `ValidatedRelayList::created_at`).
///
/// The DELETE + per-row INSERT run inside a single transaction so a fresh
/// winning kind:10002 atomically supersedes the pubkey's prior rows. The
/// `ON CONFLICT (pubkey_id, url) DO NOTHING` guards against a relay url
/// duplicated within the same event (e.g. an `r url read` + bare `r url`),
/// keeping the first-seen marker for that url.
pub async fn apply_relay_list(
    pool: &PgPool,
    pubkey_id: i64,
    relays: &[(String, &str)],
    seen_at: DateTime<Utc>,
) -> Result<(), StoreError> {
    let mut tx = pool.begin().await?;

    // Newest-wins: drop the pubkey's prior rows wholesale, then insert the
    // winner's set (RESEARCH Pattern 3).
    sqlx::query!("DELETE FROM pubkey_relays WHERE pubkey_id = $1", pubkey_id)
        .execute(&mut *tx)
        .await?;

    for (url, marker) in relays {
        sqlx::query!(
            "INSERT INTO pubkey_relays (pubkey_id, url, marker, seen_at) \
             VALUES ($1, $2, $3, $4) ON CONFLICT (pubkey_id, url) DO NOTHING",
            pubkey_id,
            url,
            *marker,
            seen_at
        )
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

/// Return a pubkey's NIP-65 write relays for the outbox fallback fetch.
///
/// Selects `marker IN ('write','both')`: a bare NIP-65 r-tag (stored as
/// `'both'`) advertises BOTH read and write, so it MUST be included as a write
/// relay (RESEARCH Pitfall 2 — omitting `'both'` would silently miss every
/// bare-r-tag write relay and the fallback would never fire for those pubkeys).
pub async fn lookup_write_relays(
    pool: &PgPool,
    pubkey_id: i64,
) -> Result<Vec<String>, StoreError> {
    let rows = sqlx::query_scalar!(
        "SELECT url FROM pubkey_relays WHERE pubkey_id = $1 AND marker IN ('write','both')",
        pubkey_id
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
