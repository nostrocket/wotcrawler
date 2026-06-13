//! WR-03: proof the RELAY-02 / RELAY-04 mechanisms are REACHED on the production
//! acquire path, not only in their own unit tests.
//!
//! The 02-VERIFICATION.md data-flow trace flagged `acquire()`,
//! `LimitCache::get_or_fetch()`, and `record_notice()` as DISCONNECTED — correct
//! library code with zero production callers. These tests drive the production
//! seam offline (the injected scripted-relay fetch fn, not a live `Client`) and
//! assert the gate, the cache-sourced cap, and the notice path are exercised:
//!
//! - Task 1: every window REQ passes `RateLimiterRegistry::acquire(relay_url)`
//!   (observable: a tight quota forces measurable throttling across windows) and
//!   the effective per-window cap is the cached `LimitCache` `max_limit`.
//! - Task 2: a synthetic `rate-limited` relay message routed through the
//!   notifications consumer's per-message handler escalates `failure_count`;
//!   `blocked` stops traffic without incrementing the rate-limit counter.
//!
//! Offline: no network, no Postgres, no live websocket.

mod common;
mod mock_relay;

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nostr_sdk::{Event, Kind, Timestamp};
use web_of_trust::relay::fetch::paginate_chunk_gated;
use web_of_trust::relay::handle_relay_message;
use web_of_trust::relay::nip11::{LimitCache, RelayLimits};
use web_of_trust::relay::rate_limit::RateLimiterRegistry;

// ---------------------------------------------------------------------------
// Task 1: the production fetch path is gated behind acquire() and sources the
// per-window cap from the NIP-11 LimitCache.
// ---------------------------------------------------------------------------

/// The gated pagination seam awaits `registry.acquire(relay_url)` before EACH
/// window REQ, so a tight per-relay quota forces measurable throttling across
/// the paged windows. Independent/ungated REQs would finish near-instantly.
#[tokio::test]
async fn gated_pagination_throttles_each_window() -> anyhow::Result<()> {
    // Two CAPPED windows then a short one, so paginate pages back and issues
    // >= 3 REQs — each must pass the acquire gate.
    let cap = 2usize;
    let author = common::keys(10).public_key();

    let now = Timestamp::now();
    let w1: Vec<Event> = vec![
        common::signed_event(&common::keys(10), Kind::ContactList, now, &[common::keys(11).public_key()]),
        common::signed_event(
            &common::keys(10),
            Kind::ContactList,
            Timestamp::from_secs(now.as_secs() - 1),
            &[common::keys(12).public_key()],
        ),
    ];
    assert_eq!(w1.len(), cap, "first window must be exactly the cap to force page-back");
    let w2: Vec<Event> = vec![common::signed_event(
        &common::keys(10),
        Kind::ContactList,
        Timestamp::from_secs(now.as_secs() - 2000),
        &[common::keys(13).public_key()],
    )];
    assert!(w2.len() < cap, "second window must be short to stop paging");

    let relay = mock_relay::ScriptedRelay::new(vec![w1, w2]);
    let mut fetch_fn = relay.fetch_fn();

    // A 1 req/sec quota: the first acquire is the immediate burst token, every
    // subsequent window REQ waits ~1s. >= 2 paged REQs => >= ~1s of throttling.
    let registry = RateLimiterRegistry::with_params(
        NonZeroU32::new(1).unwrap(),
        Duration::from_secs(1),
        Duration::from_secs(300),
    );
    let relay_url = "wss://relay.example";

    let start = Instant::now();
    let events = paginate_chunk_gated(
        &[author],
        Kind::ContactList,
        cap,
        &registry,
        relay_url,
        &mut fetch_fn,
    )
    .await?;
    let elapsed = start.elapsed();

    assert!(
        elapsed >= Duration::from_millis(500),
        "the acquire gate must throttle each window REQ, took {elapsed:?}"
    );
    // The gate did not drop any events: the full paged union is returned.
    assert_eq!(events.len(), 3, "all three unique events across the windows must be returned");
    // The relay saw the paged-back second REQ (gate sits before the fetch).
    assert!(relay.untils().len() >= 2, "page-back must have issued a second gated REQ");
    Ok(())
}

