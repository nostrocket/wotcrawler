//! RELAY-03: a capped first window triggers a SECOND page; the loop terminates
//! only when a window returns fewer than the cap; EOSE is never trusted as
//! completeness; no pubkeys are silently dropped across windows.
//!
//! Uses the injected-fetch-fn harness in `mock_relay` (the plan's documented
//! alternative to a full websocket relay mock) so the page-back decision is
//! exercised entirely offline.

mod mock_relay;

use nostr_sdk::Kind;
use web_of_trust::relay::fetch::{page_back, paginate_chunk};

use mock_relay::{event_at, ScriptedRelay};

#[test]
fn page_back_pages_on_capped_window_only() {
    // A window at/over the cap pages back to oldest-1; below the cap stops.
    let cap = 3;
    let oldest = nostr_sdk::Timestamp::from_secs(1_000);
    // Capped: page back.
    let next = page_back(cap, cap, Some(oldest)).expect("capped window must page back");
    assert_eq!(next.as_secs(), 999, "next until is oldest - 1");
    // Below cap: stop (EOSE/short window = genuine completion).
    assert!(page_back(cap - 1, cap, Some(oldest)).is_none());
    // At cap but no events seen (shouldn't happen, but defensive): stop.
    assert!(page_back(cap, cap, None).is_none());
}

#[tokio::test]
async fn capped_first_window_triggers_second_page() {
    let cap = 2;
    let authors = vec![
        nostr_sdk::Keys::generate().public_key(),
    ];

    // First window: exactly `cap` events (the relay may be silently truncating).
    // Second window: fewer than `cap` (genuine completion). EOSE follows each.
    let window1 = vec![event_at(1, 5_000), event_at(2, 4_000)]; // == cap
    let window2 = vec![event_at(3, 3_000)]; // < cap
    let relay = ScriptedRelay::new(vec![window1, window2]);

    let mut fetch = relay.fetch_fn();
    let events = paginate_chunk(&authors, Kind::ContactList, cap, |filter| {
        let fut = fetch(filter);
        async move { fut.await }
    })
    .await
    .expect("pagination must succeed");

    // The union spans BOTH windows — the capped first window did NOT end
    // pagination on its EOSE; the second (short) window did.
    assert_eq!(events.len(), 3, "events from both windows must be returned");

    // Exactly two REQs happened: the capped window forced a second fetch.
    let untils = relay.untils();
    assert_eq!(untils.len(), 2, "a capped window must trigger a second page");

    // The second REQ paged back: its `until` is strictly older than the first's
    // and equals the oldest event of window 1 minus one second.
    let first_until = untils[0].expect("first REQ carries an until");
    let second_until = untils[1].expect("second REQ carries an until");
    assert!(
        second_until < first_until,
        "the second window must page back to an older until ({second_until:?} < {first_until:?})"
    );
    assert_eq!(
        second_until.as_secs(),
        4_000 - 1,
        "page-back until is the oldest event of window 1 minus one second"
    );
}

#[tokio::test]
async fn short_first_window_does_not_page() {
    // A first window strictly under the cap is complete — no second REQ.
    let cap = 5;
    let authors = vec![nostr_sdk::Keys::generate().public_key()];
    let relay = ScriptedRelay::new(vec![vec![event_at(1, 9_000), event_at(2, 8_000)]]); // 2 < 5

    let mut fetch = relay.fetch_fn();
    let events = paginate_chunk(&authors, Kind::ContactList, cap, |filter| {
        let fut = fetch(filter);
        async move { fut.await }
    })
    .await
    .expect("pagination must succeed");

    assert_eq!(events.len(), 2);
    assert_eq!(relay.untils().len(), 1, "a short window must NOT trigger a second page");
}
