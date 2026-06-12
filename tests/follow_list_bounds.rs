//! Follow-list extraction bounds tests (INGEST-04).
//!
//! Offline. Exercises `ingest::follow_list::followee_pubkeys`:
//! - malformed/non-standard p-tags are skipped without panic;
//! - the follower's own pubkey is dropped (self-follow filtered);
//! - duplicate p-tags collapse;
//! - a list exceeding `follow_cap` is bounded (reject + count) without panic.
//!
//! Events are built directly with `EventBuilder` here (rather than the shared
//! `signed_event` fixture) because these tests need to inject malformed,
//! duplicate, and self p-tags that the fixture's `&[PublicKey]` signature can't
//! express.

mod common;

use nostr_sdk::{EventBuilder, Keys, Kind, Tag, Timestamp};
use web_of_trust::ingest::follow_list::followee_pubkeys;

/// Build a signed kind-3 event whose tags are exactly `tags`, authored by `signer`.
fn signed_with_tags(signer: &Keys, tags: Vec<Tag>) -> nostr_sdk::Event {
    EventBuilder::new(Kind::ContactList, "")
        .tags(tags)
        .custom_created_at(Timestamp::now())
        .sign_with_keys(signer)
        .expect("signing a fixture event must succeed")
}

/// A malformed/non-standard p-tag is skipped (no panic); valid p-tags survive.
mod malformed {
    use super::*;

    #[test]
    fn malformed_p_tag_is_skipped_without_panic() {
        let author = common::keys(1);
        let good = common::keys(2).public_key();

        // A valid p-tag, plus a malformed "p" tag (not 64-char hex) and a
        // non-p custom tag — both must be skipped by Tags::public_keys().
        let malformed_p = Tag::parse(["p", "not-a-valid-hex-pubkey"])
            .expect("a non-standard p-tag still parses into a raw Tag");
        let tags = vec![
            Tag::public_key(good),
            malformed_p,
            Tag::hashtag("notapubkey"),
        ];
        let event = signed_with_tags(&author, tags);

        let followees =
            followee_pubkeys(&event, 50_000).expect("extraction must not reject this list");

        assert_eq!(
            followees,
            vec![good],
            "only the well-formed p-tag pubkey survives; malformed tags are skipped"
        );
    }
}

/// A list larger than `follow_cap` is rejected (None) + counted, without panic.
mod cap {
    use super::*;

    #[test]
    fn oversized_list_is_rejected_without_panic() {
        let author = common::keys(3);

        // Build 10 distinct followees, cap at 5 -> reject.
        let tags: Vec<Tag> = (10u8..20)
            .map(|s| Tag::public_key(common::keys(s).public_key()))
            .collect();
        let event = signed_with_tags(&author, tags);

        let result = followee_pubkeys(&event, 5);
        assert!(
            result.is_none(),
            "a list exceeding follow_cap must be rejected (reject-not-truncate)"
        );
    }

    #[test]
    fn list_at_cap_is_accepted() {
        let author = common::keys(4);
        let tags: Vec<Tag> = (40u8..45)
            .map(|s| Tag::public_key(common::keys(s).public_key()))
            .collect();
        let event = signed_with_tags(&author, tags);

        let followees = followee_pubkeys(&event, 5).expect("a list exactly at the cap is accepted");
        assert_eq!(followees.len(), 5);
    }
}

/// The follower's own pubkey is dropped (D-08 self-follow defense in depth).
#[test]
fn self_follow_is_dropped() {
    let author = common::keys(5);
    let other = common::keys(6).public_key();

    let tags = vec![
        Tag::public_key(author.public_key()), // self
        Tag::public_key(other),
    ];
    let event = signed_with_tags(&author, tags);

    let followees = followee_pubkeys(&event, 50_000).expect("not oversized");
    assert_eq!(followees, vec![other], "the self-follow p-tag must be dropped");
}

/// Repeated p-tags collapse to a single followee (dedup).
#[test]
fn duplicate_p_tags_collapse() {
    let author = common::keys(7);
    let target = common::keys(8).public_key();

    let tags = vec![
        Tag::public_key(target),
        Tag::public_key(target),
        Tag::public_key(target),
    ];
    let event = signed_with_tags(&author, tags);

    let followees = followee_pubkeys(&event, 50_000).expect("not oversized");
    assert_eq!(
        followees,
        vec![target],
        "repeated p-tags must deduplicate to one followee"
    );
}
