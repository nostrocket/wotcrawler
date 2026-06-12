//! Replaceable-event resolution: future clamp + newest-wins + tie-break
//! (INGEST-03, INGEST-05).
//!
//! `created_at` is adversary-controlled, so a naive `max(created_at)` lets one
//! future-dated junk event pin a pubkey's list forever (Pitfall 2). This module
//! rejects any event whose `created_at` exceeds `now + future_clamp_secs`, then
//! picks the winner across the remaining valid events for a `(pubkey, kind)`:
//! highest `created_at`, ties broken by the LOWEST `EventId` (NIP-01's
//! deterministic tie-break, prevents same-timestamp flapping between relays —
//! Pitfall 3). The identical rule applies to kind:10002 (NIP-65, INGEST-05).
//!
//! Stub body in plan 02-01; implemented in plan 02-02 Task 2.

use nostr_sdk::{Event, Timestamp};

/// Pick the winning replaceable event from a set of already-verified events.
///
/// Filters out events dated beyond `now + future_clamp_secs`, then returns the
/// one with the highest `created_at`, breaking ties on the lowest `EventId`.
/// Returns `None` if no event survives the clamp.
pub fn pick_winner<'a>(
    _events: impl Iterator<Item = &'a Event>,
    _now: Timestamp,
    _future_clamp_secs: u64,
) -> Option<&'a Event> {
    todo!("plan 02-02 Task 2: clamp + newest-wins + lowest-id tie-break")
}
