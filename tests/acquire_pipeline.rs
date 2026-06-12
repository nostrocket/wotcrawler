//! End-to-end proof of the relay -> ingest seam (plan 02-04, RELAY-03 + INGEST-01..05).
//!
//! This is the test that makes the Phase 2 goal — "only correct, deduplicated,
//! newest-wins follow lists emerge" — observable across the wired pipeline,
//! rather than only inside the two unconnected halves (plans 02-02 and 02-03).
//!
//! It drives an adversarially-polluted, multi-window relay stream through
//! [`web_of_trust::relay::acquire_validated_lists`] and asserts exactly the
//! correct deduped / newest-wins / self-drop-filtered follow list emerges:
//!
//! - The follower's events are split across a CAPPED first window and a SHORT
//!   second window, served through the 02-03 mock relay's scripted-window fetch
//!   fn under [`paginate_chunk`], so RELAY-03 page-back is exercised *inside the
//!   seam's fetch source* and the resolver must run across BOTH windows (T-02-15).
//! - A forged event, an unsolicited wrong-author event, and a future-dated
//!   event are injected into the served stream; all must be excluded from the
//!   emerged list (INGEST-01..03, T-02-14 — the wiring must not bypass the gate).
//! - A self-follow p-tag and a duplicate p-tag in the winning event must be
//!   dropped / collapsed (INGEST-04 / D-08).
//!
//! Offline: the relay is the injected scripted-window fetch fn (the documented
//! 02-03 alternative to a live websocket mock); no network, no Postgres.

mod common;
mod mock_relay;

use std::collections::HashSet;

use nostr_sdk::{Event, Kind, Timestamp};
use web_of_trust::relay::{acquire_validated_lists, fetch::paginate_chunk};

/// Drive the wired pipeline against the 02-03 mock relay across two paged
/// windows polluted with adversarial events; assert exactly the correct
/// `ValidatedFollowList` emerges.
#[tokio::test]
async fn pipeline_emerges_one_validated_list() -> anyhow::Result<()> {
    // --- Cast (deterministic pk(seed) fixtures, edge_diff idiom) -----------
    let follower = common::keys(10);
    let follower_pk = follower.public_key();
    let other = common::keys(99); // an author we never solicited.

    let f_a = common::keys(11).public_key();
    let f_b = common::keys(12).public_key();
    let f_c = common::keys(13).public_key();
    let self_pk = follower.public_key();

    // Two pinned timestamps so newest-wins is deterministic: the OLD event lands
    // in the capped first window, the NEW (winning) event in the short second
    // window. If the resolver only saw the first window it would pick the old
    // event's id/created_at — proving it ran across both paged windows.
    let now = Timestamp::now();
    let old_ts = Timestamp::from_secs(now.as_secs() - 1000);
    let new_ts = Timestamp::from_secs(now.as_secs() - 10);

    // The follower's OLDER kind-3 (loser): followees {a}.
    let follower_old = common::signed_event(&follower, Kind::ContactList, old_ts, &[f_a]);

    // The follower's NEWER kind-3 (winner): followees {a, b, b (dup), c, self}.
    // The dup must collapse and the self-follow must drop (INGEST-04 / D-08), so
    // the emerged set is exactly {a, b, c}.
    let follower_new = common::signed_event(
        &follower,
        Kind::ContactList,
        new_ts,
        &[f_a, f_b, f_b, f_c, self_pk],
    );
    let winner_id = follower_new.id;

    // --- Adversarial pollution (must all be excluded) ----------------------
    // Forged: content mutated after signing -> Event::verify() fails (INGEST-01).
    let forged = common::forged_event(&follower, Kind::ContactList, new_ts);
    // Unsolicited: a valid event from an author we never asked for (INGEST-01 /
    // Pitfall 4). Dated NEWER than the winner to prove author-gating, not timing,
    // excludes it.
    let unsolicited = common::signed_event(
        &other,
        Kind::ContactList,
        Timestamp::from_secs(now.as_secs() - 5),
        &[f_a, f_b],
    );
    // Future-dated: a valid signature but created_at a year ahead -> clamped
    // away (INGEST-03). Authored by the follower so only the clamp can exclude
    // it; if it leaked it would win newest-wins and corrupt the list.
    let future = common::future_dated_event(&follower, Kind::ContactList, 365 * 24 * 3600);

    // --- Serve across two windows so page-back (RELAY-03) is exercised -----
    // Window 1 is CAPPED (exactly `cap` events) so the pagination loop must page
    // back; window 2 is SHORT (fewer than `cap`) so the loop then stops. The
    // winning (newest) event sits in the SECOND window — only a resolver that saw
    // the full union picks it.
    let cap = 3usize;
    let window1: Vec<Event> = vec![follower_old.clone(), forged.clone(), unsolicited.clone()];
    assert_eq!(window1.len(), cap, "first window must be exactly the cap to force page-back");
    let window2: Vec<Event> = vec![follower_new.clone(), future.clone()];
    assert!(window2.len() < cap, "second window must be short to stop paging");

    let relay = mock_relay::ScriptedRelay::new(vec![window1, window2]);

    // --- The wired seam ----------------------------------------------------
    // The fetch source is the 02-03 scripted relay driven through the real
    // `paginate_chunk` page-back loop, so the seam consumes the genuine paged
    // union. `requested` is exactly the follower (NOT `other`), so the gate
    // drops the unsolicited author.
    let mut requested = HashSet::new();
    requested.insert(follower_pk);

    let mut fetch_fn = relay.fetch_fn();
    let results = acquire_validated_lists(
        &requested,
        Kind::ContactList,
        now,
        3600,   // future_clamp_secs
        50_000, // follow_cap
        || async move { paginate_chunk(&[follower_pk], Kind::ContactList, cap, &mut fetch_fn).await },
    )
    .await?;

    // --- Assertions --------------------------------------------------------
    // (a) Exactly one list emerges (the follower's; unsolicited author excluded).
    assert_eq!(
        results.len(),
        1,
        "exactly one ValidatedFollowList must emerge for the solicited follower"
    );
    let list = &results[0];
    assert_eq!(list.follower_pubkey, follower_pk);

    // (b) The followee set is the deduped, self-drop-filtered {a, b, c}: the
    // duplicate b collapsed, the self-follow dropped, and the forged /
    // unsolicited / future-dated events contributed NOTHING.
    let mut got: Vec<_> = list.followee_pubkeys.clone();
    got.sort();
    let mut expected = vec![f_a, f_b, f_c];
    expected.sort();
    assert_eq!(got, expected, "followees must be the deduped self-drop-filtered {{a,b,c}}");
    assert!(!list.followee_pubkeys.contains(&self_pk), "self-follow must be dropped (D-08)");

    // (c) The winning event is the NEWER one from the SECOND paged window —
    // proving the resolver ran across both windows (T-02-15) and newest-wins.
    assert_eq!(list.event_id, winner_id, "winner must be the newest in-clamp event (second window)");
    assert_eq!(
        list.created_at,
        web_of_trust::ingest::timestamp_to_datetime(new_ts),
        "created_at must be the newest in-clamp event's"
    );

    // Page-back actually happened: the relay saw two REQs, the second with an
    // older `until` than the first.
    let untils = relay.untils();
    assert_eq!(untils.len(), 2, "a capped first window must trigger a second (paged-back) REQ");
    let (u0, u1) = (untils[0], untils[1]);
    assert!(
        matches!((u0, u1), (Some(a), Some(b)) if b < a),
        "second REQ must page back to an older until than the first (got {u0:?} -> {u1:?})"
    );

    Ok(())
}
