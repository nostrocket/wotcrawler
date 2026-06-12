//! Cross-relay event-id dedup test (INGEST-02).
//!
//! Offline. The same event id arriving twice (simulating two relays returning
//! the identical kind-3) must be processed at most once: the orchestrator's
//! `HashSet<EventId>` seen-set skips the second occurrence, so a single author
//! yields a single `ValidatedFollowList`.

mod common;

use nostr_sdk::{Kind, Timestamp};
use web_of_trust::ingest::ingest_events;

/// The same event presented twice is processed once.
#[test]
fn duplicate_event_id_processed_once() {
    let author = common::keys(10);
    let followee = common::keys(11).public_key();
    let event = common::signed_event(
        &author,
        Kind::ContactList,
        Timestamp::now(),
        &[followee],
    );

    // Two relays return the identical event (same id).
    let batch = vec![event.clone(), event.clone()];

    let results = ingest_events(
        batch,
        Kind::ContactList,
        Timestamp::now(),
        3600,
        50_000,
    );

    assert_eq!(
        results.len(),
        1,
        "a duplicate event id must yield exactly one ValidatedFollowList"
    );
    assert_eq!(results[0].follower_pubkey, author.public_key());
    assert_eq!(results[0].followee_pubkeys, vec![followee]);
}
