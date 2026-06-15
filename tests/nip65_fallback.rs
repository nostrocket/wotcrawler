//! NIP-65 outbox-fallback integration tests (RELAY-05).
//!
//! These drive [`web_of_trust::crawl::apply::process_batch`] directly over a real
//! testcontainers Postgres, using the relay-URL-aware [`common::ScriptedGraph`]
//! (plan 05-01) to model "author absent on the curated relays, recovered via its
//! NIP-65 write relay". `process_batch` returns the number of authors that
//! resolved to a winning list (curated OR recovered), which the recovery test
//! asserts == 1 (the `nip65_recovered` counter increments on that same hit; the
//! returned count is the deterministic, recorder-free proxy).
//!
//! The deadlock-safety test (`no_deadlock_single_permit`) drives the per-relay
//! `Semaphore` admission gate (`relay::health::admit_per_relay`) under the fixed
//! global -> per-relay -> GCRA acquisition order at `per_relay_concurrency = 1`,
//! proving the order is deadlock-free (05-04).
//!
//! Requires a running Docker daemon. Run with `-- --test-threads=2`; re-run once
//! on a testcontainers container/port flake.

mod common;

use std::collections::HashSet;

use nostr_sdk::{Kind, PublicKey, Timestamp};
use web_of_trust::crawl::apply::process_batch;
use web_of_trust::crawl::frontier::ClaimedAuthor;
use web_of_trust::ingest::relay_list::extract_relay_pairs;
use web_of_trust::relay::health::{RelayHealthRegistry, DEFAULT_HEALTH_ALPHA};
use web_of_trust::store::pubkeys::upsert_pubkey;
use web_of_trust::store::relays::apply_relay_list;

use common::{fresh_db, follows_event, relay_list_event, status_of, ScriptedGraph};

/// kind-3 (ContactList) is what the crawl fetches; future clamp + follow cap
/// mirror the daemon's per-batch literals.
const WANT_KIND: Kind = Kind::ContactList;
const FUTURE_CLAMP_SECS: u64 = 3600;
const FOLLOW_CAP: usize = 10_000;
const MAX_ATTEMPTS: i16 = 5;
const NIP65_MAX_WRITE_RELAYS: usize = 3;

const WRITE_RELAY: &str = "wss://write.example";

/// Count this follower's applied follow edges.
async fn edge_count(pool: &sqlx::PgPool, follower_id: i64) -> anyhow::Result<i64> {
    let n = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM follows WHERE follower_id = $1")
        .bind(follower_id)
        .fetch_one(pool)
        .await?;
    Ok(n)
}

/// RELAY-05: a kind:3 author absent on the curated relays is recovered by
/// fetching from its NIP-65 write relay; the author flips to `fetched`, its
/// follow edges are applied, and the recovery is counted (the returned
/// applied-count is 1 — the same hit that increments `nip65_recovered`).
#[tokio::test]
async fn fallback_recovers_via_write_relay() {
    let (_pg, pool) = fresh_db().await.expect("fresh db");

    // Author seed 1 follows seeds 2 and 3 — but ONLY publishes that kind-3 on its
    // NIP-65 write relay, not on the curated set.
    let author = common::keys(1).public_key();
    let author_id = upsert_pubkey(&pool, &author.to_bytes())
        .await
        .expect("upsert author");

    // Pre-seed the author's write relays so the fallback uses them directly
    // (the unknown-write-relays on-demand path is covered separately).
    apply_relay_list(
        &pool,
        author_id,
        &[(WRITE_RELAY.to_string(), "write")],
        common::dt(1_000),
    )
    .await
    .expect("seed write relays");

    // The write relay holds the author's kind-3; the curated union holds nothing.
    let write_event = follows_event(1, &[2, 3], 1_000);
    let graph = ScriptedGraph::with_relay(vec![(WRITE_RELAY, vec![write_event])]);

    let batch = vec![ClaimedAuthor {
        id: author_id,
        pubkey: author.to_bytes().to_vec(),
    }];
    let health = RelayHealthRegistry::new(DEFAULT_HEALTH_ALPHA);

    let relay_fetch = graph.relay_fetch_fn();
    let applied = process_batch(
        &pool,
        &batch,
        WANT_KIND,
        Timestamp::from_secs(2_000),
        FUTURE_CLAMP_SECS,
        FOLLOW_CAP,
        MAX_ATTEMPTS,
        true, // fallback enabled
        NIP65_MAX_WRITE_RELAYS,
        &health,
        // Curated union: empty -> the curated set has no kind-3 for the author.
        || std::future::ready(Ok(Vec::new())),
        // Fallback: route to the write relay via the URL-aware ScriptedGraph.
        move |pk: PublicKey, relays: Vec<String>| {
            let relay_fetch = relay_fetch.clone();
            let batch = vec![ClaimedAuthor {
                id: 0,
                pubkey: pk.to_bytes().to_vec(),
            }];
            async move {
                let mut union = Vec::new();
                for url in relays {
                    union.extend(relay_fetch(url, batch.clone()).await?);
                }
                Ok(union)
            }
        },
        // relay_list_fetch: never reached (write relays are pre-seeded).
        |_pk: PublicKey| std::future::ready(Ok(Vec::new())),
    )
    .await
    .expect("process_batch");

    assert_eq!(applied, 1, "the author was recovered via its write relay");
    assert_eq!(
        status_of(&pool, author_id).await.expect("status"),
        "fetched",
        "a fallback hit flips the author to fetched (apply_validated)"
    );
    assert_eq!(
        edge_count(&pool, author_id).await.expect("edges"),
        2,
        "the recovered follow list's two edges are written"
    );
}

