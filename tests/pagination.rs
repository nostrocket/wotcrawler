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

use std::cell::RefCell;
use std::rc::Rc;

use mock_relay::{event_at, prefix_for_until_fetch_fn, ScriptedRelay};

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
    let fetch = relay.fetch_fn();
    let events = paginate_chunk(&authors, Kind::ContactList, cap, fetch)
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
    let fetch = relay.fetch_fn();
    let events = paginate_chunk(&authors, Kind::ContactList, cap, fetch)
    .await
    .expect("pagination must succeed");

    let count = events.iter().filter(|e| e.id == shared_oldest.id).count();
    assert_eq!(count, 1, "the re-served boundary event must appear exactly once");
    assert_eq!(events.len(), 3, "union = 3 distinct events across windows");
}

#[tokio::test]
async fn capped_reserved_prefix_at_pinned_boundary_surfaces_error() {
    // RECONCILED for CR-01-new (was `zero_new_id_window_stops_even_when_capped`).
    // Window 1 = [A(5000), B(4000)] == cap -> page_back(2,2,Some(4000)) pins
    // until=4000. Window 2 re-serves the SAME [A, B] == cap with every id already
    // seen (new_ids=0), and its oldest is 4000 == current_until, so
    // page_back(2,2,Some(4000)) == Some(4000) == Some(current_until): the relay is
    // re-serving the same cap-sized prefix for the pinned boundary second while
    // (per the strengthened invariant) a sibling could remain cut at second 4000.
    //
    // The corrected invariant — "a capped re-served-prefix at a pinned boundary
    // is a stall, never silent completion" — means this is the boundary-second
    // stall, NOT genuine exhaustion: paginate_chunk must surface a requeue Err on
    // this FIRST capped zero-new-id re-request rather than complete Ok([A, B]).
    // The prior Ok/len==2 expectation was itself an instance of the silent-
    // truncation bug (CR-01-new) and is updated to expect Err.
    let cap = 2;
    let authors = vec![nostr_sdk::Keys::generate().public_key()];

    let a = event_at(1, 5_000);
    let b = event_at(2, 4_000);
    let window1 = vec![a.clone(), b.clone()]; // == cap
    // Window 2 is also == cap but every id is already seen -> zero new ids, and
    // its oldest (4000) equals the pinned until -> page_back re-pins -> stall.
    let window2 = vec![a.clone(), b.clone()];

    let relay = ScriptedRelay::new(vec![window1, window2]);
    let fetch = relay.fetch_fn();
    let result = paginate_chunk(&authors, Kind::ContactList, cap, fetch).await;

    assert!(
        matches!(result, Err(RelayError::FetchTimeout(_))),
        "a capped window re-serving the same prefix at a pinned boundary second \
         is a stall (no silent truncated Ok), got {result:?}"
    );
    assert_eq!(
        relay.untils().len(),
        2,
        "exactly two REQs: window 2's capped zero-new-id re-request surfaces the stall"
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
    let fetch = relay.fetch_fn();
    let result = paginate_chunk(&authors, Kind::ContactList, cap, fetch)
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
async fn deterministic_boundary_stall_surfaces_error() {
    // CR-03 residual (02-VERIFICATION.md gap #1): a DETERMINISTIC newest-first
    // relay that caps at the boundary second. Three events A(T), B(T), C(T) all
    // share the oldest second T (plus one newer N(T+1)); with cap=2 the relay
    // NEVER volunteers the third sibling for a pinned until=T — it re-serves the
    // SAME cap-sized prefix. Unlike inclusive_boundary_keeps_boundary_event,
    // this test does NOT hand-feed the cut sibling: it models real relay
    // behavior via prefix_for_until_fetch_fn over a fixed pool.
    //
    // Trace (cap=2):
    //   Window 1 (until=now): [N(T+1), A(T)] -> capped, oldest=T -> page back to until=T.
    //   Window 2 (until=T):   [A(T), B(T)]   -> new_ids=1 (B), still capped, until pinned at T.
    //   Window 3 (until=T):   [A(T), B(T)]   -> new_ids=0, until STILL pinned at T, STILL capped.
    // The third sibling C(T) is never served. The loop must NOT silently
    // complete with a truncated Ok that omits C(T): it must surface the
    // unresolvable boundary-second stall as a requeue Err.
    let cap = 2;
    let authors = vec![nostr_sdk::Keys::generate().public_key()];

    // T is the shared boundary second; N is one second newer.
    let t = 4_000u64;
    let a = event_at(1, t);
    let b = event_at(2, t);
    let c = event_at(3, t); // the sibling cut by the cap — never served
    let n = event_at(4, t + 1);
    // Order the pool so the newest cap=2 at-or-before T are deterministically
    // A then B (C is the third, never reached). event seeds differ so ids
    // differ; created_at ties at T are broken by the sort's stable-ish order,
    // but we only need the relay to consistently expose A,B (not C) for until=T.
    let pool = vec![n.clone(), a.clone(), b.clone(), c.clone()];

    let untils: mock_relay::UntilLog = Rc::new(RefCell::new(Vec::new()));
    let fetch = prefix_for_until_fetch_fn(pool, cap, Rc::clone(&untils));

    let result = paginate_chunk(&authors, Kind::ContactList, cap, fetch).await;

    // POST-FIX: the unresolvable stall surfaces as a requeue Err, never a
    // silent truncated Ok that omits C(T). PRE-FIX this FAILS (Ok returned).
    assert!(
        matches!(result, Err(_)),
        "a deterministic relay re-serving the same cap-sized prefix for a pinned \
         until=T (more events remaining) must surface a requeue Err, got {result:?}"
    );

    // The loop must have pinned until=T across the stall iterations (not advanced).
    let seen = untils.borrow();
    let pinned_at_t = seen
        .iter()
        .filter(|u| matches!(u, Some(ts) if ts.as_secs() == t))
        .count();
    assert!(
        pinned_at_t >= 2,
        "the loop must re-issue until=T at least twice (pinned boundary stall), saw {seen:?}"
    );
}

#[tokio::test]
async fn no_newer_event_boundary_stall_surfaces_error() {
    // CR-01-new (02-VERIFICATION.md gaps_remaining, RELAY-03): the companion to
    // deterministic_boundary_stall_surfaces_error. The ONLY structural difference
    // is that there is NO event newer than the boundary second T — every pool
    // event shares T (no N(T+1)). This is the path 02-10's prev_until 2-visit
    // guard leaves open: when until first becomes T, prev_until is Some(now), so
    // `prev_until == Some(current_until)` is FALSE on the second window and the
    // zero-new-id result is misclassified as genuine exhaustion, silently dropping
    // the third sibling C(T).
    //
    // Trace (cap=2), pool = [A(T), B(T), C(T)], all at second T, no newer event:
    //   Window 1 (until=now): newest cap=2 at-or-before now = [A(T), B(T)];
    //     oldest=T; capped; page_back -> Some(T); until=T; prev_until=Some(now).
    //   Window 2 (until=T):   [A(T), B(T)] again; new_ids=0; returned(2) >= cap(2);
    //     prev_until is Some(now) != Some(T) so the OLD 2-visit guard does NOT fire.
    // The third sibling C(T) is never served. The loop must NOT silently complete
    // with a truncated Ok([A, B]) that omits C(T): it must surface the boundary
    // stall as a requeue Err on the FIRST capped zero-new-id re-request of until=T
    // (page_back would re-pin the same until). PRE-FIX this FAILS (silent Ok).
    let cap = 2;
    let authors = vec![nostr_sdk::Keys::generate().public_key()];

    // T is the shared boundary second; NO event is newer than T (the sole
    // structural difference from deterministic_boundary_stall_surfaces_error).
    let t = 4_000u64;
    let a = event_at(1, t);
    let b = event_at(2, t);
    let c = event_at(3, t); // the sibling cut by the cap — never served
    let pool = vec![a.clone(), b.clone(), c.clone()];

    let untils: mock_relay::UntilLog = Rc::new(RefCell::new(Vec::new()));
    let fetch = prefix_for_until_fetch_fn(pool, cap, Rc::clone(&untils));

    let result = paginate_chunk(&authors, Kind::ContactList, cap, fetch).await;

    // POST-FIX: the no-newer-event boundary stall surfaces as a requeue Err, never
    // a silent truncated Ok([A, B]) that omits C(T). We do NOT assert WHICH two of
    // the three same-second events the prefix returns (that depends on the stable
    // newest-first sort over equal created_at) — only that the result is Err.
    assert!(
        result.is_err(),
        "a deterministic relay with NO event newer than the boundary second, \
         re-serving the same cap-sized prefix for a pinned until=T, must surface \
         a requeue Err rather than a silent truncated Ok, got {result:?}"
    );

    // The boundary second T must have been re-requested at least once.
    let seen = untils.borrow();
    let pinned_at_t = seen
        .iter()
        .filter(|u| matches!(u, Some(ts) if ts.as_secs() == t))
        .count();
    assert!(
        pinned_at_t >= 1,
        "the loop must re-issue until=T at least once (boundary second pinned), saw {seen:?}"
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

    let fetch = relay.fetch_fn();
    let events = paginate_chunk(&authors, Kind::ContactList, cap, fetch)
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

    let fetch = relay.fetch_fn();
    let events = paginate_chunk(&authors, Kind::ContactList, cap, fetch)
    .await
    .expect("pagination must succeed");

    assert_eq!(events.len(), 2);
    assert_eq!(relay.untils().len(), 1, "a short window must NOT trigger a second page");
}
