//! Replaceable-event resolution tests (INGEST-03).
//!
//! Offline. Exercises `ingest::replaceable::pick_winner`: future-dated events
//! beyond the clamp are excluded and can never win; among in-range events the
//! highest `created_at` wins; same-`created_at` ties break to the LOWEST id.

mod common;

use nostr_sdk::{Kind, Timestamp};
use web_of_trust::ingest::replaceable::pick_winner;

/// A future-dated event beyond the clamp is excluded; the in-range event wins,
/// never the future one (Pitfall 2 — future-dated junk can't pin a list).
mod future_clamp {
    use super::*;

    #[test]
    fn future_dated_event_never_wins() {
        let author = common::keys(1);
        let now = Timestamp::now();

        // An in-range event (dated "now") and a future event one year ahead.
        let in_range = common::signed_event(&author, Kind::ContactList, now, &[]);
        let future = common::future_dated_event(&author, Kind::ContactList, 365 * 24 * 3600);

        // Future event has a strictly higher created_at, so a naive max() would
        // wrongly pick it. The clamp (1h) must exclude it.
        assert!(future.created_at > in_range.created_at);

        let candidates = [future.clone(), in_range.clone()];
        let winner = pick_winner(candidates.iter(), now, 3600)
            .expect("the in-range event must survive the clamp");
        // (candidates outlives `winner`)

        assert_eq!(
            winner.id, in_range.id,
            "the in-range event must win, never the future-dated one"
        );
    }

    /// When every candidate is future-dated beyond the clamp, no winner survives.
    #[test]
    fn all_future_dated_yields_none() {
        let author = common::keys(2);
        let now = Timestamp::now();
        let future = common::future_dated_event(&author, Kind::ContactList, 365 * 24 * 3600);

        let candidates = [future];
        assert!(
            pick_winner(candidates.iter(), now, 3600).is_none(),
            "no event may survive when all are future-dated"
        );
    }
}

/// Two events sharing the same `created_at` break the tie to the LOWEST id
/// (NIP-01 deterministic tie-break — prevents flapping between relays).
mod tie_break {
    use super::*;

    #[test]
    fn lowest_id_wins_on_equal_created_at() {
        let author = common::keys(3);
        let now = Timestamp::now();
        let (a, b) = common::same_created_at_pair(&author, now);

        // The lower of the two ids is the expected winner, regardless of input
        // order.
        let lower = std::cmp::min(a.id, b.id);

        let ab = [a.clone(), b.clone()];
        let ba = [b.clone(), a.clone()];
        let winner_ab = pick_winner(ab.iter(), now, 3600)
            .expect("a winner must exist")
            .id;
        let winner_ba = pick_winner(ba.iter(), now, 3600)
            .expect("a winner must exist")
            .id;

        assert_eq!(winner_ab, lower, "lowest id must win (a,b order)");
        assert_eq!(winner_ba, lower, "lowest id must win (b,a order) — order-independent");
    }
}

/// Among in-range events the highest `created_at` wins (newest-wins).
#[test]
fn newest_created_at_wins() {
    let author = common::keys(4);
    let now = Timestamp::now();

    let older = common::signed_event(
        &author,
        Kind::ContactList,
        Timestamp::from(now.as_secs() - 100),
        &[common::keys(40).public_key()],
    );
    let newer = common::signed_event(
        &author,
        Kind::ContactList,
        Timestamp::from(now.as_secs() - 10),
        &[common::keys(41).public_key()],
    );

    let candidates = [older.clone(), newer.clone()];
    let winner = pick_winner(candidates.iter(), now, 3600).expect("a winner must exist");
    assert_eq!(winner.id, newer.id, "the newest in-range event must win");
}
