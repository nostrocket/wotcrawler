//! RELAY-01: the curated-set connect path applies a reconnect policy, and the
//! app-side fetch-re-arm backoff is capped-exponential-with-jitter.
//!
//! These assert on the *policy*, not on live sockets — no network required. The
//! SDK socket reconnect is linear (02-SPIKES); the exponential-with-jitter
//! requirement is met by [`web_of_trust::relay::rate_limit::backoff_delay`],
//! which is what RELAY-01 actually mandates, so the schedule is asserted here.

use std::time::Duration;

use web_of_trust::relay::rate_limit::{backoff_delay, backoff_delay_unjittered};
use web_of_trust::relay::ReconnectPolicy;

#[test]
fn crawler_default_enables_reconnect() {
    let policy = ReconnectPolicy::crawler_default();
    assert!(policy.reconnect, "the crawler must auto-reconnect dropped relays");
    assert!(
        policy.adjust_retry_interval,
        "adaptive retry must stay on so repeated failures don't hammer a relay"
    );
    assert_eq!(policy.retry_interval, Duration::from_secs(10));
}

#[test]
fn app_side_backoff_grows_exponentially() {
    let base = Duration::from_secs(1);
    let cap = Duration::from_secs(300);

    // The pre-jitter schedule doubles each failure: 1s, 2s, 4s, 8s, ...
    assert_eq!(backoff_delay_unjittered(0, base, cap), Duration::from_secs(1));
    assert_eq!(backoff_delay_unjittered(1, base, cap), Duration::from_secs(2));
    assert_eq!(backoff_delay_unjittered(2, base, cap), Duration::from_secs(4));
    assert_eq!(backoff_delay_unjittered(3, base, cap), Duration::from_secs(8));

    // Strictly increasing until the cap (exponential, not linear).
    let mut prev = Duration::ZERO;
    for f in 0..8 {
        let d = backoff_delay_unjittered(f, base, cap);
        assert!(d > prev, "delay must grow with failure count (f={f}, d={d:?})");
        prev = d;
    }
}

#[test]
fn app_side_backoff_saturates_at_cap() {
    let base = Duration::from_secs(1);
    let cap = Duration::from_secs(300);
    // Far past the cap: 2^20 * 1s is clamped to 300s, and the shift never panics.
    assert_eq!(backoff_delay_unjittered(20, base, cap), cap);
    assert_eq!(backoff_delay_unjittered(63, base, cap), cap);
    assert_eq!(backoff_delay_unjittered(255, base, cap), cap);
}

#[test]
fn jitter_desynchronizes_relays() {
    // Two relays at the same failure count must (almost surely) get different
    // jittered delays, so they never re-arm in lockstep (Pitfall 8). With full
    // jitter over a 2s span the collision probability across many draws is
    // vanishingly small.
    let base = Duration::from_secs(1);
    let cap = Duration::from_secs(300);
    let samples: Vec<Duration> = (0..16).map(|_| backoff_delay(2, base, cap)).collect();
    let all_identical = samples.iter().all(|d| *d == samples[0]);
    assert!(!all_identical, "jitter must vary the delay across relays/draws");

    // Jitter stays within the documented [delay/2, delay] window.
    let unjittered = backoff_delay_unjittered(2, base, cap);
    for d in samples {
        assert!(d >= unjittered / 2, "jittered delay below lower bound: {d:?}");
        assert!(d <= unjittered, "jittered delay above pre-jitter delay: {d:?}");
    }
}
