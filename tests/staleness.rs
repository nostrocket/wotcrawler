//! Staleness + in-run reclaim sweep verification (FRESH-02 / OPS-02).
//!
//! Proves the two Phase 4 frontier sweeps over a real Postgres:
//! - [`reclaim_stale_by_ttl`]: re-enqueues `fetched`/`not_found`/`failed` rows
//!   whose `last_fetched_at` is older than the TTL back into `discovered`,
//!   resetting `claimed_at`/`fetch_attempts`, and leaves fresh rows untouched.
//! - [`reclaim_in_progress_older_than`]: resets only `in_progress` leases older
//!   than a threshold, never freshly-claimed live rows (T-04-02).
//!
//! Requires a running Docker daemon (testcontainers Postgres). KNOWN FLAKE:
//! testcontainers container-creation race — run with `-- --test-threads=2` and
//! re-run once on a container/port timeout.

mod common;

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use web_of_trust::crawl::frontier::{reclaim_in_progress_older_than, reclaim_stale_by_ttl};

use common::{fresh_db, pk, status_of};

/// Insert a pubkey row directly with full control over its frontier bookkeeping,
/// returning the surrogate id. Seeds `status`, `last_fetched_at`, `fetch_attempts`,
/// and `claimed_at` so each test can construct the exact frontier state it asserts
/// on (analogous to the seed shape in `tests/frontier.rs::startup_reclaims_in_progress`).
async fn seed_row(
    pool: &PgPool,
    pubkey: &[u8],
    status: &str,
    last_fetched_at: Option<DateTime<Utc>>,
    fetch_attempts: i16,
    claimed_at: Option<DateTime<Utc>>,
) -> anyhow::Result<i64> {
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO pubkeys (pubkey, status, last_fetched_at, fetch_attempts, claimed_at) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(pubkey)
    .bind(status)
    .bind(last_fetched_at)
    .bind(fetch_attempts)
    .bind(claimed_at)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// Read a row's `fetch_attempts`.
async fn attempts_of(pool: &PgPool, id: i64) -> anyhow::Result<i16> {
    Ok(
        sqlx::query_scalar::<_, i16>("SELECT fetch_attempts FROM pubkeys WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await?,
    )
}

/// Read a row's `claimed_at`.
async fn claimed_at_of(pool: &PgPool, id: i64) -> anyhow::Result<Option<DateTime<Utc>>> {
    Ok(
        sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "SELECT claimed_at FROM pubkeys WHERE id = $1",
        )
        .bind(id)
        .fetch_one(pool)
        .await?,
    )
}

/// FRESH-02: `reclaim_stale_by_ttl(24h)` flips ONLY the rows whose
/// `last_fetched_at` is past the TTL back to `discovered`; the fresh row stays
/// `fetched`. Returns the count of re-enqueued rows (2).
#[tokio::test]
async fn reenqueues_only_stale() -> anyhow::Result<()> {
    let (_pg, pool) = fresh_db().await?;
    let now = Utc::now();
    let hours = chrono::Duration::hours;

    let fresh = seed_row(&pool, &pk(1), "fetched", Some(now - hours(1)), 0, None).await?;
    let stale_48 = seed_row(&pool, &pk(2), "fetched", Some(now - hours(48)), 0, None).await?;
    let stale_72 = seed_row(&pool, &pk(3), "fetched", Some(now - hours(72)), 0, None).await?;

    let count = reclaim_stale_by_ttl(&pool, 24 * 3600).await?;
    assert_eq!(count, 2, "exactly the two past-24h rows must be re-enqueued");

    assert_eq!(status_of(&pool, fresh).await?, "fetched", "fresh row untouched");
    assert_eq!(status_of(&pool, stale_48).await?, "discovered");
    assert_eq!(status_of(&pool, stale_72).await?, "discovered");

    Ok(())
}

/// FRESH-02: a stale row carrying prior `fetch_attempts`/`claimed_at` comes back
/// from the sweep with `fetch_attempts = 0` and `claimed_at IS NULL` — a re-fetch
/// cycle must not inherit prior relay-failure retry counts (load-bearing reset).
#[tokio::test]
async fn resets_attempts() -> anyhow::Result<()> {
    let (_pg, pool) = fresh_db().await?;
    let now = Utc::now();

    let stale = seed_row(
        &pool,
        &pk(7),
        "fetched",
        Some(now - chrono::Duration::hours(48)),
        2,
        Some(now),
    )
    .await?;

    let count = reclaim_stale_by_ttl(&pool, 24 * 3600).await?;
    assert_eq!(count, 1);

    assert_eq!(status_of(&pool, stale).await?, "discovered");
    assert_eq!(attempts_of(&pool, stale).await?, 0, "fetch_attempts reset to 0");
    assert!(
        claimed_at_of(&pool, stale).await?.is_none(),
        "claimed_at cleared on re-enqueue"
    );

    Ok(())
}

/// FRESH-02: the scanner covers all THREE terminal statuses — `not_found` and
/// `failed` rows past the TTL are re-enqueued to `discovered` too, not just
/// `fetched`.
#[tokio::test]
async fn not_found_and_failed_also_reenqueued() -> anyhow::Result<()> {
    let (_pg, pool) = fresh_db().await?;
    let now = Utc::now();
    let stale = Some(now - chrono::Duration::hours(48));

    let nf = seed_row(&pool, &pk(11), "not_found", stale, 0, None).await?;
    let failed = seed_row(&pool, &pk(12), "failed", stale, 3, None).await?;

    let count = reclaim_stale_by_ttl(&pool, 24 * 3600).await?;
    assert_eq!(count, 2, "both not_found and failed are re-enqueued");

    assert_eq!(status_of(&pool, nf).await?, "discovered");
    assert_eq!(status_of(&pool, failed).await?, "discovered");
    assert_eq!(attempts_of(&pool, failed).await?, 0, "failed row's attempts reset");

    Ok(())
}

/// OPS-02 / T-04-02: `reclaim_in_progress_older_than` resets only `in_progress`
/// leases whose `claimed_at` is older than the age threshold; a freshly-claimed
/// live lease (claimed_at = now()) is left untouched mid-fetch.
#[tokio::test]
async fn in_progress_only_reclaims_old() -> anyhow::Result<()> {
    let (_pg, pool) = fresh_db().await?;
    let now = Utc::now();
    let age_secs: i64 = 300; // 5 minutes

    // Claimed age_secs+60s ago -> orphaned, must be reclaimed.
    let old = seed_row(
        &pool,
        &pk(21),
        "in_progress",
        None,
        1,
        Some(now - chrono::Duration::seconds(age_secs + 60)),
    )
    .await?;
    // Claimed just now -> live lease, must be left untouched.
    let fresh = seed_row(&pool, &pk(22), "in_progress", None, 0, Some(now)).await?;

    let count = reclaim_in_progress_older_than(&pool, age_secs).await?;
    assert_eq!(count, 1, "only the old lease is reclaimed");

    assert_eq!(status_of(&pool, old).await?, "discovered", "old lease reset");
    assert_eq!(attempts_of(&pool, old).await?, 0, "old lease attempts reset");
    assert!(claimed_at_of(&pool, old).await?.is_none(), "old lease released");

    assert_eq!(
        status_of(&pool, fresh).await?,
        "in_progress",
        "fresh live lease must NOT be reclaimed mid-fetch"
    );
    assert!(
        claimed_at_of(&pool, fresh).await?.is_some(),
        "fresh lease keeps its claimed_at"
    );

    Ok(())
}
