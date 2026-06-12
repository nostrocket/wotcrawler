//! RELAY-04: a `rate-limited` notice escalates per-relay backoff, `blocked`
//! stops traffic, and the governor gate throttles rapid acquisitions. No network.

use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use web_of_trust::relay::rate_limit::{
    classify_notice, NoticeKind, RateLimiterRegistry,
};

#[test]
fn classifies_machine_readable_prefixes() {
    assert_eq!(classify_notice("rate-limited: slow down"), NoticeKind::RateLimited);
    assert_eq!(classify_notice("blocked: spam"), NoticeKind::Blocked);
    assert_eq!(classify_notice("restricted: pay to play"), NoticeKind::Blocked);
    assert_eq!(classify_notice("error: something else"), NoticeKind::Other);
    assert_eq!(classify_notice("just a notice"), NoticeKind::Other);
}

#[test]
fn rate_limited_notice_escalates_backoff_per_relay() {
    let reg = RateLimiterRegistry::with_params(
        NonZeroU32::new(4).unwrap(),
        Duration::from_secs(1),
        Duration::from_secs(300),
    );
    let relay = "wss://relay.example";

    // Each successive rate-limited notice escalates the (pre-jitter) schedule.
    // We compare lower bounds since the returned delay carries jitter; the
    // failure count itself is the unambiguous escalation signal.
    let d1 = reg.record_notice(relay, "rate-limited: too fast").expect("rate-limited yields a delay");
    assert_eq!(reg.failure_count(relay), 1);
    let d2 = reg.record_notice(relay, "rate-limited: too fast").expect("rate-limited yields a delay");
    assert_eq!(reg.failure_count(relay), 2);
    let d3 = reg.record_notice(relay, "rate-limited: too fast").expect("rate-limited yields a delay");
    assert_eq!(reg.failure_count(relay), 3);

    // Lower bounds of the jitter window grow: f=0 -> [0.5s,1s], f=1 -> [1s,2s],
    // f=2 -> [2s,4s]. The maxima are strictly ordered, so the windows separate.
    assert!(d1 <= Duration::from_secs(1));
    assert!(d2 <= Duration::from_secs(2) && d2 >= Duration::from_secs(1));
    assert!(d3 <= Duration::from_secs(4) && d3 >= Duration::from_secs(2));

    // A successful fetch resets the relay's schedule.
    reg.reset(relay);
    assert_eq!(reg.failure_count(relay), 0);
}

#[test]
fn blocked_notice_stops_relay_and_yields_no_backoff() {
    let reg = RateLimiterRegistry::new();
    let relay = "wss://hostile.example";
    assert!(
        reg.record_notice(relay, "blocked: you are banned").is_none(),
        "a blocked relay must not be retried (no backoff delay)"
    );
    // blocked does not increment the rate-limit failure counter.
    assert_eq!(reg.failure_count(relay), 0);
}

#[test]
fn backoff_is_independent_per_relay() {
    let reg = RateLimiterRegistry::new();
    reg.record_notice("wss://a.example", "rate-limited: x");
    reg.record_notice("wss://a.example", "rate-limited: x");
    reg.record_notice("wss://b.example", "rate-limited: x");
    assert_eq!(reg.failure_count("wss://a.example"), 2);
    assert_eq!(reg.failure_count("wss://b.example"), 1);
}

#[tokio::test]
async fn governor_gate_throttles_rapid_acquisitions() {
    // A tight quota (1 req/sec) means the first acquisition is immediate and the
    // burst-exhausting follow-ups must wait. We assert the gate forces a wait
    // rather than letting unbounded REQs through.
    let reg = RateLimiterRegistry::with_params(
        NonZeroU32::new(1).unwrap(),
        Duration::from_secs(1),
        Duration::from_secs(300),
    );
    let relay = "wss://relay.example";

    let start = Instant::now();
    // First token is available immediately (initial burst capacity).
    reg.acquire(relay).await.unwrap();
    // The next two must be throttled by the 1/sec replenish; allow a generous
    // bound but require measurable throttling so the gate is proven to engage.
    reg.acquire(relay).await.unwrap();
    reg.acquire(relay).await.unwrap();
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(500),
        "the governor gate must throttle rapid acquisitions, took {elapsed:?}"
    );
}
