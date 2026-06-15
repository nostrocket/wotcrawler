//! Integration tests for the Phase 5 NIP-65 relay storage layer (RELAY-05).
//!
//! Proves migration 0004 (`pubkey_relays`) idempotency and the
//! `store::relays::apply_relay_list` / `lookup_write_relays` contract against an
//! ephemeral testcontainers Postgres. All queries are parameterized sqlx
//! queries — never string-formatted SQL.
//!
//! Requires a running Docker daemon. Run with `-- --test-threads=2`; re-run
//! once on a testcontainers container/port flake.

mod common;

use chrono::Utc;
use web_of_trust::store::pubkeys::upsert_pubkey;
use web_of_trust::store::relays::{apply_relay_list, lookup_write_relays};

/// RELAY-05: a fresh database migrates to the 0004 `pubkey_relays` schema, and
/// re-running the migrations is a no-op (applies zero further versions). The
/// table exists with the expected internal columns after migrating.
#[tokio::test]
async fn migration_0004_idempotent() -> anyhow::Result<()> {
    let (_container, url) = common::start_postgres().await?;
    let pool = sqlx::PgPool::connect(&url).await?;

    // Migrate from empty (reaches 0004).
    sqlx::migrate!("./migrations").run(&pool).await?;

    let after_first: i64 =
        sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE success")
            .fetch_one(&pool)
            .await?;
    assert!(
        after_first >= 4,
        "expected at least four applied migrations (0001..0004), found {after_first}"
    );

    // pubkey_relays exists with its expected columns.
    let cols: Vec<String> = sqlx::query_scalar(
        "SELECT column_name FROM information_schema.columns \
         WHERE table_name = 'pubkey_relays' ORDER BY column_name",
    )
    .fetch_all(&pool)
    .await?;
    assert_eq!(
        cols,
        vec![
            "marker".to_string(),
            "pubkey_id".to_string(),
            "seen_at".to_string(),
            "url".to_string(),
        ],
        "pubkey_relays must have exactly (pubkey_id, url, marker, seen_at)"
    );

    // Re-run: must succeed and apply nothing further.
    sqlx::migrate!("./migrations").run(&pool).await?;
    let after_second: i64 =
        sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE success")
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        after_first, after_second,
        "re-running migrations applied additional versions; expected a no-op"
    );

    // pubkey_relays must NOT be exposed by the public contract view.
    let in_view: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.columns \
         WHERE table_name = 'pubkey_freshness' AND column_name = 'url')",
    )
    .fetch_one(&pool)
    .await?;
    assert!(
        !in_view,
        "pubkey_relays routing state must not leak into the pubkey_freshness contract view"
    );

    Ok(())
}

/// RELAY-05: `apply_relay_list` is a newest-wins FULL REPLACE — persisting a
/// second relay set for a pubkey deletes the prior rows entirely.
#[tokio::test]
async fn apply_relay_list_newest_wins_replace() -> anyhow::Result<()> {
    let (_container, pool) = common::fresh_db().await?;

    let key = [21u8; 32];
    let id = upsert_pubkey(&pool, &key[..]).await?;

    // First winning relay list.
    apply_relay_list(
        &pool,
        id,
        &[
            ("wss://old-a.example".to_string(), "write"),
            ("wss://old-b.example".to_string(), "read"),
        ],
        Utc::now(),
    )
    .await?;

    let count_after_first: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pubkey_relays WHERE pubkey_id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(count_after_first, 2, "first relay list must persist two rows");

    // Second winning relay list with a disjoint set.
    apply_relay_list(
        &pool,
        id,
        &[("wss://new.example".to_string(), "both")],
        Utc::now(),
    )
    .await?;

    let urls: Vec<String> =
        sqlx::query_scalar("SELECT url FROM pubkey_relays WHERE pubkey_id = $1 ORDER BY url")
            .bind(id)
            .fetch_all(&pool)
            .await?;
    assert_eq!(
        urls,
        vec!["wss://new.example".to_string()],
        "the second winning relay list must wholesale-replace the prior rows (newest-wins)"
    );

    Ok(())
}

/// RELAY-05 / Pitfall 2: `lookup_write_relays` returns `write` and `both`
/// markers (a bare r-tag advertises both) and EXCLUDES read-only relays.
#[tokio::test]
async fn lookup_write_relays_write_and_both() -> anyhow::Result<()> {
    let (_container, pool) = common::fresh_db().await?;

    let key = [22u8; 32];
    let id = upsert_pubkey(&pool, &key[..]).await?;

    apply_relay_list(
        &pool,
        id,
        &[
            ("wss://write-only.example".to_string(), "write"),
            ("wss://both.example".to_string(), "both"),
            ("wss://read-only.example".to_string(), "read"),
        ],
        Utc::now(),
    )
    .await?;

    let mut writes = lookup_write_relays(&pool, id).await?;
    writes.sort();

    assert_eq!(
        writes,
        vec![
            "wss://both.example".to_string(),
            "wss://write-only.example".to_string(),
        ],
        "lookup_write_relays must return write+both and exclude read-only"
    );

    Ok(())
}
