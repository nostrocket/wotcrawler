//! web-of-trust: nostr follow-graph crawler & data layer.
//!
//! Crate root. Re-exports the typed errors and the module trees:
//! - `store` (Phase 1) — the Postgres write layer.
//! - `relay` (Phase 2) — relay-pool acquisition, NIP-11 limits, pagination.
//! - `ingest` (Phase 2) — the validation gate that emits `ValidatedFollowList`.
//! - `crawl` (Phase 3) — the DB-resident BFS frontier (claim/lease/seed/requeue).
//! - `daemon` (Phase 4) — the long-running daemon orchestrator (config, loop,
//!   observability) wired by the `crawler` binary.

pub mod crawl;
pub mod daemon;
pub mod error;
pub mod ingest;
pub mod relay;
pub mod store;

pub use error::{IngestError, RelayError, StoreError};
