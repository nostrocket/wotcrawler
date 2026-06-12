//! Signature/kind/author verify-gate tests (INGEST-01).
//!
//! Offline, no network, no Postgres. Exercises `ingest::verify::accept`:
//! a validly-signed solicited event passes; a forged (id/sig mismatch) event is
//! rejected; and unsolicited events (wrong kind or wrong author) are dropped.

mod common;

use std::collections::HashSet;

use nostr_sdk::{Kind, Timestamp};
use web_of_trust::ingest::verify::accept;

/// A validly-signed event of the requested kind from a requested author passes.
#[test]
fn signed_solicited_event_is_accepted() {
    let author = common::keys(1);
    let event = common::signed_event(&author, Kind::ContactList, Timestamp::now(), &[]);

    let mut requested = HashSet::new();
    requested.insert(author.public_key());

    assert!(
        accept(&event, Kind::ContactList, &requested),
        "a verifying, solicited event must pass the gate"
    );
}

/// A forged event (content mutated after signing -> id/sig mismatch) is rejected.
#[test]
fn forged_event_is_rejected() {
    let author = common::keys(2);
    let event = common::forged_event(&author, Kind::ContactList, Timestamp::now());

    let mut requested = HashSet::new();
    requested.insert(author.public_key());

    assert!(
        !accept(&event, Kind::ContactList, &requested),
        "an event failing Event::verify() must be rejected"
    );
}

/// Unsolicited events — wrong kind, or an author the fetch never requested — are
/// dropped by the gate (Pitfall 4: relays are adversarial).
mod unsolicited {
    use super::*;

    /// A correctly-signed event of the WRONG kind is dropped.
    #[test]
    fn wrong_kind_is_dropped() {
        let author = common::keys(3);
        // Signed kind:10002, but the fetch wanted kind:3 (ContactList).
        let event = common::signed_event(&author, Kind::RelayList, Timestamp::now(), &[]);

        let mut requested = HashSet::new();
        requested.insert(author.public_key());

        assert!(
            !accept(&event, Kind::ContactList, &requested),
            "an event whose kind != want_kind must be dropped"
        );
    }

    /// A correctly-signed event from an author NOT in the requested set is dropped.
    #[test]
    fn wrong_author_is_dropped() {
        let author = common::keys(4);
        let event = common::signed_event(&author, Kind::ContactList, Timestamp::now(), &[]);

        // The fetch requested a DIFFERENT author.
        let mut requested = HashSet::new();
        requested.insert(common::keys(5).public_key());

        assert!(
            !accept(&event, Kind::ContactList, &requested),
            "an event from an unrequested author must be dropped"
        );
    }
}
