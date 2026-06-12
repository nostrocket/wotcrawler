//! Typed errors for the store crate boundary.

use thiserror::Error;

/// Errors surfaced by the store layer.
#[derive(Debug, Error)]
pub enum StoreError {
    /// An underlying sqlx error (query, decode, pool, connection).
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),

    /// A migration failed to apply.
    #[error("migration failed: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
}
