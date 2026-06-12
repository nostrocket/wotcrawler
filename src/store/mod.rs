//! Store layer: PgPool wiring, programmatic migrations, and the transactional
//! edge-diff writer.
//!
//! This module is the crawler's write API over the Phase 1 schema
//! (`migrations/0001_graph_schema.sql`). Concurrency, pooling, and migration
//! idempotency are delegated to sqlx / Postgres; the only custom logic is the
//! edge-diff computation in [`follows::apply_follow_list`].
//!
//! All queries use sqlx's parameterized API (`query!` / `query_scalar!` /
//! `query_as!`) exclusively — SQL is never string-formatted (T-03-01).

pub mod follows;
pub mod pubkeys;

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

use crate::error::StoreError;

/// Maximum connections in the crawler's writer pool.
///
/// Sizing is explicit Claude discretion (RESEARCH A4); 8 is a sane default for
/// the single-process crawler and is not load-bearing for Phase 1 correctness.
const MAX_CONNECTIONS: u32 = 8;

/// Connect to Postgres and return a pooled handle.
///
/// The `database_url` is loaded by the caller via `config`/env and is never
/// logged (T-03-04 — DB URL leakage).
pub async fn connect(database_url: &str) -> Result<PgPool, StoreError> {
    let pool = PgPoolOptions::new()
        .max_connections(MAX_CONNECTIONS)
        .connect(database_url)
        .await?;
    Ok(pool)
}

/// Apply all pending migrations to `pool`.
///
/// Idempotent: sqlx compares `migrations/` against the `_sqlx_migrations`
/// tracking table and applies only pending versions, so re-running against an
/// already-migrated database is a no-op (GRAPH-01).
pub async fn run_migrations(pool: &PgPool) -> Result<(), StoreError> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}
