//! Typed errors for the crate's module boundaries.
//!
//! One `#[derive(Debug, Error)]` enum per boundary:
//! - [`StoreError`] — the Phase 1 Postgres write layer.
//! - [`RelayError`] — the Phase 2 acquisition (relay-pool / NIP-11 / fetch) layer.
//! - [`IngestError`] — the Phase 2 validation layer, reserved for *genuine*
//!   failures (see the [`IngestError`] doc comment for the count-and-skip vs.
//!   error split).

use nostr_sdk::Kind;
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

/// Errors surfaced by the relay-acquisition layer (RELAY-01..04).
///
/// nostr-sdk owns the websocket lifecycle, reconnect, and relay-message
/// handling; this enum wraps its client error transparently and adds the
/// app-side acquisition failures (a relay missing from the pool, a NIP-11
/// document that could not be fetched/parsed, and a per-fetch deadline that
/// expired). Reconnect/backoff is policy on top of nostr-sdk, not an error.
#[derive(Debug, Error)]
pub enum RelayError {
    /// An underlying nostr-sdk client/relay-pool error (connect, subscribe,
    /// fetch, relay-message handling).
    #[error(transparent)]
    Client(#[from] nostr_sdk::client::Error),

    /// A relay url was requested that is not present in the managed pool.
    #[error("relay not found in pool: {0}")]
    RelayNotFound(String),

    /// The relay's NIP-11 `RelayInformationDocument` could not be fetched or
    /// parsed (RELAY-02).
    #[error("NIP-11 fetch failed for {relay}: {reason}")]
    Nip11Fetch {
        /// The relay url whose NIP-11 document failed.
        relay: String,
        /// A human-readable reason (never embeds secrets — T-02-01).
        reason: String,
    },

    /// A per-fetch deadline expired before the window completed (Pitfall 9 —
    /// a timed-out window is requeued, never treated as "complete").
    #[error("fetch timed out for relay {0}")]
    FetchTimeout(String),
}

/// Errors surfaced by the validation/ingest layer (INGEST-01..05).
///
/// IMPORTANT — count-and-skip vs. error: routine, expected validation
/// rejections that happen at scale on adversarial relay input (bad signature,
/// unsolicited kind/author, future-dated beyond the clamp, oversized follow
/// list in the count-and-skip path) are NOT errors. The ingest gate returns
/// `false` for those events and increments a `metrics` counter so a single bad
/// event never aborts a batch. `IngestError` is reserved for *genuine*
/// failures that should propagate — its variants exist so callers can match a
/// specific cause when the configured policy chooses to surface a rejection
/// (e.g. a strict mode that rejects rather than counts) rather than silently
/// skip it.
#[derive(Debug, Error)]
pub enum IngestError {
    /// An event failed `Event::verify()` (id recomputation or secp256k1
    /// signature) (INGEST-01).
    #[error("event signature/id verification failed")]
    InvalidSignature,

    /// A relay returned an event of a kind/author that was not requested
    /// (INGEST-01 / Pitfall 4 — relays are adversarial).
    #[error("unsolicited event: wanted kind {wanted}, got {got}")]
    UnsolicitedEvent {
        /// The kind the fetch requested.
        wanted: Kind,
        /// The kind the relay actually returned.
        got: Kind,
    },

    /// The event's `created_at` is beyond the configurable future clamp
    /// (INGEST-03 / Pitfall 2 — future-dated junk pinning a list).
    #[error("event is future-dated beyond the clamp")]
    FutureDated,

    /// An extracted follow list exceeds the configurable `follow_cap`
    /// (INGEST-04 / Pitfall 6 — follow-bomb). Carries the offending size,
    /// mirroring [`StoreError::InvalidPubkey`]'s value-carrying shape.
    #[error("oversized follow list: {0} p-tags exceeds the configured cap")]
    OversizedFollowList(usize),
}
