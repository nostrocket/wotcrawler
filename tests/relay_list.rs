//! kind:10002 (NIP-65 RelayList) resolution test (INGEST-05).
//!
//! Offline. Proves the SAME `pick_winner` resolver — written for kind:3
//! ContactList — handles kind:10002 RelayList identically: it operates on
//! `&Event` regardless of kind, so the newest valid relay-list per pubkey wins
//! under the identical replaceable rules.

mod common;

use nostr_sdk::{Kind, Timestamp};
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
