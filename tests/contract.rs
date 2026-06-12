//! Integration tests for the Phase 1 public contract surface (GRAPH-04).
//!
//! Proves the three contract views exist and are SELECT-able, that
//! pubkey_freshness exposes freshness while hiding internal bookkeeping (D-11)
//! and surfaces discovered-but-unfetched rows (D-12), and that every contract
//! view carries an introspectable PUBLIC CONTRACT comment (D-02).
//! All queries are parameterized sqlx queries — never string-formatted SQL.
//!
//! Requires a running Docker daemon.

mod common;

/// GRAPH-04: all three contract views exist and are SELECT-able after migrating.
#[tokio::test]
async fn contract_views_present() -> anyhow::Result<()> {
    let (_container, url) = common::start_postgres().await?;
    let pool = sqlx::PgPool::connect(&url).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;

    // Each contract view must be SELECT-able. View names are fixed identifiers
    // (not user input), so a static query per view keeps SQL un-formatted.
    sqlx::query("SELECT follower_id, followee_id FROM follow_edges LIMIT 0")
        .fetch_all(&pool)
        .await?;
    sqlx::query("SELECT id, pubkey FROM pubkey_lookup LIMIT 0")
        .fetch_all(&pool)
        .await?;
    sqlx::query("SELECT id, status, last_fetched_at FROM pubkey_freshness LIMIT 0")
        .fetch_all(&pool)
        .await?;

    Ok(())
}

/// D-11 / D-12: pubkey_freshness exposes exactly {id, status, last_fetched_at}
/// (freshness exposed, internal bookkeeping hidden), and a discovered-status row
/// is visible (honest knowledge boundary).
#[tokio::test]
async fn freshness_exposed_bookkeeping_hidden() -> anyhow::Result<()> {
    let (_container, url) = common::start_postgres().await?;
    let pool = sqlx::PgPool::connect(&url).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;

    // The exact column set of pubkey_freshness.
    let cols: Vec<String> = sqlx::query_scalar(
        "SELECT column_name FROM information_schema.columns \
         WHERE table_name = 'pubkey_freshness' ORDER BY column_name",
    )
    .fetch_all(&pool)
    .await?;
    assert_eq!(
        cols,
        vec![
            "id".to_string(),
            "last_fetched_at".to_string(),
            "status".to_string(),
        ],
        "pubkey_freshness must expose exactly {{id, status, last_fetched_at}}"
    );

    // Internal bookkeeping columns must NOT be exposed.
    for hidden in [
        "fetch_count",
        "change_count",
        "applied_event_id",
        "applied_created_at",
        "last_changed_at",
        "last_confirmed_at",
    ] {
        assert!(
            !cols.contains(&hidden.to_string()),
            "pubkey_freshness must not expose internal column {hidden}"
        );
    }

    // D-12: a discovered-but-unfetched pubkey is visible in pubkey_freshness
    // with status 'discovered'. Insert a 32-byte pubkey (default status).
    let pk = vec![0xABu8; 32];
    let id: i64 = sqlx::query_scalar("INSERT INTO pubkeys (pubkey) VALUES ($1) RETURNING id")
        .bind(&pk)
        .fetch_one(&pool)
        .await?;

    let status: String =
        sqlx::query_scalar("SELECT status FROM pubkey_freshness WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        status, "discovered",
        "a freshly-inserted pubkey should appear in pubkey_freshness as 'discovered'"
    );

    Ok(())
}

/// D-02: each contract view carries a non-null COMMENT ON containing the
/// PUBLIC CONTRACT label, retrievable via obj_description.
#[tokio::test]
async fn contract_comments_present() -> anyhow::Result<()> {
    let (_container, url) = common::start_postgres().await?;
    let pool = sqlx::PgPool::connect(&url).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;

    for view in ["follow_edges", "pubkey_lookup", "pubkey_freshness"] {
        // obj_description(regclass) resolves the view's comment; bind the view
        // name as a parameter cast to regclass rather than formatting SQL.
        let comment: Option<String> =
            sqlx::query_scalar("SELECT obj_description($1::regclass)")
                .bind(view)
                .fetch_one(&pool)
                .await?;
        let comment = comment
            .unwrap_or_else(|| panic!("contract view {view} has no COMMENT ON"));
        assert!(
            comment.contains("PUBLIC CONTRACT"),
            "comment on {view} should contain PUBLIC CONTRACT, got: {comment}"
        );
    }

    Ok(())
}
