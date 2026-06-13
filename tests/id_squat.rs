//! Id-squat censorship attack test (CR-01 / T-02-14, INGEST-02).
//!
//! Offline. A hostile relay sends a forged event F that carries a genuine
//! event G's claimed `id` but fails verification, ordered BEFORE the honest
//! copy G. If dedup ran before verification, F would insert G's id into the
//! cross-relay seen-set, verification of F would fail (forged), and the
//! genuine G would then be skipped as a "duplicate" — turning duplicate
//! suppression (INGEST-02) into a censorship primitive.
//!
//! The fix (dedup-after-verify) means only VERIFIED ids occupy the seen-set,
//! so F never touches it and G survives. This test fails with the old
//! ordering and passes with the fix.

mod common;

use std::collections::HashSet;

use nostr_sdk::{Kind, Timestamp};
use web_of_trust::ingest::ingest_events;

/// A forged id-squat copy ordered ahead of the genuine event must NOT suppress
/// it: G's follow list still emerges as the single ValidatedFollowList.
#[test]
fn id_squat_does_not_suppress_genuine_event() {
    let author = common::keys(20);
    let followee = common::keys(21).public_key();

    // Genuine, fully valid event from the honest author.
    let genuine = common::signed_event(
        &author,
        Kind::ContactList,
        Timestamp::now(),
        &[followee],
    );

    // Forged copy: claims `genuine.id` but fails verify() (tampered content).
    // A hostile relay (any signer) can send it; the author is the same so it
    // is "solicited", isolating the dedup-vs-verify ordering as the only thing
    // under test.
    let forgery = common::id_squat_forgery(
        &author,
        Kind::ContactList,
        Timestamp::now(),
        &genuine,
    );
    assert_eq!(forgery.id, genuine.id, "forgery must claim the genuine id");
    assert!(forgery.verify().is_err(), "forgery must fail verification");

    // Attacker orders the forged copy FIRST.
    let batch = vec![forgery, genuine.clone()];

    let mut requested = HashSet::new();
    requested.insert(author.public_key());

    let results = ingest_events(
        batch,
        Kind::ContactList,
        &requested,
        Timestamp::now(),
        3600,
        50_000,
    );

    assert_eq!(
        results.len(),
        1,
        "the genuine event must survive an id-squat forgery ordered ahead of it"
    );
    assert_eq!(results[0].follower_pubkey, author.public_key());
    assert_eq!(results[0].event_id, genuine.id);
    assert_eq!(results[0].followee_pubkeys, vec![followee]);
}
