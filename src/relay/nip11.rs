//! NIP-11 relay-information discovery + limit cache (RELAY-02).
//!
//! Fetches each relay's `RelayInformationDocument`, reads the `limitation`
//! fields (`max_limit`, `max_subscriptions`, `max_filters`), and caches them so
//! the pagination planner ([`super::fetch`]) can cap each filter's `limit` at
//! the relay's advertised `max_limit`. The exact SDK accessor vs. `reqwest`
//! fallback and the per-field defaults are recorded in
//! `.planning/phases/02-relay-acquisition-validation/02-SPIKES.md` (RELAY-02).
//!
//! Stub bodies in plan 02-01; implemented in plan 02-03 Task 2.

use crate::error::RelayError;

/// Per-relay NIP-11 limits the pagination planner consumes.
///
/// Populated from the relay's `limitation` block; any field a relay omits
/// falls back to the sane default recorded in `02-SPIKES.md` (RELAY-02).
#[derive(Debug, Clone, Copy)]
pub struct RelayLimits {
    /// The maximum number of events a relay returns per REQ (`max_limit`).
    pub max_limit: usize,
    /// The maximum number of concurrent subscriptions (`max_subscriptions`).
    pub max_subscriptions: usize,
    /// The maximum number of filters per REQ (`max_filters`).
    pub max_filters: usize,
}

/// Fetch and parse a relay's NIP-11 limits.
///
/// `relay_url` is the websocket url of the relay; the NIP-11 document is read
/// over HTTP(S) with `Accept: application/nostr+json` (per `02-SPIKES.md`),
/// then the `limitation` fields are extracted with omitted fields defaulted.
pub async fn fetch_limits(_relay_url: &str) -> Result<RelayLimits, RelayError> {
    todo!("plan 02-03 Task 2: fetch + parse NIP-11 limitation per 02-SPIKES.md")
}
