//! Author-chunked `until`-window pagination loop (RELAY-03).
//!
//! Defeats the EOSE-completeness trap (Pitfall 1): `fetch_events` auto-closes
//! on EOSE, but a relay silently caps at `max_limit`, so a returned window that
//! equals the cap is NOT proof of completeness. This loop chunks the author set
//! under `max_authors_per_req`, pages backwards with `until = oldest - 1`, and
//! only stops when a window returns strictly fewer than the effective cap
//! (`min(requested_limit, relay_max_limit)`). Every fetch carries a timeout
//! (Pitfall 9); a timed-out window is requeued, never treated as done.
//!
//! Stub bodies in plan 02-01; implemented in plan 02-03 Task 3 and wired to
//! ingest in plan 02-04.

use nostr_sdk::{Client, Event, Kind, PublicKey};

use crate::error::RelayError;

/// Fetch every stored event of `kind` for `authors` from the connected pool,
/// paging past relay `max_limit` caps until each window is exhausted.
///
/// `max_limit` is the effective per-window cap (relay NIP-11 `max_limit` from
/// [`super::nip11`]); `max_authors` is the author-chunk size kept under the
/// relay cap. Returns the raw, still-unverified events for the ingest gate
/// ([`crate::ingest`]) to validate.
pub async fn fetch_complete(
    _client: &Client,
    _authors: &[PublicKey],
    _kind: Kind,
    _max_limit: usize,
    _max_authors: usize,
) -> Result<Vec<Event>, RelayError> {
    todo!("plan 02-03 Task 3: author-chunked until-window pagination loop")
}
