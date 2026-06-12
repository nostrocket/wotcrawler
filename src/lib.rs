//! web-of-trust: nostr follow-graph crawler & data layer.
//!
//! Crate root. Re-exports the error type and the (Plan 03) store module.

pub mod error;
pub mod store;

pub use error::StoreError;
