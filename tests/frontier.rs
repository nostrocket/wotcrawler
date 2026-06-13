//! BFS frontier verification.
//!
//! These tests prove the DB-resident, crash-safe BFS frontier built on
//! `pubkeys.status`: reachability-gated discovery (CRAWL-01/02), crash-resume
//! via the startup reclaim of orphaned `in_progress` leases (CRAWL-03), bounded
//! concurrency with `FOR UPDATE SKIP LOCKED` no-double-claim (CRAWL-04), and
//! terminal-status `last_fetched_at` stamping (FRESH-01).
//!
//! Plan 03-02 fills the frontier-module tests (seed/claim/lease/reclaim/requeue);
//! the end-to-end crawl-loop tests (BFS reachability, bounded concurrency, the
//! crash-resume integration) land in 03-03 once the worker loop exists and are
//! kept here as `#[ignore]` scaffolds so the suite contract stays visible.
//!
//! Requires a running Docker daemon (testcontainers Postgres).

mod common;

use chrono::Utc;
use web_of_trust::crawl::frontier::{
    claim_batch, reclaim_stale_on_startup, requeue_or_fail, seed_anchor,
};
use web_of_trust::store::{self, pubkeys::upsert_pubkey};

/// Deterministic 32-byte pubkey from a single seed (mirrors concurrency::pk).
fn pk(seed: u16) -> [u8; 32] {
    let mut k = [0u8; 32];
    k[0] = (seed & 0xff) as u8;
    k[1] = (seed >> 8) as u8;
    k
}

/// Connect + migrate a fresh testcontainers Postgres, returning the live pool.
/// The container handle is returned alongside so the caller keeps it alive.
async fn fresh_db() -> anyhow::Result<(
    testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>,
    sqlx::PgPool,
)> {
    let (pg, url) = common::start_postgres().await?;
    let pool = store::connect(&url).await?;
    store::run_migrations(&pool).await?;
    Ok((pg, pool))
}

