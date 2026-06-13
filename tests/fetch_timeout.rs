//! CR-02 / Pitfall 9: a window whose injected fetch elapses at or beyond the
//! per-fetch timeout must surface [`RelayError::FetchTimeout`] so the caller
//! requeues those authors — never recording the (partial) window as complete.
//!
//! nostr-relay-pool 0.44.1 drops the activity sender on timeout and the event
//! stream ends WITHOUT an error (the SDK returns a partial `Ok`), so an
//! independent elapsed-time check is the ONLY timeout signal. This test drives
//! that elapsed check offline through [`fetch_window_with_deadline`], a thin
//! wrapper over an injected async fetch fn that mirrors the per-window fetch
//! closure inside `fetch_complete_with_timeout`.

use std::time::Duration;

use nostr_sdk::{Event, Filter};
use web_of_trust::error::RelayError;
use web_of_trust::relay::fetch::fetch_window_with_deadline;

#[tokio::test]
async fn timed_out_window_requeues() {
    // A fetch that sleeps past the deadline returns an (empty/partial) Ok, but
    // the elapsed check must convert that into a FetchTimeout requeue signal.
    let timeout = Duration::from_millis(20);
    let relay_url = "wss://relay.example".to_string();

    let result = fetch_window_with_deadline(
        Filter::new(),
        timeout,
        &relay_url,
        |_filter| async move {
            tokio::time::sleep(Duration::from_millis(60)).await;
            Ok::<Vec<Event>, RelayError>(Vec::new())
        },
    )
    .await;

    match result {
        Err(RelayError::FetchTimeout(url)) => {
            assert_eq!(url, relay_url, "the timeout error carries the relay url label");
        }
        other => panic!("expected FetchTimeout requeue, got {other:?}"),
    }
}

#[tokio::test]
async fn fast_window_returns_events() {
    // A fetch that completes well within the deadline returns its events.
    let timeout = Duration::from_secs(5);
    let relay_url = "wss://relay.example".to_string();

    let result = fetch_window_with_deadline(
        Filter::new(),
        timeout,
        &relay_url,
        |_filter| async move { Ok::<Vec<Event>, RelayError>(Vec::new()) },
    )
    .await;

    assert!(
        result.is_ok(),
        "a window completing within the deadline must not be a timeout: {result:?}"
    );
}
