//! RELAY-03: a capped first window triggers a SECOND page; the loop terminates
//! only when a window returns fewer than the cap; EOSE is never trusted as
//! completeness; no pubkeys are silently dropped across windows.
//!
//! Uses the injected-fetch-fn harness in `mock_relay` (the plan's documented
//! alternative to a full websocket relay mock) so the page-back decision is
//! exercised entirely offline.

mod mock_relay;

use nostr_sdk::Kind;
use web_of_trust::error::RelayError;
use web_of_trust::relay::fetch::{page_back, paginate_chunk, MAX_PAGES_PER_CHUNK};

use mock_relay::{event_at, ScriptedRelay};

#[test]
fn page_back_pages_on_capped_window_only() {
    // A window at/over the cap pages back to oldest (INCLUSIVE); below cap stops.
    // CR-03: paging back exclusively (oldest - 1) drops any event sharing the
    // oldest event's second that the relay's cap cut off, leaving a permanent
    // hole at the boundary second. Page back INCLUSIVELY to `oldest`; the loop's
    // cross-window dedup discards the re-seen oldest event and the zero-new-id
    // guard stops a genuinely exhausted chunk.
    let cap = 3;
    let oldest = nostr_sdk::Timestamp::from_secs(1_000);
    // Capped: page back to oldest INCLUSIVE.
    let next = page_back(cap, cap, Some(oldest)).expect("capped window must page back");
    assert_eq!(next.as_secs(), 1_000, "next until is oldest (inclusive)");
    // Below cap: stop (EOSE/short window = genuine completion).
    assert!(page_back(cap - 1, cap, Some(oldest)).is_none());
    // At cap but no events seen (shouldn't happen, but defensive): stop.
    assert!(page_back(cap, cap, None).is_none());
}

#[tokio::test]
async fn inclusive_boundary_keeps_boundary_event() {
    // CR-03: the second window's `until` equals the oldest event's second
    // (NOT oldest - 1), and a boundary-second event cut by the cap is recovered.
    let cap = 2;
    let authors = vec![nostr_sdk::Keys::generate().public_key()];

    // Window 1 is capped (== cap); its oldest event sits at second 4_000.
    // A SECOND event also lives at second 4_000 but was cut off by the cap.
    let boundary_a = event_at(2, 4_000); // oldest of window 1
    let boundary_b = event_at(9, 4_000); // same second, cut by the cap
    let window1 = vec![event_at(1, 5_000), boundary_a.clone()]; // == cap
    let window2 = vec![boundary_a.clone(), boundary_b.clone()]; // boundary re-served + the cut event
    let window3 = vec![]; // genuine exhaustion

    let relay = ScriptedRelay::new(vec![window1, window2, window3]);
    let mut fetch = relay.fetch_fn();
    let events = paginate_chunk(&authors, Kind::ContactList, cap, |filter| {
        let fut = fetch(filter);
        async move { fut.await }
    })
    .await
    .expect("pagination must succeed");

    let untils = relay.untils();
    let second_until = untils[1].expect("second REQ carries an until");
    assert_eq!(
        second_until.as_secs(),
        4_000,
        "page-back until is the oldest event's second (inclusive), not oldest - 1"
    );

    // The boundary-second event cut by the cap is present exactly once.
    let ids: Vec<_> = events.iter().map(|e| e.id).collect();
    assert!(
        ids.contains(&boundary_b.id),
        "the boundary-second event cut by the cap must be recovered"
    );
}

#[tokio::test]
async fn cross_window_dedup_keeps_each_event_once() {
    // CR-03/CR-04: the oldest event of window 1 reappears at the inclusive
    // boundary of window 2; the final union contains it exactly once.
    let cap = 2;
    let authors = vec![nostr_sdk::Keys::generate().public_key()];

    let shared_oldest = event_at(2, 4_000); // appears in BOTH windows
    let window1 = vec![event_at(1, 5_000), shared_oldest.clone()]; // == cap
    let window2 = vec![shared_oldest.clone(), event_at(3, 3_000)]; // boundary re-served + one new
    let window3 = vec![]; // exhaustion

    let relay = ScriptedRelay::new(vec![window1, window2, window3]);
    let mut fetch = relay.fetch_fn();
    let events = paginate_chunk(&authors, Kind::ContactList, cap, |filter| {
        let fut = fetch(filter);
        async move { fut.await }
    })
    .await
    .expect("pagination must succeed");

    let count = events.iter().filter(|e| e.id == shared_oldest.id).count();
    assert_eq!(count, 1, "the re-served boundary event must appear exactly once");
    assert_eq!(events.len(), 3, "union = 3 distinct events across windows");
}

#[tokio::test]
async fn zero_new_id_window_stops_even_when_capped() {
    // CR-04: a capped window contributing only already-seen ids is genuine
    // exhaustion (the boundary re-request returned nothing new) — stop, do NOT
    // keep paging on the >= cap signal alone.
    let cap = 2;
    let authors = vec![nostr_sdk::Keys::generate().public_key()];

    let a = event_at(1, 5_000);
    let b = event_at(2, 4_000);
    let window1 = vec![a.clone(), b.clone()]; // == cap
    // Window 2 is also == cap but every id is already seen -> zero new ids.
    let window2 = vec![a.clone(), b.clone()];

    let relay = ScriptedRelay::new(vec![window1, window2]);
    let mut fetch = relay.fetch_fn();
    let events = paginate_chunk(&authors, Kind::ContactList, cap, |filter| {
        let fut = fetch(filter);
        async move { fut.await }
    })
    .await
    .expect("pagination must succeed");

    assert_eq!(events.len(), 2, "no new ids in window 2 -> union stays at 2");
    assert_eq!(
        relay.untils().len(),
        2,
        "exactly two REQs: window 2's zero-new-id result stops paging"
    );
}

#[tokio::test]
async fn budget_guard_errors_on_adversarial_relay() {
    // CR-04 / T-02-16: a relay that always returns a full-cap window of NEW ids
    // ignoring `until` must NOT loop forever — paginate_chunk errors after
    // MAX_PAGES_PER_CHUNK pages.
    let cap = 2;
    let authors = vec![nostr_sdk::Keys::generate().public_key()];

    // Build MAX_PAGES_PER_CHUNK + 2 windows, each with distinct (new) ids, so
    // the new-id-progress guard never trips and only the budget can stop it.
    let mut windows: Vec<Vec<nostr_sdk::Event>> = Vec::new();
    let total = MAX_PAGES_PER_CHUNK + 2;
    for i in 0..total {
        let base = 1_000_000 - (i as u64) * 10;
        windows.push(vec![
            event_at((2 * i % 250) as u8 + 1, base),
            event_at((2 * i % 250) as u8 + 2, base - 1),
        ]);
    }

    let relay = ScriptedRelay::new(windows);
    let mut fetch = relay.fetch_fn();
    let result = paginate_chunk(&authors, Kind::ContactList, cap, |filter| {
        let fut = fetch(filter);
        async move { fut.await }
    })
    .await;

    assert!(
        matches!(result, Err(RelayError::FetchTimeout(_))),
        "an adversarial always-full relay must hit the MAX_PAGES_PER_CHUNK budget, got {result:?}"
    );
    assert_eq!(
        relay.untils().len(),
        MAX_PAGES_PER_CHUNK,
        "the loop must stop at exactly MAX_PAGES_PER_CHUNK fetches"
    );
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
        4_000,
        "page-back until is the oldest event of window 1 (inclusive, CR-03)"
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
