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
///
/// Kind-agnostic: it operates on `&Event` regardless of kind, so the SAME
/// function resolves kind:3 ContactList and kind:10002 RelayList (INGEST-05).
/// `EventId` derives `Ord` over its 32-byte big-endian id in nostr 0.44, so the
/// `then_with` tie-break is the deterministic NIP-01 lowest-id rule.
pub fn pick_winner<'a>(
    events: impl Iterator<Item = &'a Event>,
    now: Timestamp,
    future_clamp_secs: u64,
) -> Option<&'a Event> {
    // Saturating add so an absurd clamp value can't overflow the cutoff
    // (Pitfall 2). `created_at` is adversary-controlled.
    let cutoff = now.as_secs().saturating_add(future_clamp_secs);

    events
        .filter(|event| {
            if event.created_at.as_secs() > cutoff {
                // A future-dated event must never be able to pin a pubkey's list.
                metrics::counter!("ingest_future_dated").increment(1);
                false
            } else {
                true
            }
        })
        // Newest-wins on created_at; on a tie, the LOWEST id wins. `max_by`
        // returns the LAST maximum element, so to make the lowest id win on a
        // tie we order equal-timestamp events by DESCENDING id (b.id.cmp(&a.id))
        // — then the last (== maximum) element is the one with the lowest id.
        .max_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| b.id.cmp(&a.id))
        })
}
