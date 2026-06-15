//! BFS frontier verification.
//!
//! These tests prove the DB-resident, crash-safe BFS frontier built on
//! `pubkeys.status`: reachability-gated discovery (CRAWL-01/02), crash-resume
//! via the startup reclaim of orphaned `in_progress` leases (CRAWL-03), bounded
//! concurrency with `FOR UPDATE SKIP LOCKED` no-double-claim (CRAWL-04), and
//! terminal-status `last_fetched_at` stamping (FRESH-01).
//!
//! Plan 03-02 filled the frontier-module tests (seed/claim/lease/reclaim/requeue);
//! plan 03-03 fills the end-to-end crawl-loop tests (BFS reachability, structural
//! spam-island exclusion, crash-resume no-redo, bounded concurrency, terminal
//! stamping) now that the bounded worker loop exists. All tests are active (no
//! ignored scaffolds remain).
//!
//! Requires a running Docker daemon (testcontainers Postgres).

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use chrono::Utc;
use nostr_sdk::{Event, Kind, Timestamp};
use web_of_trust::crawl::frontier::{
    claim_batch, reclaim_stale_on_startup, requeue_or_fail, seed_anchor, ClaimedAuthor,
};
use web_of_trust::crawl::run_crawl;
use web_of_trust::error::RelayError;
use web_of_trust::store::{self, pubkeys::upsert_pubkey};

// The `ScriptedGraph` mock + the `follows_event` / `pk` / `fresh_db` / `status_of`
// fixtures were promoted into `tests/common/mod.rs` (plan 04-01) so the
// crawl/daemon-loop test binaries share one harness. The existing test bodies
// below use them unqualified via these re-uses.
use common::{follows_event, fresh_db, pk, status_of, ScriptedGraph};

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
// 03-03 end-to-end crawl-loop tests (bounded worker pool drives the frontier
// against the deterministic offline ScriptedGraph — never live relays).
// ---------------------------------------------------------------------------

/// Read the surrogate id for a seed's pubkey (the BFS graph references authors by
/// seed; this resolves the row the loop created for them).
async fn id_of_seed(pool: &sqlx::PgPool, seed: u8) -> anyhow::Result<Option<i64>> {
    let key = common::keys(seed).public_key().to_bytes().to_vec();
    let id: Option<i64> = sqlx::query_scalar::<_, i64>("SELECT id FROM pubkeys WHERE pubkey = $1")
        .bind(key)
        .fetch_optional(pool)
        .await?;
    Ok(id)
}

/// CRAWL-01/02: a crawl from a single configured anchor discovers every pubkey
/// reachable through follows via BFS — anchor and all multi-hop followees end
/// `fetched` — while an isolated spam-island pubkey that nobody reachable follows
/// is never even inserted (CRAWL-02 structural). Graph: anchor(1) -> {2,3};
/// 2 -> {4}; 3 -> {4,5}; 4,5 -> {} (leaves). Spam island: seed 99, followed by
/// nobody reachable.
#[tokio::test]
async fn bfs_reaches_full_component() -> anyhow::Result<()> {
    let (_pg, pool) = fresh_db().await?;

    // Build the scripted relay graph. Leaves (4,5) publish empty follow lists so
    // they terminate `fetched` rather than `not_found`.
    let graph = ScriptedGraph::new(vec![
        follows_event(1, &[2, 3], 1000),
        follows_event(2, &[4], 1000),
        follows_event(3, &[4, 5], 1000),
        follows_event(4, &[], 1000),
        follows_event(5, &[], 1000),
        // Seed 99 (the spam island) publishes a follow list too, but NOBODY
        // reachable follows it, so it is never upserted and never fetched.
        follows_event(99, &[1], 1000),
    ]);

    let anchor = common::keys(1).public_key().to_bytes().to_vec();
    let stats = run_crawl(
        &pool,
        &anchor,
        8,
        4,
        Kind::ContactList,
        Timestamp::now(),
        3600,
        10_000,
        web_of_trust::crawl::DEFAULT_MAX_ATTEMPTS,
        graph.fetch_fn(),
    )
    .await?;

    assert_eq!(stats.reclaimed_on_startup, 0, "fresh DB has no orphans");
    assert!(stats.authors_claimed >= 5, "all 5 reachable authors must be claimed");

    // Every reachable pubkey ended `fetched`.
    for seed in [1u8, 2, 3, 4, 5] {
        let id = id_of_seed(&pool, seed)
            .await?
            .unwrap_or_else(|| panic!("reachable seed {seed} must be a row"));
        assert_eq!(
            status_of(&pool, id).await?,
            "fetched",
            "reachable seed {seed} must end fetched (CRAWL-01)"
        );
    }

    // CRAWL-02: the spam island (seed 99) was never inserted — nobody reachable
    // follows it, so structural reachability kept it out of the frontier entirely.
    assert!(
        id_of_seed(&pool, 99).await?.is_none(),
        "spam-island seed 99 must never be a row (CRAWL-02 structural)"
    );

    Ok(())
}

