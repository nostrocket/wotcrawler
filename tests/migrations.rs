//! Integration tests for the Phase 1 graph schema migration (GRAPH-01).
//!
//! Proves migrate-from-empty, re-run idempotency, and schema shape against an
//! ephemeral Postgres started via the shared [`common::start_postgres`] fixture.
//! All queries are parameterized sqlx queries — never string-formatted SQL.
//!
//! Requires a running Docker daemon.

mod common;

use sqlx::Row;

/// GRAPH-01: a fresh database migrates from empty, and re-running the
/// migrations against the already-migrated database is a no-op (succeeds,
/// applies zero further versions).
#[tokio::test]
async fn migrations_idempotent() -> anyhow::Result<()> {
    let (_container, url) = common::start_postgres().await?;
    let pool = sqlx::PgPool::connect(&url).await?;

    // Migrate from empty.
    sqlx::migrate!("./migrations").run(&pool).await?;

    // The migration tracking table records exactly the applied versions.
    let after_first: i64 =
        sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE success")
            .fetch_one(&pool)
            .await?;
    assert!(
        after_first >= 1,
        "expected at least one applied migration, found {after_first}"
    );

    // Re-run against the already-migrated DB: must succeed and change nothing.
    sqlx::migrate!("./migrations").run(&pool).await?;

    let after_second: i64 =
        sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE success")
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        after_first, after_second,
        "re-running migrations applied additional versions; expected a no-op"
    );

    Ok(())
}

/// GRAPH-01: after migrating, the schema shape is correct — pubkeys has a
/// bigint identity id, a bytea pubkey, all freshness/churn/applied columns, and
/// follows has exactly (follower_id, followee_id) bigint with a two-column PK.
#[tokio::test]
async fn schema_shape() -> anyhow::Result<()> {
    let (_container, url) = common::start_postgres().await?;
    let pool = sqlx::PgPool::connect(&url).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;

    // pubkey is bytea.
    let pubkey_type: String = sqlx::query_scalar(
        "SELECT data_type FROM information_schema.columns \
         WHERE table_name = 'pubkeys' AND column_name = 'pubkey'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(pubkey_type, "bytea", "pubkeys.pubkey should be bytea");

    // id is a bigint identity column.
    let id_row = sqlx::query(
        "SELECT data_type, is_identity FROM information_schema.columns \
         WHERE table_name = 'pubkeys' AND column_name = 'id'",
    )
    .fetch_one(&pool)
    .await?;
    let id_type: String = id_row.try_get("data_type")?;
    let id_is_identity: String = id_row.try_get("is_identity")?;
    assert_eq!(id_type, "bigint", "pubkeys.id should be bigint");
    assert_eq!(
        id_is_identity, "YES",
        "pubkeys.id should be a GENERATED IDENTITY column"
    );

    // All freshness / churn / applied columns are present on pubkeys.
    for col in [
        "last_fetched_at",
        "last_confirmed_at",
        "last_changed_at",
        "fetch_count",
        "change_count",
        "applied_event_id",
        "applied_created_at",
    ] {
        let present: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM information_schema.columns \
             WHERE table_name = 'pubkeys' AND column_name = $1)",
        )
        .bind(col)
        .fetch_one(&pool)
        .await?;
        assert!(present, "pubkeys is missing expected column {col}");
    }

    // follows has exactly (follower_id, followee_id), both bigint.
    let follows_cols: Vec<(String, String)> = sqlx::query(
        "SELECT column_name, data_type FROM information_schema.columns \
         WHERE table_name = 'follows' ORDER BY column_name",
    )
    .fetch_all(&pool)
    .await?
    .into_iter()
    .map(|r| {
        (
            r.try_get::<String, _>("column_name").unwrap(),
            r.try_get::<String, _>("data_type").unwrap(),
        )
    })
    .collect();
    assert_eq!(
        follows_cols,
        vec![
            ("followee_id".to_string(), "bigint".to_string()),
            ("follower_id".to_string(), "bigint".to_string()),
        ],
        "follows should have exactly (follower_id, followee_id) bigint columns"
    );

    // follows has a two-column primary key.
    let pk_cols: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.key_column_usage k \
         JOIN information_schema.table_constraints t \
           ON k.constraint_name = t.constraint_name \
         WHERE t.table_name = 'follows' AND t.constraint_type = 'PRIMARY KEY'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(pk_cols, 2, "follows primary key should span two columns");

    Ok(())
}

/// Phase 3 (migration 0002): the widened status CHECK accepts the transient
/// `in_progress` lease state, the `pubkey_freshness` contract view collapses
/// `in_progress` back to `discovered`, and the new internal lease/retry columns
/// (`claimed_at`, `fetch_attempts`) are absent from the public contract view
/// (T-03-02). The `migrations_idempotent` test above already proves 0002 re-run
/// is a no-op, so that assertion is not duplicated here.
#[tokio::test]
async fn migration_0002_widens_status_and_hides_in_progress() -> anyhow::Result<()> {
    let (_container, url) = common::start_postgres().await?;
    let pool = sqlx::PgPool::connect(&url).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;

    // Seed a row via a parameterized insert (never string-formatted SQL), then
    // move it into the transient lease state — proving the widened CHECK domain.
    let key = [7u8; 32];
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO pubkeys (pubkey) VALUES ($1) RETURNING id",
    )
    .bind(&key[..])
    .fetch_one(&pool)
    .await?;

    sqlx::query("UPDATE pubkeys SET status = 'in_progress' WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await?;

    // The contract view collapses in_progress -> discovered.
    let view_status: String =
        sqlx::query_scalar("SELECT status FROM pubkey_freshness WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        view_status, "discovered",
        "pubkey_freshness must collapse the transient in_progress lease state to discovered"
    );

    // The internal lease/retry columns must not be exposed by the contract view.
    for hidden in ["claimed_at", "fetch_attempts"] {
        let exposed: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM information_schema.columns \
             WHERE table_name = 'pubkey_freshness' AND column_name = $1)",
        )
        .bind(hidden)
        .fetch_one(&pool)
        .await?;
        assert!(
            !exposed,
            "pubkey_freshness must not expose internal column {hidden}"
        );
    }

    // The contract view exposes exactly the documented three columns.
    let view_cols: Vec<String> = sqlx::query_scalar(
        "SELECT column_name FROM information_schema.columns \
         WHERE table_name = 'pubkey_freshness' ORDER BY column_name",
    )
    .fetch_all(&pool)
    .await?;
    assert_eq!(
        view_cols,
        vec![
            "id".to_string(),
            "last_fetched_at".to_string(),
            "status".to_string(),
        ],
        "pubkey_freshness must expose exactly (id, status, last_fetched_at)"
    );

    Ok(())
}
