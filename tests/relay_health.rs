//! Offline `RelayHealthRegistry` EWMA / routing / permit unit tests (RELAY-06, 05-02).
//!
//! Pure-unit (no DB, no network): every test exercises the public
//! [`RelayHealthRegistry`] API directly and asserts the score/permit/probe
//! behaviors. Mirrors the offline `#[test]` style of `tests/daemon_config.rs`
//! and the pure-fn invariants `rate_limit.rs` proves.
//!
//! ```text
//! SQLX_OFFLINE=true cargo test --test relay_health -- --test-threads=2
//! ```

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;
use web_of_trust::relay::health::{
    admit_per_relay, RelayHealthRegistry, DEFAULT_HEALTH_ALPHA, DEFAULT_PER_RELAY_CONCURRENCY,
    DEFAULT_RELAY_HEALTH_THRESHOLD,
};

const RELAY: &str = "wss://relay.example.com";

/// An unknown relay is healthy (1.0); a fast success keeps it near 1.0; a
/// timeout / connect-failure drives it toward 0; a rate-limit hit degrades it
/// but never zeroes it.
#[test]
fn ewma_moves_with_signals() {
    let reg = RelayHealthRegistry::new(DEFAULT_HEALTH_ALPHA);

    // Unknown relay defaults to fully healthy.
    assert_eq!(reg.score(RELAY), 1.0, "unknown relay scores 1.0");

    // A fast success keeps the score essentially at 1.0 (sample ~1.0).
    reg.record_success(RELAY, Duration::from_millis(10));
    assert!(
        reg.score(RELAY) > 0.99,
        "fast success keeps the score near 1.0, got {}",
        reg.score(RELAY)
    );

    // A slow-but-successful fetch yields a sample well below 1.0, so the score
    // dips beneath the fast-success score.
    let slow = RelayHealthRegistry::new(DEFAULT_HEALTH_ALPHA);
    slow.record_success(RELAY, Duration::from_secs(9));
    assert!(
        slow.score(RELAY) < 0.95,
        "a 9s success is penalized below 0.95, got {}",
        slow.score(RELAY)
    );

    // Repeated timeouts drive the score toward 0.
    let down = RelayHealthRegistry::new(DEFAULT_HEALTH_ALPHA);
    for _ in 0..20 {
        down.record_timeout(RELAY);
    }
    assert!(
        down.score(RELAY) < 0.05,
        "repeated timeouts drive the score toward 0, got {}",
        down.score(RELAY)
    );

    // Connect failures behave like timeouts (sample 0.0).
    let conn = RelayHealthRegistry::new(DEFAULT_HEALTH_ALPHA);
    for _ in 0..20 {
        conn.record_connect_failure(RELAY);
    }
    assert!(
        conn.score(RELAY) < 0.05,
        "repeated connect failures drive the score toward 0, got {}",
        conn.score(RELAY)
    );

    // Rate-limit hits degrade but never zero: the EWMA floor of a sustained 0.2
    // signal is 0.2, so the score settles at ~0.2, strictly above 0.
    let limited = RelayHealthRegistry::new(DEFAULT_HEALTH_ALPHA);
    for _ in 0..50 {
        limited.record_rate_limited(RELAY);
    }
    let s = limited.score(RELAY);
    assert!(
        s > 0.0 && s < 0.3,
        "sustained rate-limit hits settle near 0.2 (degrade, not zero), got {s}"
    );
}

/// A relay below the threshold is skipped until a probe is due; once a probe is
/// due `route_allowed` flips true again, and a recovered (>= threshold) relay is
/// always allowed.
#[test]
fn skip_then_probe() {
    let reg = RelayHealthRegistry::new(DEFAULT_HEALTH_ALPHA);
    let threshold = DEFAULT_RELAY_HEALTH_THRESHOLD;

    // Healthy relay: always routable.
    assert!(
        reg.route_allowed(RELAY, threshold),
        "a healthy (1.0) relay is routable"
    );

    // Drive the relay below the threshold via repeated timeouts.
    for _ in 0..20 {
        reg.record_timeout(RELAY);
    }
    assert!(
        reg.score(RELAY) < threshold,
        "the relay is now below threshold, got {}",
        reg.score(RELAY)
    );

    // Record an attempt NOW so a probe is not yet due (the probe interval has
    // not elapsed): the degraded relay is skipped.
    reg.mark_attempt(RELAY);
    assert!(
        !reg.route_allowed(RELAY, threshold),
        "a degraded relay with a fresh attempt is skipped (no probe due yet)"
    );

    // A relay that has never been attempted (last_probe == never) is treated as
    // probe-due, so even below threshold it is allowed one probe.
    let fresh = RelayHealthRegistry::new(DEFAULT_HEALTH_ALPHA);
    for _ in 0..20 {
        fresh.record_timeout(RELAY);
    }
    assert!(
        fresh.route_allowed(RELAY, threshold),
        "a never-probed degraded relay is allowed a probe"
    );
}