/// CRAWL-02 (end-to-end): an isolated pubkey nobody reachable follows is never
/// fetched. Distinct from `bfs_reaches_full_component`'s never-inserted island:
/// here we PRE-SEED the island as a `discovered` row by hand (as if a prior run
/// left it), give it no scripted event, and assert the crawl resolves it to a
/// non-`fetched` terminal status rather than synthesizing edges for it.
#[tokio::test]
async fn spam_island_never_fetched_endtoend() -> anyhow::Result<()> {
    let (_pg, pool) = fresh_db().await?;

    // anchor(1) -> {2}; 2 -> {} (leaf). The island (seed 50) is NOT in this graph.
    let graph = ScriptedGraph::new(vec![
        follows_event(1, &[2], 1000),
        follows_event(2, &[], 1000),
    ]);

    // Pre-seed the island so it IS a discovered row, but no reachable author
    // follows it and it has no scripted event -> it must resolve to not_found,
    // never fetched (no edges are ever synthesized for it).
    let island = upsert_pubkey(&pool, &common::keys(50).public_key().to_bytes()).await?;

    let anchor = common::keys(1).public_key().to_bytes().to_vec();
    run_crawl(
        &pool,
        &anchor,
        8,
        4,
        Kind::ContactList,
        Timestamp::now(),
        3600,
        10_000,
        web_of_trust::crawl::DEFAULT_MAX_ATTEMPTS,
        graph.fetch_fn(),
    )
    .await?;

    assert_ne!(
        status_of(&pool, island).await?,
        "fetched",
        "an island with no follow list must never be fetched (CRAWL-02)"
    );
    // It has no outgoing edges (the crawl never invented follows for it).
    let island_edges: i64 =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM follows WHERE follower_id = $1")
            .bind(island)
            .fetch_one(&pool)
            .await?;
    assert_eq!(island_edges, 0, "no edges may be synthesized for the island");

    Ok(())
}

