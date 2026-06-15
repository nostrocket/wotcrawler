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

use nostr_sdk::nips::nip65;
use nostr_sdk::nips::nip65::RelayMetadata;
use nostr_sdk::Event;

use super::{timestamp_to_datetime, ValidatedRelayList};

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