/// RELAY-05: an author that misses on BOTH the curated relays and its (known)
/// write relays is stamped terminal `not_found` with zero edges applied.
#[tokio::test]
async fn fallback_miss_stamps_not_found() {
    let (_pg, pool) = fresh_db().await.expect("fresh db");

    let author = common::keys(1).public_key();
    let author_id = upsert_pubkey(&pool, &author.to_bytes())
        .await
        .expect("upsert author");

    // Known write relay, but it holds nothing for the author.
    apply_relay_list(
        &pool,
        author_id,
        &[(WRITE_RELAY.to_string(), "write")],
        common::dt(1_000),
    )
    .await
    .expect("seed write relays");

    // Empty graph: neither curated nor the write relay has a kind-3 for the author.
    let graph = ScriptedGraph::with_relay(vec![(WRITE_RELAY, Vec::new())]);

    let batch = vec![ClaimedAuthor {
        id: author_id,
        pubkey: author.to_bytes().to_vec(),
    }];
    let health = RelayHealthRegistry::new(DEFAULT_HEALTH_ALPHA);

    let relay_fetch = graph.relay_fetch_fn();
    let applied = process_batch(
        &pool,
        &batch,
        WANT_KIND,
        Timestamp::from_secs(2_000),
        FUTURE_CLAMP_SECS,
        FOLLOW_CAP,
        MAX_ATTEMPTS,
        true,
        NIP65_MAX_WRITE_RELAYS,
        &health,
        || std::future::ready(Ok(Vec::new())),
        move |pk: PublicKey, relays: Vec<String>| {
            let relay_fetch = relay_fetch.clone();
            let batch = vec![ClaimedAuthor {
                id: 0,
                pubkey: pk.to_bytes().to_vec(),
            }];
            async move {
                let mut union = Vec::new();
                for url in relays {
                    union.extend(relay_fetch(url, batch.clone()).await?);
                }
                Ok(union)
            }
        },
        |_pk: PublicKey| std::future::ready(Ok(Vec::new())),
    )
    .await
    .expect("process_batch");

    assert_eq!(applied, 0, "a miss on curated AND write relays recovers nothing");
    assert_eq!(
        status_of(&pool, author_id).await.expect("status"),
        "not_found",
        "a fallback miss stamps terminal not_found"
    );
    assert_eq!(
        edge_count(&pool, author_id).await.expect("edges"),
        0,
        "no edges are applied for a miss"
    );
}

/// RELAY-05 / Open Question 1: an author with no stored relays whose on-demand
/// curated kind:10002 fetch also yields nothing falls to terminal `not_found`
/// WITHOUT consuming the kind-3 retry budget (status is `not_found`, not a
/// requeued `discovered`/`in_progress`, and fetch_attempts is not bumped).
#[tokio::test]
async fn unknown_write_relays_no_kind10002_stamps_not_found() {
    let (_pg, pool) = fresh_db().await.expect("fresh db");

    let author = common::keys(1).public_key();
    let author_id = upsert_pubkey(&pool, &author.to_bytes())
        .await
        .expect("upsert author");

    // No stored write relays AND the on-demand curated kind:10002 fetch returns
    // nothing -> the fallback cannot resolve any write relays.
    let batch = vec![ClaimedAuthor {
        id: author_id,
        pubkey: author.to_bytes().to_vec(),
    }];
    let health = RelayHealthRegistry::new(DEFAULT_HEALTH_ALPHA);

    let applied = process_batch(
        &pool,
        &batch,
        WANT_KIND,
        Timestamp::from_secs(2_000),
        FUTURE_CLAMP_SECS,
        FOLLOW_CAP,
        MAX_ATTEMPTS,
        true,
        NIP65_MAX_WRITE_RELAYS,
        &health,
        || std::future::ready(Ok(Vec::new())),
        // Fallback never reached (no write relays resolved).
        |_pk: PublicKey, _relays: Vec<String>| std::future::ready(Ok(Vec::new())),
        // On-demand curated kind:10002: empty -> no write relays discovered.
        |_pk: PublicKey| std::future::ready(Ok(Vec::new())),
    )
    .await
    .expect("process_batch");

    assert_eq!(applied, 0, "no write relays resolvable -> nothing recovered");
    assert_eq!(
        status_of(&pool, author_id).await.expect("status"),
        "not_found",
        "a failed on-demand kind:10002 fetch proceeds to terminal not_found"
    );
    let attempts = sqlx::query_scalar::<_, i16>("SELECT fetch_attempts FROM pubkeys WHERE id = $1")
        .bind(author_id)
        .fetch_one(&pool)
        .await
        .expect("fetch_attempts");
    assert_eq!(
        attempts, 0,
        "a failed on-demand kind:10002 fetch must NOT consume the kind-3 retry budget"
    );
}

