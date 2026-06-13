//! RELAY-04: a `rate-limited` notice escalates per-relay backoff, `blocked`
//! stops traffic, and the governor gate throttles rapid acquisitions. No network.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use web_of_trust::relay::rate_limit::{
    backoff_delay_unjittered, classify_notice, NoticeKind, RateLimiterRegistry,
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

#[test]
fn backoff_saturates_at_cap_for_high_failure_counts() {
    // WR-01: at base=1s the unjittered factor is base_nanos (~2^30) << failures.
    // With a u128 checked_shl, shift counts in 119..=127 push the set bits past
    // bit 127, truncating the product to 0 and returning a ZERO delay — the
    // opposite of saturation (a zero-delay retry storm, threat T-02-20).
    // Saturating at failures >= 64 closes the entire window.
    let base = Duration::from_secs(1);
    let cap = Duration::from_secs(300);

    // Explicit point inside the old truncation window.
    assert_eq!(
        backoff_delay_unjittered(120, base, cap),
        cap,
        "failures=120 must saturate at cap, not truncate to zero"
    );

    // Sweep the whole high-failure range: never zero, never above cap.
    for failures in 64..=127u32 {
        let d = backoff_delay_unjittered(failures, base, cap);
        assert!(
            d != Duration::ZERO,
            "backoff at failures={failures} must never be zero (retry-storm guard)"
        );
        assert!(
            d <= cap,
            "backoff at failures={failures} must never exceed cap ({d:?} > {cap:?})"
        );
        assert_eq!(d, cap, "high failure counts must saturate at cap (failures={failures})");
    }

    // Low counts still grow exponentially (monotonic up to cap) and are non-zero.
    let d0 = backoff_delay_unjittered(0, base, cap);
    let d1 = backoff_delay_unjittered(1, base, cap);
    let d2 = backoff_delay_unjittered(2, base, cap);
    assert_eq!(d0, base);
    assert_eq!(d1, base * 2);
    assert_eq!(d2, base * 4);
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_acquires_share_one_limiter() {
    // CR-05: concurrent acquires on one relay must share a SINGLE GCRA limiter,
    // so aggregate outbound rate obeys the per-relay quota regardless of
    // concurrency. With the old remove/reinsert implementation each concurrent
    // caller found the slot vacant and minted a fresh full-burst limiter, so N
    // tasks fired ~immediately (quota multiplied by N — the politeness void).
    //
    // With a 1 req/sec shared limiter, N concurrent acquires must take at least
    // ~(N-1) seconds in aggregate (one immediate burst token, then one per
    // second). We require measurable throttling proportional to N.
    let reg = Arc::new(RateLimiterRegistry::with_params(
        NonZeroU32::new(1).unwrap(),
        Duration::from_secs(1),
        Duration::from_secs(300),
    ));
    let relay = "wss://relay.example";
    let n: u32 = 4;

    let start = Instant::now();
    let mut handles = Vec::new();
    for _ in 0..n {
        let reg = Arc::clone(&reg);
        handles.push(tokio::spawn(async move {
            reg.acquire(relay).await.unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    let elapsed = start.elapsed();

    // A single shared 1/sec limiter forces at least (n-1) replenish intervals for
    // the n concurrent demands. Allow slack but require the throttle to engage:
    // independent full-burst limiters would finish near-instantly (well under 1s).
    let lower = Duration::from_millis(((n - 1) as u64) * 1000 - 200);
    assert!(
        elapsed >= lower,
        "concurrent acquires must share one limiter (expected >= {lower:?} for \
         {n} tasks at 1/sec), took {elapsed:?} — a vacant-slot mint per caller \
         would finish nearly instantly"
    );
}