/// CRAWL-03: after a crash (orphaned `in_progress` rows) and restart, a row that
/// was already `fetched` before the crash is never re-fetched on the second pass.
/// We simulate a crash by claiming a batch (-> in_progress) and dropping it
/// without finishing, and a prior success by marking a row `fetched` with a known
/// `fetch_count`; the second `run_crawl` reclaims the orphan and completes it,
/// but never touches the pre-`fetched` row's `fetch_count`.
#[tokio::test]
async fn crash_resume_no_redo() -> anyhow::Result<()> {
    let (_pg, pool) = fresh_db().await?;

    // Graph: anchor(1) -> {2}; 2 -> {} (leaf).
    let graph = ScriptedGraph::new(vec![
        follows_event(1, &[2], 1000),
        follows_event(2, &[], 1000),
    ]);

    // --- Simulate a crash mid-crawl: seed the anchor + an extra orphan, claim
    // them (-> in_progress), then "die" without finishing. ---
    let anchor_id = seed_anchor(&pool, &common::keys(1).public_key().to_bytes()).await?;
    let orphan_id = upsert_pubkey(&pool, &common::keys(7).public_key().to_bytes()).await?;
    let claimed = claim_batch(&pool, 100).await?;
    assert_eq!(claimed.len(), 2, "anchor + orphan claimed -> in_progress");

    // Simulate a row that completed BEFORE the crash: mark seed 2's future row
    // `fetched` by hand with a known fetch_count, so we can prove it is not
    // re-fetched. (It is reachable from the anchor, so the crawl could try to
    // re-fetch it if the claim were too broad — it must NOT.)
    let pre_fetched = upsert_pubkey(&pool, &common::keys(2).public_key().to_bytes()).await?;
    sqlx::query(
        "UPDATE pubkeys SET status = 'fetched', fetch_count = 5, last_fetched_at = now() WHERE id = $1",
    )
    .bind(pre_fetched)
    .execute(&pool)
    .await?;

    let fc_before: i64 =
        sqlx::query_scalar::<_, i64>("SELECT fetch_count FROM pubkeys WHERE id = $1")
            .bind(pre_fetched)
            .fetch_one(&pool)
            .await?;
    assert_eq!(fc_before, 5);

    // --- Restart: run_crawl seeds (idempotent) + reclaims the 2 orphans, then
    // crawls. The pre-`fetched` seed-2 row must NEVER be re-claimed. ---
    let anchor = common::keys(1).public_key().to_bytes().to_vec();
    let stats = run_crawl(
        &pool,
        &anchor,
        8,
        4,
        Kind::ContactList,
        Timestamp::now(),
        3600,
        10_000,
        web_of_trust::crawl::DEFAULT_MAX_ATTEMPTS,
        graph.fetch_fn(),
    )
    .await?;

    assert_eq!(
        stats.reclaimed_on_startup, 2,
        "the 2 orphaned in_progress rows must be reclaimed (CRAWL-03)"
    );

    // The orphaned anchor was reclaimed and completed -> fetched.
    assert_eq!(status_of(&pool, anchor_id).await?, "fetched");
    // The orphan(7) had no scripted event -> terminal not_found (still resolved).
    assert_eq!(status_of(&pool, orphan_id).await?, "not_found");

    // The pre-`fetched` row's fetch_count is UNCHANGED — it was never re-fetched.
    let fc_after: i64 =
        sqlx::query_scalar::<_, i64>("SELECT fetch_count FROM pubkeys WHERE id = $1")
            .bind(pre_fetched)
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        fc_after, 5,
        "a pre-fetched row must never be re-fetched after restart (CRAWL-03 no-redo)"
    );

    Ok(())
}

/// CRAWL-04: with concurrency cap K, the number of batches in flight at once never
/// exceeds K. We instrument the injected fetch closure with an `AtomicUsize` that
/// is bumped on entry and decremented on exit, holding a short delay in between so
/// multiple batches overlap; the max observed in-flight count must be <= K.
#[tokio::test]
async fn bounded_concurrency() -> anyhow::Result<()> {
    let (_pg, pool) = fresh_db().await?;

    // Seed a wide fan-out so there are many small batches to overlap: anchor(1)
    // follows 20 leaves (seeds 100..120), each a leaf publishing an empty list.
    let leaves: Vec<u8> = (100u8..120).collect();
    let mut events = vec![follows_event(1, &leaves, 1000)];
    for &s in &leaves {
        events.push(follows_event(s, &[], 1000));
    }
    let graph = ScriptedGraph::new(events);

    let k = 3usize;
    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));

    // Wrap the graph fetch with the in-flight instrument + a small delay so
    // overlap is observable. batch_size = 1 forces one batch per author so up to
    // K can overlap.
    let fetch = {
        let graph = graph.clone();
        let in_flight = Arc::clone(&in_flight);
        let max_seen = Arc::clone(&max_seen);
        move |batch: Vec<ClaimedAuthor>| {
            let graph = graph.clone();
            let in_flight = Arc::clone(&in_flight);
            let max_seen = Arc::clone(&max_seen);
            async move {
                let cur = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                max_seen.fetch_max(cur, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(40)).await;
                let union = graph.union_for(&batch);
                in_flight.fetch_sub(1, Ordering::SeqCst);
                Ok::<Vec<Event>, RelayError>(union)
            }
        }
    };

    let anchor = common::keys(1).public_key().to_bytes().to_vec();
    run_crawl(
        &pool,
        &anchor,
        1, // batch_size = 1 -> one batch per author, maximizing overlap
        k,
        Kind::ContactList,
        Timestamp::now(),
        3600,
        10_000,
        web_of_trust::crawl::DEFAULT_MAX_ATTEMPTS,
        fetch,
    )
    .await?;

    let peak = max_seen.load(Ordering::SeqCst);
    assert!(peak >= 2, "the wide fan-out must actually overlap batches (saw {peak})");
    assert!(
        peak <= k,
        "in-flight batch count {peak} exceeded the concurrency cap {k} (CRAWL-04)"
    );

    Ok(())
}

