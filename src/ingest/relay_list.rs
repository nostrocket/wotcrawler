//! kind:10002 (NIP-65) r-tag extraction (RELAY-05).
//!
//! The shared INGEST-05 pipeline ([`super::ingest_events`] / `pick_winner`)
//! already resolves the newest valid kind:10002 event per pubkey but discards
//! its relay-url r-tags at the contract boundary (it emits only p-tag pubkeys).
//! This module re-reads the winning event's r-tags into the
//! [`ValidatedRelayList`] companion contract.
//!
//! Tag parsing is delegated entirely to the built-in NIP-65 helper
//! `nostr_sdk::nip65::extract_relay_list` — never hand-rolled (CLAUDE.md:
//! "Never hand-roll nostr parsing/crypto"). The only app-side logic here is the
//! marker mapping (`None` = both, per NIP-65) and url normalization to the
//! canonical trailing-slash-free form used as the relay key everywhere else in
//! the relay layer.

use std::collections::HashSet;

use nostr_sdk::nips::nip65;
use nostr_sdk::nips::nip65::RelayMetadata;
use nostr_sdk::{Event, Kind, PublicKey, Timestamp};

use super::{replaceable, timestamp_to_datetime, verify, ValidatedRelayList};

/// Map a NIP-65 relay marker to its stored `pubkey_relays.marker` token.
///
/// NIP-65: a bare `r` tag (no read/write token) advertises BOTH read and write.
/// `extract_relay_list` returns `None` for that case, which is easy to misread
/// as "no marker → skip"; mapping it to `"both"` is what lets the fallback's
/// `marker IN ('write','both')` lookup recover a bare-r-tag write relay
/// (RESEARCH Pitfall 2).
fn marker_of(meta: &Option<RelayMetadata>) -> &'static str {
    match meta {
        Some(RelayMetadata::Read) => "read",
        Some(RelayMetadata::Write) => "write",
        None => "both",
    }
}

/// Extract the `(url, marker)` pairs from a winning kind:10002 event.
///
/// Urls are normalized with [`RelayUrl::as_str_without_trailing_slash`] so they
/// match the relay-url keys used by the GCRA limiter and the rest of the relay
/// layer. The marker is one of `"read"`, `"write"`, `"both"` (see
/// [`marker_of`]).
///
/// [`RelayUrl::as_str_without_trailing_slash`]: nostr_sdk::RelayUrl::as_str_without_trailing_slash
pub fn extract_relay_pairs(event: &Event) -> Vec<(String, &'static str)> {
    nip65::extract_relay_list(event)
        .map(|(url, meta)| {
            (
                url.as_str_without_trailing_slash().to_string(),
                marker_of(meta),
            )
        })
        .collect()
}

/// Assemble a [`ValidatedRelayList`] from a winning kind:10002 event.
///
/// The caller (the ingest orchestrator / fallback path) supplies the event that
/// won replaceable resolution via the unchanged `pick_winner`; this constructor
/// performs only the r-tag extraction ([`extract_relay_pairs`]) and the
/// `Timestamp` -> `DateTime<Utc>` conversion (reusing
/// [`super::timestamp_to_datetime`] so the conversion lives in one place).
pub fn from_event(event: &Event) -> ValidatedRelayList {
    ValidatedRelayList {
        pubkey: event.pubkey,
        event_id: event.id,
        created_at: timestamp_to_datetime(event.created_at),
        relays: extract_relay_pairs(event),
    }
}

/// Resolve the winning kind:10002 ([`ValidatedRelayList`]) for a SINGLE author
/// from a raw, untrusted event union (the on-demand fallback path, RELAY-05).
///
/// This is the relay-list analogue of [`super::ingest_events`]: it runs the
/// IDENTICAL gate primitives — [`verify::accept`] (id+sig + `Kind::RelayList`
/// kind/author gate, INGEST-01), the verify-before-dedup ordering (CR-01 /
/// T-02-14 — only verified ids enter the seen-set), and
/// [`replaceable::pick_winner`] (future-clamp + newest-wins + lowest-id
/// tie-break, INGEST-03/05) — then re-reads the winner's r-tags via
/// [`from_event`]. It owns NO new validation logic; it only narrows the existing
/// gate to a single requested author and emits the relay-list contract value
/// (`ingest_events` discards r-tags, so the kind:10002 path needs the winning
/// event itself, not a [`super::ValidatedFollowList`]).
///
/// `author` is the only solicited pubkey: any event from a different author is
/// dropped as unsolicited (Pitfall 4). `now` + `future_clamp_secs` drive the
/// future clamp. A write relay is just as adversarial as a curated one, which is
/// exactly why this reuses the full verify/dedup/clamp gate rather than trusting
/// the raw fetch. Returns `None` when no valid kind:10002 for `author` survives
/// (the caller then proceeds to `not_found` WITHOUT consuming the kind-3 retry
/// budget — Open Question 1).
pub fn resolve_relay_list(
    events: impl IntoIterator<Item = Event>,
    author: PublicKey,
    now: Timestamp,
    future_clamp_secs: u64,
) -> Option<ValidatedRelayList> {
    let requested: HashSet<PublicKey> = HashSet::from([author]);
    let mut seen: HashSet<nostr_sdk::EventId> = HashSet::new();
    let mut candidates: Vec<Event> = Vec::new();

    for event in events {
        // SECURITY-CRITICAL ORDERING (CR-01 / T-02-14): verify BEFORE dedup so a
        // forged id-squat copy can never consume a genuine id in the seen-set.
        if !verify::accept(&event, Kind::RelayList, &requested) {
            continue; // forged or unsolicited (counted inside the gate).
        }
        if !seen.insert(event.id) {
            continue; // genuine duplicate id (verified) — already handled.
        }
        candidates.push(event);
    }

    let winner = replaceable::pick_winner(candidates.iter(), now, future_clamp_secs)?;
    Some(from_event(winner))
}
