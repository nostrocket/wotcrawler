//! Smoke test proving the shared testcontainers Postgres fixture works against
//! real Postgres: start a container, connect with sqlx, run `SELECT 1`.
//!
//! Requires a running Docker daemon.

mod common;

use sqlx::Row;

#[tokio::test]
async fn postgres_bootstrap_connects_and_selects_one() -> anyhow::Result<()> {
    let (_container, url) = common::start_postgres().await?;

    let pool = sqlx::PgPool::connect(&url).await?;
    let row = sqlx::query("SELECT 1 AS one").fetch_one(&pool).await?;
    let one: i32 = row.try_get("one")?;
    assert_eq!(one, 1);

    Ok(())
}