/// FRESH-01: every terminal transition (`fetched`, `not_found`, `failed`) stamps
/// `last_fetched_at` through the full crawl loop. We construct a graph producing
/// all three: anchor(1) -> {2 (has list -> fetched), 8 (no list -> not_found)};
/// plus a pre-seeded row (seed 60) whose fetch always errors -> failed after the
/// retry cap. The failing author is driven by a fetch closure that errors for it.
#[tokio::test]
async fn last_fetched_at_stamped_on_terminal() -> anyhow::Result<()> {
    let (_pg, pool) = fresh_db().await?;

    // anchor(1) -> {2, 8}; 2 -> {} (leaf, fetched); 8 has NO scripted event
    // (relays answer, no list -> not_found).
    let graph = ScriptedGraph::new(vec![
        follows_event(1, &[2, 8], 1000),
        follows_event(2, &[], 1000),
    ]);

    // Pre-seed seed 60 as a discovered row whose fetch always errors -> failed.
    let failing = upsert_pubkey(&pool, &common::keys(60).public_key().to_bytes()).await?;
    let failing_pk = common::keys(60).public_key().to_bytes().to_vec();

    // fetch closure: error for any batch containing the failing pubkey (a genuine
    // transient RelayError -> requeue_or_fail), else serve the scripted union.
    let fetch = {
        let graph = graph.clone();
        let failing_pk = failing_pk.clone();
        move |batch: Vec<ClaimedAuthor>| {
            let graph = graph.clone();
            let failing_pk = failing_pk.clone();
            async move {
                if batch.iter().any(|c| c.pubkey == failing_pk) {
                    Err(RelayError::FetchTimeout("scripted transient failure".into()))
                } else {
                    Ok::<Vec<Event>, RelayError>(graph.union_for(&batch))
                }
            }
        }
    };

    let anchor = common::keys(1).public_key().to_bytes().to_vec();
    // max_attempts = 1 so the failing author hits the cap on its first failure and
    // terminates `failed` in a single pass (the loop re-claims requeued rows).
    run_crawl(
        &pool,
        &anchor,
        1, // batch_size = 1 so the failing author is isolated in its own batch
        4,
        Kind::ContactList,
        Timestamp::now(),
        3600,
        10_000,
        1,
        fetch,
    )
    .await?;

    // Resolve the three terminal rows.
    let fetched = id_of_seed(&pool, 2).await?.expect("seed 2 must be a row");
    let not_found = id_of_seed(&pool, 8).await?.expect("seed 8 must be a row");

    assert_eq!(status_of(&pool, fetched).await?, "fetched");
    assert_eq!(status_of(&pool, not_found).await?, "not_found");
    assert_eq!(status_of(&pool, failing).await?, "failed");

    // FRESH-01: all three terminal rows have a non-NULL last_fetched_at.
    for (label, id) in [("fetched", fetched), ("not_found", not_found), ("failed", failing)] {
        let lfa: Option<chrono::DateTime<Utc>> =
            sqlx::query_scalar::<_, Option<chrono::DateTime<Utc>>>(
                "SELECT last_fetched_at FROM pubkeys WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&pool)
            .await?;
        assert!(
            lfa.is_some(),
            "FRESH-01: terminal {label} row must stamp last_fetched_at"
        );
    }

    Ok(())
}