/// RELAY-05: the on-demand curated kind:10002 path resolves+persists a pubkey's
/// write relays when unknown, then recovers its kind-3 from them — proving the
/// sole persist-on-kind:10002-winner-seen hook works end-to-end. (Also confirms
/// the persisted r-tags round-trip through `extract_relay_pairs`.)
#[tokio::test]
async fn on_demand_kind10002_resolves_then_recovers() {
    let (_pg, pool) = fresh_db().await.expect("fresh db");

    let author = common::keys(1).public_key();
    let author_id = upsert_pubkey(&pool, &author.to_bytes())
        .await
        .expect("upsert author");

    // The author's kind:10002 advertises WRITE_RELAY as a write relay; the curated
    // set serves that relay-list on-demand, and the write relay serves the kind-3.
    let relay_list = relay_list_event(1, &[(WRITE_RELAY, "write")], 900);
    // Sanity: the fixture round-trips to the write marker we expect.
    assert!(
        extract_relay_pairs(&relay_list).contains(&(WRITE_RELAY.to_string(), "write")),
        "fixture advertises WRITE_RELAY as a write relay"
    );
    let kind3 = follows_event(1, &[2, 3, 4], 1_000);
    let graph = ScriptedGraph::with_relay(vec![(WRITE_RELAY, vec![kind3])]);

    let batch = vec![ClaimedAuthor {
        id: author_id,
        pubkey: author.to_bytes().to_vec(),
    }];
    let health = RelayHealthRegistry::new(DEFAULT_HEALTH_ALPHA);

    let relay_fetch = graph.relay_fetch_fn();
    let applied = process_batch(
        &pool,
        &batch,
        WANT_KIND,
        Timestamp::from_secs(2_000),
        FUTURE_CLAMP_SECS,
        FOLLOW_CAP,
        MAX_ATTEMPTS,
        true,
        NIP65_MAX_WRITE_RELAYS,
        &health,
        || std::future::ready(Ok(Vec::new())),
        move |pk: PublicKey, relays: Vec<String>| {
            let relay_fetch = relay_fetch.clone();
            let batch = vec![ClaimedAuthor {
                id: 0,
                pubkey: pk.to_bytes().to_vec(),
            }];
            async move {
                let mut union = Vec::new();
                for url in relays {
                    union.extend(relay_fetch(url, batch.clone()).await?);
                }
                Ok(union)
            }
        },
        // On-demand curated kind:10002: return the author's relay-list event.
        move |pk: PublicKey| {
            let ev = if pk == author {
                vec![relay_list.clone()]
            } else {
                Vec::new()
            };
            std::future::ready(Ok(ev))
        },
    )
    .await
    .expect("process_batch");

    assert_eq!(applied, 1, "the author was recovered via on-demand-resolved relays");
    assert_eq!(
        status_of(&pool, author_id).await.expect("status"),
        "fetched",
        "the on-demand-resolved write relay recovered the kind-3"
    );
    assert_eq!(
        edge_count(&pool, author_id).await.expect("edges"),
        3,
        "the recovered follow list's three edges are written"
    );

    // The on-demand kind:10002 winner was persisted (the sole persist hook).
    let persisted: HashSet<String> =
        sqlx::query_scalar::<_, String>("SELECT url FROM pubkey_relays WHERE pubkey_id = $1")
            .bind(author_id)
            .fetch_all(&pool)
            .await
            .expect("persisted relays")
            .into_iter()
            .collect();
    assert!(
        persisted.contains(WRITE_RELAY),
        "the on-demand kind:10002 winner's write relay was persisted (persist-on-winner-seen)"
    );
}

