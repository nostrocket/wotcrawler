//! DB-resident BFS frontier primitives: seed, claim (lease), reclaim, requeue.
//!
//! These are the only custom queue logic in the crawler (D-01) — the frontier
//! IS `pubkeys.status`. The hard primitives are reused verbatim:
//! [`crate::store::pubkeys::upsert_pubkey`] is the seed/enqueue mechanism and
//! [`crate::store::pubkeys::set_fetch_status`] is the timestamp-stamping idiom
//! every terminal transition routes through (FRESH-01).
//!
//! The claim uses the canonical Postgres job-queue primitive
//! `SELECT ... FOR UPDATE SKIP LOCKED` in its OWN short transaction (D-04): the
//! row lock releases at commit, NOT during the multi-second relay fetch that runs
//! afterward (RESEARCH Pitfall 6). Two concurrent workers therefore never claim
//! the same row and never block each other (T-03-04).

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::error::StoreError;
use crate::store::pubkeys::upsert_pubkey;

/// A pubkey claimed by a worker: its surrogate id plus the 32-byte key to fetch.
///
/// `pubkey` comes back from the claim CTE as `Vec<u8>` (bytea) — convert it to a
/// nostr `PublicKey` at the fetch boundary, mirroring how
/// [`crate::store::follows::apply_follow_list`] reads bytea ids as `Vec<u8>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedAuthor {
    /// Stable surrogate `bigint` id for the pubkey (the queue key).
    pub id: i64,
    /// The 32-byte nostr x-only pubkey (bytea), to fetch from relays.
    pub pubkey: Vec<u8>,
}

/// Seed the anchor pubkey into the frontier as the only externally-inserted row
/// (D-03, CRAWL-01).
///
/// This is a verbatim [`upsert_pubkey`] call: the upsert lands the anchor as
/// `discovered` (or returns its existing id if already seen), making it the
/// single root the BFS expands from. No new SQL — discovery of everyone else
/// happens purely by applying fetched follow lists.
pub async fn seed_anchor(pool: &PgPool, anchor_pubkey: &[u8]) -> Result<i64, StoreError> {
    upsert_pubkey(pool, anchor_pubkey).await
}

/// Atomically claim up to `limit` `discovered` pubkeys, leasing them to this
/// worker by flipping them to `in_progress` and stamping `claimed_at` (D-04/D-07).
///
/// Runs the claim CTE inside its OWN short transaction so the `FOR UPDATE` row
/// lock is released at commit — NOT held during the subsequent multi-second relay
/// fetch (RESEARCH Pitfall 6). `FOR UPDATE SKIP LOCKED` means two concurrent
/// claims never return the same id and neither blocks the other (T-03-04).
///
/// The claim selects ONLY `status = 'discovered'`: a `fetched`/`not_found`/
/// `failed` row is never re-claimed, which is the core of CRAWL-03's no-redo
/// guarantee (RESEARCH Pitfall 3). Returns the claimed authors (possibly empty
/// when the frontier is drained, which the worker loop uses as its termination
/// signal).
pub async fn claim_batch(pool: &PgPool, limit: i64) -> Result<Vec<ClaimedAuthor>, StoreError> {
    let mut tx = pool.begin().await?;

    let rows = sqlx::query!(
        "WITH claimed AS ( \
             SELECT id FROM pubkeys \
             WHERE status = 'discovered' \
             ORDER BY id \
             LIMIT $1 \
             FOR UPDATE SKIP LOCKED \
         ) \
         UPDATE pubkeys p \
         SET status = 'in_progress', claimed_at = now() \
         FROM claimed \
         WHERE p.id = claimed.id \
         RETURNING p.id, p.pubkey",
        limit
    )
    .fetch_all(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(rows
        .into_iter()
        .map(|r| ClaimedAuthor {
            id: r.id,
            pubkey: r.pubkey,
        })
        .collect())
}

/// Reset every orphaned `in_progress` lease back to `discovered` at startup
/// (D-06, CRAWL-03). Returns the number of rows reset.
///
/// A clean shutdown leaves zero `in_progress` rows; whatever remains at startup
/// is a crash orphan (a worker claimed a batch then the process died before
/// finishing it), so no age threshold is needed for the Phase 3 startup case. The
/// claimed-but-unfinished work becomes claimable again; re-fetching it is
/// harmless because [`crate::store::follows::apply_follow_list`] is idempotent on
/// an unchanged event id (D-05). Continuous in-run reclaim is deferred to Phase 4.
pub async fn reclaim_stale_on_startup(pool: &PgPool) -> Result<u64, StoreError> {
    let result = sqlx::query!(
        // Reset `fetch_attempts` too: a crash-orphaned row must not consume its
        // retry budget. The counter is a *relay-failure* count; a process crash is
        // not a relay failure, so a row merely in-flight at crash time would
        // otherwise be one error away from a spurious terminal `failed` (WR-02).
        // Re-fetching is harmless — apply_follow_list is idempotent (D-05).
        "UPDATE pubkeys SET status = 'discovered', claimed_at = NULL, fetch_attempts = 0 \
         WHERE status = 'in_progress'"
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

/// Resolve a transient fetch error: bump `fetch_attempts` and either requeue the
/// pubkey for retry or mark it terminally `failed` (D-09/D-11, FRESH-01).
///
/// If the bumped `fetch_attempts` is still `< max_attempts`, the pubkey returns
/// to `discovered` (claimable again) and `claimed_at` is cleared. Once it reaches
/// `max_attempts` the pubkey transitions to the terminal `failed` state AND
/// stamps `last_fetched_at` to `now` (FRESH-01 — the Phase 4 staleness loop reads
/// that timestamp; a terminal status without it would break the age comparison,
/// RESEARCH Pitfall 5). The cap guarantees a flaky pubkey cannot bounce
/// `discovered <-> in_progress` forever (RESEARCH Pitfall 7).
///
/// `now` is the timestamp of the fetch attempt that failed (passed explicitly so
/// the caller controls the clock, mirroring
/// [`crate::store::pubkeys::set_fetch_status`]).
pub async fn requeue_or_fail(
    pool: &PgPool,
    id: i64,
    max_attempts: i16,
    now: DateTime<Utc>,
) -> Result<(), StoreError> {
    // Single atomic UPDATE: bump the counter, then branch on the BUMPED value.
    // `failed` routes through last_fetched_at so FRESH-01 holds; a requeue leaves
    // last_fetched_at untouched (the knowledge was not refreshed — only retried).
    // Either way the lease is released (`claimed_at = NULL`): both the requeue
    // (`discovered`) and the terminal (`failed`) row are no longer leased to any
    // worker, so a non-NULL claimed_at on either would be semantically wrong and
    // would poison `claimed_at IS NOT NULL` in-flight monitoring (WR-01).
    sqlx::query!(
        // `$2` is cast to int2 so the i16 bind matches the SMALLINT column domain
        // (fetch_attempts + 1 alone promotes to int4, which would force an i32 bind).
        "UPDATE pubkeys \
         SET fetch_attempts = fetch_attempts + 1, \
             status = CASE \
                 WHEN fetch_attempts + 1 >= $2::int2 THEN 'failed' \
                 ELSE 'discovered' \
             END, \
             last_fetched_at = CASE \
                 WHEN fetch_attempts + 1 >= $2::int2 THEN $3 \
                 ELSE last_fetched_at \
             END, \
             claimed_at = NULL \
         WHERE id = $1",
        id,
        max_attempts,
        now
    )
    .execute(pool)
    .await?;

    Ok(())
}