/// Permits scale with health: `max(1, round(per_relay_concurrency * score))`,
/// and a near-zero score still yields at least one permit.
#[test]
fn permits_scale_with_health() {
    let cap = DEFAULT_PER_RELAY_CONCURRENCY;

    // Fully healthy relay (1.0): full permit count.
    let healthy = RelayHealthRegistry::new(DEFAULT_HEALTH_ALPHA);
    assert_eq!(
        healthy.permits(RELAY, cap),
        cap,
        "a fully healthy relay gets the full per-relay concurrency"
    );

    // A degraded relay gets proportionally fewer permits but never below 1.
    let degraded = RelayHealthRegistry::new(DEFAULT_HEALTH_ALPHA);
    for _ in 0..40 {
        degraded.record_timeout(RELAY);
    }
    let p = degraded.permits(RELAY, cap);
    assert_eq!(
        p, 1,
        "a near-zero-score relay still keeps exactly one probe permit, got {p}"
    );

    // The exact rounding contract: score ~0.5 with cap 4 → round(2.0) == 2.
    // Build a registry whose alpha makes a single rate-limit (sample 0.2) move
    // 1.0 → a value we can check the rounding against directly via a known math.
    let mid = RelayHealthRegistry::new(1.0); // alpha 1.0 => score == last sample
    mid.record_rate_limited(RELAY); // score becomes exactly 0.2
    assert_eq!(
        mid.permits(RELAY, cap),
        1,
        "round(4 * 0.2) == round(0.8) == 1"
    );
    mid.record_success(RELAY, Duration::from_millis(0)); // score becomes exactly 1.0
    assert_eq!(mid.permits(RELAY, cap), cap, "score back to 1.0 => full permits");

    // in_use bookkeeping: incr/decr track concurrent admissions.
    let reg = RelayHealthRegistry::new(DEFAULT_HEALTH_ALPHA);
    assert_eq!(reg.in_use(RELAY), 0, "no admissions yet");
    reg.incr_in_use(RELAY);
    reg.incr_in_use(RELAY);
    assert_eq!(reg.in_use(RELAY), 2, "two in-flight admissions");
    reg.decr_in_use(RELAY);
    assert_eq!(reg.in_use(RELAY), 1, "one admission completed");
}

/// CR-02 regression: under concurrency, exactly ONE task may win the probe for a
/// degraded relay per probe interval. The old `route_allowed` + `mark_attempt`
/// pair released its lock between the read and the write, so every concurrent
/// caller saw the probe as "due" and all of them probed at once. The atomic
/// `try_mark_attempt` holds the `last_probe` lock across the read-and-claim, so
/// only one caller returns `true`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn try_mark_attempt_single_probe_under_concurrency() {
    let threshold = DEFAULT_RELAY_HEALTH_THRESHOLD;

    // Drive the relay below threshold so routing is probe-gated. The relay has
    // never been probed, so the FIRST caller to win claims the only probe slot
    // for this interval.
    let reg = Arc::new(RelayHealthRegistry::new(DEFAULT_HEALTH_ALPHA));
    for _ in 0..20 {
        reg.record_timeout(RELAY);
    }
    assert!(
        reg.score(RELAY) < threshold,
        "relay must be below threshold for the probe-gated path, got {}",
        reg.score(RELAY)
    );

    // 32 concurrent callers race try_mark_attempt for the same degraded relay.
    let mut handles = Vec::new();
    for _ in 0..32 {
        let reg = Arc::clone(&reg);
        handles.push(tokio::spawn(
            async move { reg.try_mark_attempt(RELAY, threshold) },
        ));
    }
    let mut winners = 0usize;
    for h in handles {
        if h.await.expect("probe task did not panic") {
            winners += 1;
        }
    }
    assert_eq!(
        winners, 1,
        "exactly one concurrent caller may win the probe for a degraded relay, got {winners}"
    );

    // A healthy relay is always allowed (no probe gating) — every caller wins.
    let healthy = Arc::new(RelayHealthRegistry::new(DEFAULT_HEALTH_ALPHA));
    let mut handles = Vec::new();
    for _ in 0..8 {
        let healthy = Arc::clone(&healthy);
        handles.push(tokio::spawn(
            async move { healthy.try_mark_attempt(RELAY, threshold) },
        ));
    }
    for h in handles {
        assert!(
            h.await.expect("task did not panic"),
            "a healthy relay is always routable for every caller"
        );
    }
}

/// CR-01 regression: `admit_per_relay` must not live-lock under full saturation.
///
/// With the OLD ordering (acquire the semaphore permit BEFORE the in_use spin
/// loop), `per_relay_concurrency` spinners would hold every permit while waiting
/// for in_use to drop, but no in-flight fetch could complete to decrement it —
/// because completing requires a permit that the spinners hold. The fix acquires
/// the permit AFTER the in_use admission check, so a waiter never holds a permit
/// while waiting. Many more tasks than permits all complete within a bounded
/// timeout; a hang here is the live-lock.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admit_per_relay_no_livelock_under_saturation() {
    const PER_RELAY_CONCURRENCY: usize = 2;
    const TASKS: usize = 16;

    let health = Arc::new(RelayHealthRegistry::new(DEFAULT_HEALTH_ALPHA));
    // The hard ceiling equals per_relay_concurrency, exactly as the daemon builds
    // it — the worst case for the old permit-held-while-spinning ordering.
    let sem = Arc::new(Semaphore::new(PER_RELAY_CONCURRENCY));

    let mut handles = Vec::new();
    for _ in 0..TASKS {
        let health = Arc::clone(&health);
        let sem = Arc::clone(&sem);
        handles.push(tokio::spawn(async move {
            admit_per_relay(&health, &sem, RELAY, PER_RELAY_CONCURRENCY, || async {
                // A tiny amount of work so several tasks contend on the gate; the
                // InUseGuard must drop (decrementing in_use) for waiters to proceed.
                tokio::time::sleep(Duration::from_millis(2)).await;
                Ok::<(), ()>(())
            })
            .await
        }));
    }

    let all = async {
        for h in handles {
            h.await
                .expect("admit task did not panic")
                .expect("fetch closure is infallible here");
        }
    };
    tokio::time::timeout(Duration::from_secs(10), all)
        .await
        .expect("saturated admit_per_relay must complete (no live-lock)");

    // Every guard dropped: the in-use count is back to zero, no slot leaked.
    assert_eq!(
        health.in_use(RELAY),
        0,
        "every InUseGuard dropped — no in-use slot leaked after saturation"
    );
}
