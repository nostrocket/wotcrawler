//! Concurrent reader+writer integration test (GRAPH-03, D-14 — success criterion 3).
//!
//! Proves Postgres MVCC delivers the cross-process read/write guarantee: a
//! writer task continuously upserts edges while a reader on a SEPARATE pool runs
//! contract-view queries, and neither blocks the other. Requires Docker.

mod common;

use std::time::Duration;

use chrono::Utc;
use web_of_trust::store::{self, follows::apply_follow_list, pubkeys::upsert_pubkey};

fn pk(seed: u16) -> [u8; 32] {
    let mut k = [0u8; 32];
    k[0] = (seed & 0xff) as u8;
    k[1] = (seed >> 8) as u8;
    k
}

#[tokio::test]
async fn reader_and_writer_do_not_block() -> anyhow::Result<()> {
    let (_pg, url) = common::start_postgres().await?;

    // Writer pool: runs migrations and the continuous edge-upsert loop.
    let writer_pool = store::connect(&url).await?;
    store::run_migrations(&writer_pool).await?;

    // Seed a follower plus a handful of followees the writer churns over.
    let follower = upsert_pubkey(&writer_pool, &pk(1)).await?;
    let mut followees = Vec::new();
    for i in 2..12u16 {
        followees.push(upsert_pubkey(&writer_pool, &pk(i)).await?);
    }

    // Writer task: loop applying follow lists that alternate the edge set so the
    // diff genuinely DELETEs + INSERTs rows on every iteration. Each apply uses a
    // fresh event id so it never hits the zero-touch short circuit.
    let writer = {
        let pool = writer_pool.clone();
        let followees = followees.clone();
        tokio::spawn(async move {
            let mut n: u8 = 0;
            loop {
                let half = if n % 2 == 0 {
                    &followees[..5]
                } else {
                    &followees[5..]
                };
                // Distinct event id per iteration.
                let mut event = [0u8; 32];
                event[31] = n;
                let _ = apply_follow_list(&pool, follower, &event, Utc::now(), half).await;
                n = n.wrapping_add(1);
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
    };

    // Reader pool: a SEPARATE PgPool. Distinct pools against the same database
    // exercise the same per-connection MVCC isolation path that a separate
    // OS process (the spam layer) would — a faithful proxy for criterion-3
    // fidelity without spawning a second process (RESEARCH Open Question Q2).
    let reader_pool = store::connect(&url).await?;

    // 100 concurrent contract-view reads; each must return Ok without hanging on
    // the writer. A timeout guards against a (regression) blocking read.
    for _ in 0..100u32 {
        let read = sqlx::query("SELECT follower_id, followee_id FROM follow_edges LIMIT 1000")
            .fetch_all(&reader_pool);
        let rows = tokio::time::timeout(Duration::from_secs(5), read)
            .await
            .expect("reader timed out — it blocked on the concurrent writer (GRAPH-03 regression)")
            .expect("reader query failed");
        // Result may be empty mid-diff; the assertion is that the read returned.
        let _ = rows.len();
    }

    writer.abort();
    Ok(())
}
