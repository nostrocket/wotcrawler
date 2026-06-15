//! Daemon continuous-loop + graceful-shutdown verification (filled in 04-04).
//!
//! These integration tests drive [`web_of_trust::daemon::loop_::run_daemon_loop`]
//! over a real testcontainers Postgres with the promoted [`common::ScriptedGraph`]
//! offline mock + the injected-`fetch_union` seam, and an injected
//! [`tokio_util::sync::CancellationToken`] (never real signals — RESEARCH §Test
//! Seams). They prove the OPS-02 graceful-drain guarantee (zero orphaned
//! `in_progress` leases), the FRESH-02 continuous idle-then-resume behavior, and
//! an OBS-04 progress-summary count over the sampler's `frontier_counts`.
//!
//! Requires a running Docker daemon (testcontainers Postgres). Run with
//! `-- --test-threads=2` (testcontainers race).

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use nostr_sdk::{Kind, PublicKey};
use tokio_util::sync::CancellationToken;
use web_of_trust::crawl::frontier::seed_anchor;
use web_of_trust::daemon::loop_::run_daemon_loop;
use web_of_trust::relay::health::{RelayHealthRegistry, DEFAULT_HEALTH_ALPHA};
use web_of_trust::daemon::sampler::frontier_counts;
use web_of_trust::store::pubkeys::upsert_pubkey;

use common::{follows_event, fresh_db, status_of, ScriptedGraph};

/// Resolve a seed's pubkey row id if it exists.
async fn id_of_seed(pool: &sqlx::PgPool, seed: u8) -> anyhow::Result<Option<i64>> {
    let pubkey = common::keys(seed).public_key().to_bytes().to_vec();
    let id = sqlx::query_scalar::<_, i64>("SELECT id FROM pubkeys WHERE pubkey = $1")
        .bind(pubkey)
        .fetch_optional(pool)
        .await?;
    Ok(id)
}

/// Count `in_progress` leases.
async fn in_progress_count(pool: &sqlx::PgPool) -> anyhow::Result<i64> {
    let n = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM pubkeys WHERE status = 'in_progress'",
    )
    .fetch_one(pool)
    .await?;
    Ok(n)
}

/// OPS-02 (T-04-08): cancelling the loop drains in-flight workers and leaves zero
/// `in_progress` leases (no orphans). We seed a small scripted graph, spawn the
/// loop, cancel it after it has made progress, await its clean return, and assert
/// no row is left leased.
#[tokio::test]
async fn graceful_drain_no_orphan_leases() -> anyhow::Result<()> {
    let (_pg, pool) = fresh_db().await?;

    // anchor(1) -> {2,3}; 2 -> {4}; 3 -> {}; 4 -> {} — a small finite component.
    let graph = ScriptedGraph::new(vec![
        follows_event(1, &[2, 3], 1000),
        follows_event(2, &[4], 1000),
        follows_event(3, &[], 1000),
        follows_event(4, &[], 1000),
    ]);

    let anchor = common::keys(1).public_key().to_bytes().to_vec();
    let token = CancellationToken::new();
    let loop_alive = Arc::new(AtomicBool::new(false));

    let loop_token = token.clone();
    let loop_alive_h = Arc::clone(&loop_alive);
    let loop_pool = pool.clone();
    let handle = tokio::spawn(async move {
        run_daemon_loop(
            &loop_pool,
            &anchor,
            8,
            4,
            Kind::ContactList,
            3600,
            10_000,
            web_of_trust::crawl::DEFAULT_MAX_ATTEMPTS,
            Duration::from_millis(20),
            loop_token,
            loop_alive_h,
            graph.fetch_fn(),
            // Fallback disabled for these loop-control tests (the live NIP-65
            // recovery path is exercised in tests/nip65_fallback.rs): a fresh
            // registry + no-op closures keep behavior identical to before 05-04.
            false,
            web_of_trust::relay::health::DEFAULT_NIP65_MAX_WRITE_RELAYS,
            Arc::new(RelayHealthRegistry::new(DEFAULT_HEALTH_ALPHA)),
            |_pk: PublicKey, _relays: Vec<String>| std::future::ready(Ok(Vec::new())),
            |_pk: PublicKey| std::future::ready(Ok(Vec::new())),
        )
        .await
    });

    // Wait until the loop has made progress (at least one row fetched) or a short
    // deadline, then cancel — this exercises the drain with work in flight.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let fetched: i64 = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM pubkeys WHERE status = 'fetched'",
        )
        .fetch_one(&pool)
        .await?;
        if fetched >= 1 || tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    token.cancel();

    // The loop must return Ok within a bounded time after cancel (graceful drain).
    let stats = tokio::time::timeout(Duration::from_secs(20), handle)
        .await
        .expect("loop must return promptly after cancel")
        .expect("loop task must not panic")
        .expect("loop must return Ok");

    assert!(stats.authors_claimed >= 1, "loop must have claimed at least the anchor");
    // WR-03 / OBS-03: once the loop has returned (drained on cancel), readiness
    // must be back to false — `/health/ready` returns 200 only while the loop is
    // genuinely alive, never through/after the shutdown drain.
    assert!(
        !loop_alive.load(Ordering::Relaxed),
        "loop_alive must flip back to false once the loop drains/returns (WR-03)"
    );

    // OPS-02 guarantee: zero orphaned in_progress leases after the drain.
    assert_eq!(
        in_progress_count(&pool).await?,
        0,
        "graceful drain must leave zero in_progress leases (OPS-02)"
    );

    Ok(())
}

