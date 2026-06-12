//! web-of-trust: nostr follow-graph crawler & data layer.
//!
//! Crate root. Re-exports the typed errors and the module trees:
//! - `store` (Phase 1) — the Postgres write layer.
//! - `relay` (Phase 2) — relay-pool acquisition, NIP-11 limits, pagination.
//! - `ingest` (Phase 2) — the validation gate that emits `ValidatedFollowList`.

pub mod error;
pub mod ingest;
pub mod relay;
pub mod store;

pub use error::{IngestError, RelayError, StoreError};
