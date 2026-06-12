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

    /// A pubkey crossing the store boundary was not exactly 32 bytes (V5 input
    /// validation): nostr x-only pubkeys are always 32 bytes.
    #[error("invalid pubkey length: expected 32 bytes, got {0}")]
    InvalidPubkey(usize),
}