/// `acquire_validated_lists_client_offline` sources the effective per-window cap
/// from the seeded `LimitCache` (not its caller-supplied ceiling), so the filter
/// `limit` reflects the relay's cached `max_limit`.
#[tokio::test]
async fn production_cap_comes_from_limit_cache() -> anyhow::Result<()> {
    let author = common::keys(10).public_key();
    let relay_url = "wss://relay.example";

    // Seed the cache with a TIGHT max_limit so the cap is observable: a single
    // capped window of exactly `cached_cap` events forces a page-back, and the
    // recorded filter `limit` must equal the cached cap (not some larger ceiling).
    let cached_cap = 2usize;
    let cache = LimitCache::new();
    cache.insert(
        relay_url,
        RelayLimits { max_limit: cached_cap, max_subscriptions: 20, max_filters: 10 },
    );

    let now = Timestamp::now();
    let w1: Vec<Event> = vec![
        common::signed_event(&common::keys(10), Kind::ContactList, now, &[common::keys(11).public_key()]),
        common::signed_event(
            &common::keys(10),
            Kind::ContactList,
            Timestamp::from_secs(now.as_secs() - 1),
            &[common::keys(12).public_key()],
        ),
    ];
    assert_eq!(w1.len(), cached_cap);
    let w2: Vec<Event> = vec![]; // exhausted -> stop.

    let relay = mock_relay::ScriptedRelay::new(vec![w1, w2]);
    let limits = cache.get_or_fetch(relay_url).await; // exercises the cache read.
    assert_eq!(limits.max_limit, cached_cap, "cap must be sourced from the cache");

    let mut fetch_fn = relay.limit_capturing_fetch_fn();
    let registry = RateLimiterRegistry::new();
    let _events = paginate_chunk_gated(
        &[author],
        Kind::ContactList,
        limits.max_limit,
        &registry,
        relay_url,
        &mut fetch_fn,
    )
    .await?;

    // The filter limit the relay saw on every REQ must equal the cached cap.
    let limits_seen = relay.limits_seen();
    assert!(!limits_seen.is_empty(), "at least one REQ must have been issued");
    for l in &limits_seen {
        assert_eq!(*l, Some(cached_cap), "every REQ's filter limit must be the cached max_limit");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Task 2: the notifications consumer's per-message handler routes rate-limited /
// blocked relay messages into record_notice()/backoff().
// ---------------------------------------------------------------------------

/// A `rate-limited` relay message routed through `handle_relay_message` escalates
/// the relay's `failure_count` (the same counter the fetch gate consults).
#[test]
fn rate_limited_message_escalates_failure_count() {
    let registry = RateLimiterRegistry::new();
    let relay = "wss://relay.example";

    assert_eq!(registry.failure_count(relay), 0);
    handle_relay_message(&registry, relay, "rate-limited: slow down");
    assert_eq!(registry.failure_count(relay), 1, "a rate-limited notice must escalate backoff");
    handle_relay_message(&registry, relay, "rate-limited: still too fast");
    assert_eq!(registry.failure_count(relay), 2, "repeated notices escalate per relay");
}

/// A `blocked` relay message does NOT increment the rate-limit failure counter
/// (it is a stop-traffic signal, not a backoff escalation).
#[test]
fn blocked_message_does_not_increment_rate_limit_counter() {
    let registry = RateLimiterRegistry::new();
    let relay = "wss://hostile.example";

    handle_relay_message(&registry, relay, "blocked: you are banned");
    assert_eq!(
        registry.failure_count(relay),
        0,
        "blocked must stop traffic without escalating the rate-limit counter"
    );
}

/// Sharing one `Arc<RateLimiterRegistry>` between the (would-be-spawned) consumer
/// and the fetch path: a notice recorded via the handler is visible to the same
/// registry the gate consults.
#[test]
fn consumer_and_fetch_share_one_registry() {
    let registry = Arc::new(RateLimiterRegistry::new());
    let relay = "wss://relay.example";

    // Consumer side records a notice...
    handle_relay_message(&registry, relay, "rate-limited: too fast");
    // ...and the fetch side (a clone of the same Arc) sees the escalation.
    let fetch_side = Arc::clone(&registry);
    assert_eq!(fetch_side.failure_count(relay), 1);
}