/// RELAY-06: the global -> per-relay -> GCRA acquisition order is deadlock-free
/// even at `per_relay_concurrency = 1`.
///
/// Models the daemon's fan-out gating discipline exactly (the fan-out is a
/// live-Client closure, so this drives the SAME `admit_per_relay` helper + the
/// SAME acquisition order it uses): N concurrent batches, each holding a global
/// crawl permit (mirroring `loop_.rs`), then fanning out across MULTIPLE relays —
/// each gated through the per-relay `Semaphore::new(1)` admission gate, then a
/// per-relay GCRA token — and a relay-URL-aware `ScriptedGraph` as the fetch. If
/// the global -> per-relay -> GCRA order ever formed a cycle, a fan-out at
/// `per_relay_concurrency = 1` would hang; the bounded timeout fails the test on
/// a hang. A clean completion proves the order is deadlock-free.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_deadlock_single_permit() {
    use std::collections::HashMap;
    use std::num::NonZeroU32;
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::sync::Semaphore;
    use web_of_trust::relay::health::admit_per_relay;
    use web_of_trust::relay::rate_limit::{
        RateLimiterRegistry, DEFAULT_BACKOFF_BASE, DEFAULT_BACKOFF_CAP,
    };

    const PER_RELAY_CONCURRENCY: usize = 1;
    const RELAYS: [&str; 3] = ["wss://a.example", "wss://b.example", "wss://c.example"];

    // Each relay holds the same author's kind-3 (the fetch content is irrelevant —
    // the test asserts on COMPLETION, not on edges). The graph is relay-URL-aware.
    let author = common::keys(7).public_key();
    let kind3 = follows_event(7, &[8, 9], 1_000);
    let placements: Vec<(&str, Vec<_>)> =
        RELAYS.iter().map(|r| (*r, vec![kind3.clone()])).collect();
    let graph = ScriptedGraph::with_relay(placements);

    // Shared health + a FIXED-SIZE per-relay semaphore map (size 1 — the
    // deadlock-prone case), exactly as the daemon builds them.
    let health = Arc::new(RelayHealthRegistry::new(DEFAULT_HEALTH_ALPHA));
    let per_relay_sems: Arc<HashMap<String, Arc<Semaphore>>> = Arc::new(
        RELAYS
            .iter()
            .map(|r| (r.to_string(), Arc::new(Semaphore::new(PER_RELAY_CONCURRENCY))))
            .collect(),
    );
    // Per-relay GCRA registry (the third gate), mirroring the daemon.
    let registry = Arc::new(RateLimiterRegistry::with_params(
        NonZeroU32::new(1000).unwrap(),
        DEFAULT_BACKOFF_BASE,
        DEFAULT_BACKOFF_CAP,
    ));

    // The GLOBAL crawl permit (loop_.rs:99) — acquired BEFORE the per-relay permit
    // in every task, so the fixed order is: global -> per-relay -> GCRA -> fetch.
    let global = Arc::new(Semaphore::new(2));

    // Fan out 8 concurrent batches contending for 3 single-permit relays + 2
    // global permits — the worst case for an ordering bug.
    let mut handles = Vec::new();
    for _ in 0..8 {
        let health = Arc::clone(&health);
        let per_relay_sems = Arc::clone(&per_relay_sems);
        let registry = Arc::clone(&registry);
        let global = Arc::clone(&global);
        let relay_fetch = graph.relay_fetch_fn();
        handles.push(tokio::spawn(async move {
            // (global crawl permit) — held for the whole batch, as in loop_.rs.
            let _g = global.acquire().await.expect("global never closed");
            for r in RELAYS {
                let sem = per_relay_sems.get(r).expect("relay has a semaphore");
                let registry = Arc::clone(&registry);
                let relay_fetch = relay_fetch.clone();
                // (per-relay permit) -> (GCRA token) -> (fetch) inside admit.
                let batch = vec![ClaimedAuthor {
                    id: 0,
                    pubkey: author.to_bytes().to_vec(),
                }];
                let _ = admit_per_relay(&health, sem, r, PER_RELAY_CONCURRENCY, || async {
                    // GCRA token (third gate) BEFORE the fetch.
                    registry.acquire(r).await?;
                    relay_fetch(r.to_string(), batch.clone()).await
                })
                .await;
            }
        }));
    }

    // A hang = deadlock. A clean join within the bound = deadlock-free.
    let all = async {
        for h in handles {
            h.await.expect("worker did not panic");
        }
    };
    tokio::time::timeout(Duration::from_secs(10), all)
        .await
        .expect("fan-out at per_relay_concurrency=1 must complete (no deadlock)");
}