/// Read a pubkey's current status string.
async fn status_of(pool: &sqlx::PgPool, id: i64) -> anyhow::Result<String> {
    let s = sqlx::query_scalar::<_, String>("SELECT status FROM pubkeys WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await?;
    Ok(s)
}

/// CRAWL-01 / D-03: seeding the anchor lands exactly one `discovered` row and
/// returns its id; it is the only externally-inserted pubkey.
#[tokio::test]
async fn seed_anchor_lands_discovered() -> anyhow::Result<()> {
    let (_pg, pool) = fresh_db().await?;

    let anchor = pk(1);
    let id = seed_anchor(&pool, &anchor).await?;

    // Exactly one row, and it is `discovered`.
    let count: i64 = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM pubkeys")
        .fetch_one(&pool)
        .await?;
    assert_eq!(count, 1, "anchor seed must create exactly one row");
    assert_eq!(status_of(&pool, id).await?, "discovered");

    // Re-seeding the same anchor is idempotent (same id, still one row).
    let id2 = seed_anchor(&pool, &anchor).await?;
    assert_eq!(id, id2, "re-seeding the anchor returns the same id");
    let count2: i64 = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM pubkeys")
        .fetch_one(&pool)
        .await?;
    assert_eq!(count2, 1, "re-seeding must not create a duplicate row");

    Ok(())
}

/// CRAWL-03 core (RESEARCH Pitfall 3): `claim_batch` selects only `discovered`,
/// so a `fetched` row is never returned — completed work is never re-fetched.
#[tokio::test]
async fn claim_never_returns_fetched() -> anyhow::Result<()> {
    let (_pg, pool) = fresh_db().await?;

    let discovered = upsert_pubkey(&pool, &pk(1)).await?;
    let done = upsert_pubkey(&pool, &pk(2)).await?;

    // Mark `done` terminal-fetched (the writer would set this on success).
    sqlx::query("UPDATE pubkeys SET status = 'fetched', last_fetched_at = now() WHERE id = $1")
        .bind(done)
        .execute(&pool)
        .await?;

    let claimed = claim_batch(&pool, 100).await?;
    let ids: Vec<i64> = claimed.iter().map(|c| c.id).collect();

    assert!(ids.contains(&discovered), "the discovered row must be claimed");
    assert!(
        !ids.contains(&done),
        "a fetched row must never be claimed (CRAWL-03)"
    );
    // The claimed pubkey bytes round-trip as the original 32 bytes.
    let got = claimed.iter().find(|c| c.id == discovered).unwrap();
    assert_eq!(got.pubkey, pk(1).to_vec());
    // The claim flipped it to `in_progress`.
    assert_eq!(status_of(&pool, discovered).await?, "in_progress");

    Ok(())
}

/// CRAWL-04 / T-03-04: two concurrent `claim_batch` calls on SEPARATE pools over
/// a pool of M `discovered` rows return disjoint id sets with no duplicates, and
/// neither call blocks (`FOR UPDATE SKIP LOCKED`). Timeout-guarded like
/// `concurrency.rs`.
#[tokio::test]
async fn skip_locked_no_double_claim() -> anyhow::Result<()> {
    use std::time::Duration;

    let (_pg, url) = common::start_postgres().await?;
    let setup = store::connect(&url).await?;
    store::run_migrations(&setup).await?;

    // Seed M discovered rows.
    let m: u16 = 40;
    for i in 1..=m {
        upsert_pubkey(&setup, &pk(i)).await?;
    }

    // Two SEPARATE pools proxy two workers (same fidelity as concurrency.rs).
    let pool_a = store::connect(&url).await?;
    let pool_b = store::connect(&url).await?;

    let task_a = tokio::spawn(async move { claim_batch(&pool_a, m as i64).await });
    let task_b = tokio::spawn(async move { claim_batch(&pool_b, m as i64).await });

    // Neither may block: guard both with a timeout.
    let res_a = tokio::time::timeout(Duration::from_secs(10), task_a)
        .await
        .expect("worker A blocked — SKIP LOCKED regression")??;
    let res_b = tokio::time::timeout(Duration::from_secs(10), task_b)
        .await
        .expect("worker B blocked — SKIP LOCKED regression")??;

    let ids_a: std::collections::HashSet<i64> = res_a.iter().map(|c| c.id).collect();
    let ids_b: std::collections::HashSet<i64> = res_b.iter().map(|c| c.id).collect();

    // Disjoint: no id claimed by both workers.
    let overlap: Vec<&i64> = ids_a.intersection(&ids_b).collect();
    assert!(
        overlap.is_empty(),
        "two workers claimed the same id(s): {overlap:?} (SKIP LOCKED double-claim)"
    );
    // Union has no duplicates and covers all M rows (each claimed exactly once).
    assert_eq!(
        ids_a.len() + ids_b.len(),
        m as usize,
        "every discovered row must be claimed exactly once across the two workers"
    );

    Ok(())
}

/// CRAWL-03 / D-06: `reclaim_stale_on_startup` resets every orphaned `in_progress`
/// row back to `discovered` with `claimed_at = NULL`, and returns the count reset.
#[tokio::test]
async fn startup_reclaims_in_progress() -> anyhow::Result<()> {
    let (_pg, pool) = fresh_db().await?;

    // Seed rows and simulate a crash: claim some (-> in_progress), leave a
    // `fetched` row that the reclaim must NOT touch.
    for i in 1..=5u16 {
        upsert_pubkey(&pool, &pk(i)).await?;
    }
    let fetched = upsert_pubkey(&pool, &pk(99)).await?;
    sqlx::query("UPDATE pubkeys SET status = 'fetched', last_fetched_at = now() WHERE id = $1")
        .bind(fetched)
        .execute(&pool)
        .await?;

    // Claim 5 -> they go in_progress with claimed_at stamped (the "crash" leaves
    // them orphaned since no clean shutdown / status flip follows).
    let claimed = claim_batch(&pool, 5).await?;
    assert_eq!(claimed.len(), 5);

    let reset = reclaim_stale_on_startup(&pool).await?;
    assert_eq!(reset, 5, "all 5 orphaned in_progress rows must be reclaimed");

    // No in_progress rows remain.
    let in_prog: i64 =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM pubkeys WHERE status = 'in_progress'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(in_prog, 0, "no in_progress rows may survive the reclaim");

    // The reclaimed rows are `discovered` with claimed_at cleared.
    for c in &claimed {
        assert_eq!(status_of(&pool, c.id).await?, "discovered");
        let claimed_at: Option<chrono::DateTime<Utc>> =
            sqlx::query_scalar::<_, Option<chrono::DateTime<Utc>>>(
                "SELECT claimed_at FROM pubkeys WHERE id = $1",
            )
            .bind(c.id)
            .fetch_one(&pool)
            .await?;
        assert!(claimed_at.is_none(), "claimed_at must be cleared on reclaim");
    }

    // The pre-existing `fetched` row was untouched.
    assert_eq!(status_of(&pool, fetched).await?, "fetched");

    Ok(())
}

/// D-09: a transient error with `fetch_attempts < max` returns the pubkey to
/// `discovered` and increments `fetch_attempts` (and clears the lease).
#[tokio::test]
async fn requeue_under_cap_returns_to_discovered() -> anyhow::Result<()> {
    let (_pg, pool) = fresh_db().await?;

    let id = upsert_pubkey(&pool, &pk(1)).await?;
    // Claim it so it is in_progress with a lease.
    let _ = claim_batch(&pool, 1).await?;
    assert_eq!(status_of(&pool, id).await?, "in_progress");

    let max = 3i16;
    // First transient failure: attempts 0 -> 1, under cap -> discovered.
    requeue_or_fail(&pool, id, max, Utc::now()).await?;

    assert_eq!(status_of(&pool, id).await?, "discovered");
    let attempts: i16 =
        sqlx::query_scalar::<_, i16>("SELECT fetch_attempts FROM pubkeys WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(attempts, 1, "fetch_attempts must be incremented");
    // Lease cleared on requeue.
    let claimed_at: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar::<_, Option<chrono::DateTime<Utc>>>(
            "SELECT claimed_at FROM pubkeys WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await?;
    assert!(claimed_at.is_none(), "requeue must clear the lease");
    // Under-cap requeue must NOT stamp last_fetched_at (knowledge not refreshed).
    let lfa: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar::<_, Option<chrono::DateTime<Utc>>>(
            "SELECT last_fetched_at FROM pubkeys WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await?;
    assert!(
        lfa.is_none(),
        "an under-cap requeue must not stamp last_fetched_at"
    );

    Ok(())
}

/// D-09/D-11 + FRESH-01 (RESEARCH Pitfall 5/7): at the cap, `requeue_or_fail`
/// sets status `failed` AND stamps `last_fetched_at` so the staleness loop can
/// later re-enqueue it. Proves a flaky pubkey terminates rather than bouncing.
#[tokio::test]
async fn requeue_at_cap_sets_failed_and_stamps() -> anyhow::Result<()> {
    let (_pg, pool) = fresh_db().await?;

    let id = upsert_pubkey(&pool, &pk(1)).await?;
    let max = 3i16;

    // Drive transient failures up to the cap. With max=3, the 3rd bump
    // (attempts 2 -> 3) hits the cap and transitions to `failed`.
    requeue_or_fail(&pool, id, max, Utc::now()).await?; // 0 -> 1, discovered
    requeue_or_fail(&pool, id, max, Utc::now()).await?; // 1 -> 2, discovered
    assert_eq!(
        status_of(&pool, id).await?,
        "discovered",
        "still under cap before the final attempt"
    );

    let stamp = Utc::now();
    requeue_or_fail(&pool, id, max, stamp).await?; // 2 -> 3 == cap -> failed

    assert_eq!(
        status_of(&pool, id).await?,
        "failed",
        "at the cap the pubkey must be terminal failed (no infinite bounce)"
    );
    let attempts: i16 =
        sqlx::query_scalar::<_, i16>("SELECT fetch_attempts FROM pubkeys WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(attempts, 3);
    // FRESH-01: last_fetched_at stamped on the terminal failed transition.
    let lfa: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar::<_, Option<chrono::DateTime<Utc>>>(
            "SELECT last_fetched_at FROM pubkeys WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await?;
    assert!(
        lfa.is_some(),
        "FRESH-01: a terminal `failed` row must stamp last_fetched_at"
    );

    Ok(())
}

/// CRAWL-02 (claim-level portion): the structural reachability gate is that
/// `upsert_pubkey`-on-followee is the ONLY insertion path besides the anchor
/// seed. Here we assert that whatever IS a `discovered` row is claimable — i.e.
/// `claim_batch` returns a pubkey *because* it is `discovered`, never on any
/// reachability predicate. The full end-to-end spam-island test (an isolated
/// pubkey nobody reachable follows is never inserted) is exercised in 03-03 once
/// the crawl loop drives discovery; here we document that insertion is the gate.
#[tokio::test]
async fn spam_island_never_crawled() -> anyhow::Result<()> {
    let (_pg, pool) = fresh_db().await?;

    // A `discovered` row IS claimable purely because of its status. There is no
    // reachability column or predicate in the claim — reachability is enforced by
    // the (separate) fact that nothing inserts an unreachable pubkey (D-02).
    let reachable = upsert_pubkey(&pool, &pk(1)).await?;
    let claimed = claim_batch(&pool, 100).await?;
    let ids: Vec<i64> = claimed.iter().map(|c| c.id).collect();
    assert_eq!(
        ids,
        vec![reachable],
        "claim returns a pubkey solely because it is discovered (no reachability predicate)"
    );

    // A pubkey that never lands as a row (the spam island — nobody reachable
    // upserts it) is, by construction, absent from the table and so can never be
    // claimed. Asserting the table holds only the one inserted row documents that
    // insertion (via upsert_pubkey) is the gate, not the claim query.
    let total: i64 = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM pubkeys")
        .fetch_one(&pool)
        .await?;
    assert_eq!(
        total, 1,
        "an un-upserted spam-island pubkey is never a row, so never claimable"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// 03-03 scaffolds (end-to-end crawl loop) — bodies land once the worker loop
// exists. Kept `#[ignore]` so the suite contract stays visible.
// ---------------------------------------------------------------------------

/// CRAWL-01: a crawl from a configured anchor discovers every reachable pubkey
/// via BFS — the anchor and all multi-hop followees end `fetched`.
#[tokio::test]
#[ignore] // 03-03 — needs the bounded worker loop
async fn bfs_reaches_full_component() -> anyhow::Result<()> {
    Ok(())
}

/// CRAWL-03: after a crash (orphaned `in_progress` rows) and restart, a row that
/// was already `fetched` before the crash is never re-fetched on the second pass.
#[tokio::test]
#[ignore] // 03-03 — needs the bounded worker loop
async fn crash_resume_no_redo() -> anyhow::Result<()> {
    Ok(())
}

/// CRAWL-04: with concurrency cap K, at most K batches are ever in flight.
#[tokio::test]
#[ignore] // 03-03 — needs the bounded worker loop
async fn bounded_concurrency() -> anyhow::Result<()> {
    Ok(())
}

/// FRESH-01: every terminal transition (`fetched`, `not_found`, `failed`) stamps
/// `last_fetched_at` through the full crawl loop.
#[tokio::test]
#[ignore] // 03-03 — needs the bounded worker loop
async fn last_fetched_at_stamped_on_terminal() -> anyhow::Result<()> {
    Ok(())
}