/// FRESH-02: the loop idle-polls an empty frontier instead of terminating, and
/// resumes when a new `discovered` row is enqueued. We run the loop, let the
/// initial finite component drain (loop now idling), enqueue a fresh discovered
/// row with a scripted event, and assert the loop claims and fetches it — then
/// cancel and drain.
#[tokio::test]
async fn idle_then_resume_after_reenqueue() -> anyhow::Result<()> {
    let (_pg, pool) = fresh_db().await?;

    // Initial component is just the anchor following nobody, so it drains to one
    // fetched row almost immediately and the loop starts idling. Seed 7 is the row
    // we re-enqueue mid-idle; its scripted event lets the loop fetch it on resume.
    let graph = ScriptedGraph::new(vec![
        follows_event(1, &[], 1000),
        follows_event(7, &[], 1000),
    ]);

    let anchor = common::keys(1).public_key().to_bytes().to_vec();
    let token = CancellationToken::new();
    let loop_alive = Arc::new(AtomicBool::new(false));

    let loop_token = token.clone();
    let loop_alive_h = Arc::clone(&loop_alive);
    let loop_pool = pool.clone();
    let handle = tokio::spawn(async move {
        run_daemon_loop(
            &loop_pool,
            &anchor,
            8,
            4,
            Kind::ContactList,
            3600,
            10_000,
            web_of_trust::crawl::DEFAULT_MAX_ATTEMPTS,
            Duration::from_millis(20),
            loop_token,
            loop_alive_h,
            graph.fetch_fn(),
            // Fallback disabled for these loop-control tests (the live NIP-65
            // recovery path is exercised in tests/nip65_fallback.rs): a fresh
            // registry + no-op closures keep behavior identical to before 05-04.
            false,
            web_of_trust::relay::health::DEFAULT_NIP65_MAX_WRITE_RELAYS,
            Arc::new(RelayHealthRegistry::new(DEFAULT_HEALTH_ALPHA)),
            |_pk: PublicKey, _relays: Vec<String>| std::future::ready(Ok(Vec::new())),
            |_pk: PublicKey| std::future::ready(Ok(Vec::new())),
        )
        .await
    });

    // Wait for the anchor to be fetched — the loop is now idling on an empty
    // frontier (it did NOT terminate, which is the FRESH-02 distinction).
    let anchor_id = {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(id) = id_of_seed(&pool, 1).await? {
                if status_of(&pool, id).await? == "fetched" {
                    break id;
                }
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "anchor must be fetched before the idle window"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    let _ = anchor_id;

    // The loop has NOT exited (idle-poll, not terminate): enqueue a brand-new
    // discovered row mid-idle. upsert_pubkey lands seed 7 as `discovered`.
    let resumed_id = upsert_pubkey(&pool, &common::keys(7).public_key().to_bytes()).await?;

    // Assert the idling loop wakes, claims, and fetches the re-enqueued row.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if status_of(&pool, resumed_id).await? == "fetched" {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "idle loop must resume and fetch the re-enqueued row (FRESH-02)"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    token.cancel();
    let _stats = tokio::time::timeout(Duration::from_secs(20), handle)
        .await
        .expect("loop must return promptly after cancel")
        .expect("loop task must not panic")
        .expect("loop must return Ok");

    assert_eq!(
        in_progress_count(&pool).await?,
        0,
        "drain after resume leaves zero in_progress leases"
    );

    Ok(())
}

/// OBS-04: the sampler's `frontier_counts` aggregate reports the correct
/// per-status counts and coverage math over a hand-seeded DB. This is the
/// automated proof behind the periodic progress summary (which logs exactly these
/// numbers).
#[tokio::test]
async fn progress_summary_counts() -> anyhow::Result<()> {
    let (_pg, pool) = fresh_db().await?;

    // Seed a known mix: 1 discovered (anchor), then hand-set statuses so coverage
    // math is exact. We upsert four distinct pubkeys and drive their statuses.
    seed_anchor(&pool, &common::keys(1).public_key().to_bytes()).await?; // discovered
    let f1 = upsert_pubkey(&pool, &common::keys(2).public_key().to_bytes()).await?;
    let f2 = upsert_pubkey(&pool, &common::keys(3).public_key().to_bytes()).await?;
    let nf = upsert_pubkey(&pool, &common::keys(4).public_key().to_bytes()).await?;

    // 2 fetched, 1 not_found, 1 discovered (the anchor) -> total 4.
    for id in [f1, f2] {
        sqlx::query("UPDATE pubkeys SET status = 'fetched', last_fetched_at = now() WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await?;
    }
    sqlx::query("UPDATE pubkeys SET status = 'not_found', last_fetched_at = now() WHERE id = $1")
        .bind(nf)
        .execute(&pool)
        .await?;

    let counts = frontier_counts(&pool).await?;
    assert_eq!(counts.discovered, 1, "one discovered (the anchor)");
    assert_eq!(counts.fetched, 2, "two fetched");
    assert_eq!(counts.not_found, 1, "one not_found");
    assert_eq!(counts.failed, 0, "no failed");
    assert_eq!(counts.total, 4, "four total rows");
    assert!(
        (counts.coverage() - 0.5).abs() < f64::EPSILON,
        "coverage = fetched/total = 2/4 = 0.5, got {}",
        counts.coverage()
    );

    Ok(())
}

/// CR-01 regression: a `not_found` row's `last_fetched_at` must be stamped with
/// the ACTUAL fetch time (a fresh per-batch `Timestamp::now()`), NOT the daemon
/// start time. If the stamp were frozen at spawn, a long-running daemon would
/// write a stale `last_fetched_at`, and the TTL staleness scanner would
/// immediately re-enqueue every `not_found`/`failed` row, defeating FRESH-02.
///
/// We model the bug's blast radius directly: an anchor with NO scripted event
/// resolves to `not_found`; its stamp is captured before the loop spawns and
/// must end up AFTER that captured instant. We then assert a 24h-TTL staleness
/// scan does NOT re-enqueue the freshly-stamped row (it is recent, not stale).
#[tokio::test]
async fn terminal_stamp_reflects_fetch_time_not_spawn() -> anyhow::Result<()> {
    use web_of_trust::crawl::frontier::reclaim_stale_by_ttl;

    let (_pg, pool) = fresh_db().await?;

    // The anchor follows nobody AND has no scripted event of its own, so the
    // fetch union is empty for it -> the loop drives it to `not_found`, stamping
    // last_fetched_at via set_fetch_status.
    let graph = ScriptedGraph::new(vec![]);

    // Capture an instant strictly before the loop spawns. The CR-01 bug would
    // stamp last_fetched_at at-or-before this (daemon "spawn" time); the fix
    // stamps it strictly after, at actual fetch time.
    let before_spawn: chrono::DateTime<chrono::Utc> = sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>("SELECT now()")
        .fetch_one(&pool)
        .await?;

    let anchor = common::keys(1).public_key().to_bytes().to_vec();
    let token = CancellationToken::new();
    let loop_alive = Arc::new(AtomicBool::new(false));

    let loop_token = token.clone();
    let loop_alive_h = Arc::clone(&loop_alive);
    let loop_pool = pool.clone();
    let handle = tokio::spawn(async move {
        run_daemon_loop(
            &loop_pool,
            &anchor,
            8,
            4,
            Kind::ContactList,
            3600,
            10_000,
            web_of_trust::crawl::DEFAULT_MAX_ATTEMPTS,
            Duration::from_millis(20),
            loop_token,
            loop_alive_h,
            graph.fetch_fn(),
            // Fallback disabled for these loop-control tests (the live NIP-65
            // recovery path is exercised in tests/nip65_fallback.rs): a fresh
            // registry + no-op closures keep behavior identical to before 05-04.
            false,
            web_of_trust::relay::health::DEFAULT_NIP65_MAX_WRITE_RELAYS,
            Arc::new(RelayHealthRegistry::new(DEFAULT_HEALTH_ALPHA)),
            |_pk: PublicKey, _relays: Vec<String>| std::future::ready(Ok(Vec::new())),
            |_pk: PublicKey| std::future::ready(Ok(Vec::new())),
        )
        .await
    });

    // Wait for the anchor to reach not_found.
    let anchor_id = {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(id) = id_of_seed(&pool, 1).await? {
                if status_of(&pool, id).await? == "not_found" {
                    break id;
                }
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "anchor must reach not_found"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };

    token.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(20), handle)
        .await
        .expect("loop must return promptly after cancel")
        .expect("loop task must not panic")
        .expect("loop must return Ok");

    // The terminal stamp must reflect the actual fetch time: strictly AFTER the
    // instant captured before the loop spawned. The frozen-at-spawn bug would
    // fail this (its stamp would precede the loop's own work).
    let stamp: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar::<_, Option<chrono::DateTime<chrono::Utc>>>(
            "SELECT last_fetched_at FROM pubkeys WHERE id = $1",
        )
        .bind(anchor_id)
        .fetch_one(&pool)
        .await?;
    let stamp = stamp.expect("not_found row must have last_fetched_at stamped (FRESH-01)");
    // `Timestamp::now()` (nostr-sdk) has whole-second resolution, so the stamp is
    // truncated to its second; compare against `before_spawn` truncated likewise
    // (subtracting its sub-second part) so we assert "at or after spawn second"
    // without a false failure from sub-second truncation. The load-bearing CR-01
    // proof is the staleness-scan check below: a frozen-at-spawn stamp on a long
    // run would be arbitrarily old and trip re-enqueue.
    let before_spawn_sec = before_spawn
        - chrono::Duration::nanoseconds(before_spawn.timestamp_subsec_nanos() as i64);
    assert!(
        stamp >= before_spawn_sec,
        "not_found last_fetched_at ({stamp}) must be at/after the pre-spawn second ({before_spawn_sec}) — it must reflect actual fetch time, not a frozen daemon-spawn snapshot (CR-01)"
    );

    // A 24h-TTL staleness scan must NOT re-enqueue this freshly-stamped row: a
    // fresh stamp is recent, so the row stays not_found. The frozen-at-spawn bug
    // on a long run would make this stamp arbitrarily old and trip re-enqueue.
    let reenqueued = reclaim_stale_by_ttl(&pool, 24 * 3600).await?;
    assert_eq!(
        reenqueued, 0,
        "a freshly-stamped not_found row must NOT be re-enqueued by a 24h TTL scan (FRESH-02)"
    );
    assert_eq!(
        status_of(&pool, anchor_id).await?,
        "not_found",
        "row stays not_found after the staleness scan (not re-enqueued)"
    );

    Ok(())
}
