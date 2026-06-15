//! kind:10002 (NIP-65 RelayList) resolution test (INGEST-05).
//!
//! Offline. Proves the SAME `pick_winner` resolver — written for kind:3
//! ContactList — handles kind:10002 RelayList identically: it operates on
//! `&Event` regardless of kind, so the newest valid relay-list per pubkey wins
//! under the identical replaceable rules.

mod common;

use nostr_sdk::{Kind, Timestamp};
use web_of_trust::ingest::relay_list::{extract_relay_pairs, from_event};
use web_of_trust::ingest::replaceable::pick_winner;

/// The newest valid kind:10002 event is selected by the same resolver.
#[test]
fn newest_relay_list_wins() {
    let author = common::keys(7);
    let now = Timestamp::now();

    // Two RelayList (kind:10002) events for one pubkey, different created_at.
    let older = common::signed_event(
        &author,
        Kind::RelayList,
        Timestamp::from(now.as_secs() - 200),
        &[],
    );
    let newer = common::signed_event(
        &author,
        Kind::RelayList,
        Timestamp::from(now.as_secs() - 5),
        &[],
    );

    assert_eq!(older.kind, Kind::RelayList);
    assert_eq!(newer.kind, Kind::RelayList);

    let candidates = [older.clone(), newer.clone()];
    let winner =
        pick_winner(candidates.iter(), now, 3600).expect("a relay-list winner must exist");
    assert_eq!(
        winner.id, newer.id,
        "the newest kind:10002 event must win under the identical replaceable rules"
    );
}

/// A future-dated kind:10002 event is clamped out just like a kind:3 one.
#[test]
fn future_dated_relay_list_is_clamped() {
    let author = common::keys(8);
    let now = Timestamp::now();

    let in_range = common::signed_event(&author, Kind::RelayList, now, &[]);
    let future = common::future_dated_event(&author, Kind::RelayList, 365 * 24 * 3600);

    let candidates = [future, in_range.clone()];
    let winner = pick_winner(candidates.iter(), now, 3600)
        .expect("the in-range relay list must survive");
    assert_eq!(winner.id, in_range.id, "future-dated kind:10002 must be clamped out");
}

/// RELAY-05: a bare r-tag (no read/write token) extracts as the "both" marker,
/// an `r url read` as "read", an `r url write` as "write" — exercising the
/// `nip65::extract_relay_list` mapping in `ingest::relay_list`.
#[test]
fn extract_relay_pairs_maps_bare_read_write_markers() {
    let event = common::relay_list_event(
        11,
        &[
            ("wss://bare.example", "both"),
            ("wss://reader.example", "read"),
            ("wss://writer.example", "write"),
        ],
        Timestamp::now().as_secs(),
    );

    let pairs = extract_relay_pairs(&event);

    assert!(
        pairs.contains(&("wss://bare.example".to_string(), "both")),
        "a bare r-tag must map to the \"both\" marker, got: {pairs:?}"
    );
    assert!(
        pairs.contains(&("wss://reader.example".to_string(), "read")),
        "an `r url read` tag must map to \"read\", got: {pairs:?}"
    );
    assert!(
        pairs.contains(&("wss://writer.example".to_string(), "write")),
        "an `r url write` tag must map to \"write\", got: {pairs:?}"
    );
}

/// RELAY-05: extracted relay urls are normalized to the trailing-slash-free
/// canonical form used as the relay key everywhere in the relay layer.
#[test]
fn extract_relay_pairs_normalizes_trailing_slash() {
    let event = common::relay_list_event(
        12,
        &[("wss://trailing.example/", "write")],
        Timestamp::now().as_secs(),
    );

    let pairs = extract_relay_pairs(&event);

    assert_eq!(
        pairs,
        vec![("wss://trailing.example".to_string(), "write")],
        "extracted url must drop the trailing slash, got: {pairs:?}"
    );
}

/// RELAY-05: `from_event` carries the winning event's pubkey/id/created_at plus
/// the extracted pairs into the `ValidatedRelayList` contract.
#[test]
fn from_event_builds_validated_relay_list() {
    let created = Timestamp::now().as_secs();
    let event = common::relay_list_event(13, &[("wss://w.example", "write")], created);

    let vrl = from_event(&event);

    assert_eq!(vrl.pubkey, event.pubkey, "pubkey must be the event author");
    assert_eq!(vrl.event_id, event.id, "event_id must be the winning event id");
    assert_eq!(
        vrl.created_at.timestamp() as u64,
        created,
        "created_at must convert from the event timestamp"
    );
    assert_eq!(
        vrl.relays,
        vec![("wss://w.example".to_string(), "write")],
        "relays must be the extracted (url, marker) pairs"
    );
}
