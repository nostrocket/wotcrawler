//! The per-batch composition seam: acquire → union → ingest → upsert → apply.
//!
//! This module wires the Phase 2 validation output ([`ValidatedFollowList`])
//! into the Phase 1 transactional edge writer
//! ([`crate::store::follows::apply_follow_list`]) and resolves the per-author
//! terminal/retry status for a claimed batch. It owns NO validation, NO raw SQL,
//! and NO replaceable-event resolution — those live in
//! [`crate::ingest::ingest_events`] (CONSUMED via
//! [`crate::relay::acquire_validated_lists`]) and the store helpers (CONSUMED).
//! The new value here is the *composition*:
//!
//! 1. [`apply_validated`] — the bridge: resolve the follower id + every followee
//!    id via [`crate::store::pubkeys::upsert_pubkey`] (the upsert IS discovery,
//!    D-03 — new followees land `discovered`, which is what makes CRAWL-02
//!    structural), then hand the resolved ids to the unmodified
//!    [`crate::store::follows::apply_follow_list`].
//! 2. [`process_batch`] — for a claimed batch, fan out across the curated relays
//!    collecting a RAW `Vec<Event>` union, run that union through a SINGLE
//!    [`crate::relay::acquire_validated_lists`] pass (D-08 — `ingest_events`
//!    runs ONCE over the cross-relay union, never per relay), index the result
//!    by author, then resolve each claimed author's terminal status: a hit ->
//!    [`apply_validated`] (status becomes `fetched` inside the writer); relays
//!    answered but no list for that author -> `not_found` (D-10); a genuine
//!    transient `RelayError` for the batch -> [`requeue_or_fail`] (D-09). Every
//!    terminal path routes through a stamping write so FRESH-01 holds.

use std::collections::{HashMap, HashSet};
use std::future::Future;

use chrono::{DateTime, Utc};
use nostr_sdk::{Event, Kind, PublicKey, Timestamp};
use sqlx::PgPool;

use crate::crawl::frontier::{requeue_or_fail, ClaimedAuthor};
use crate::error::StoreError;
use crate::ingest::ValidatedFollowList;
use crate::relay::acquire_validated_lists;
use crate::store::follows::apply_follow_list;
use crate::store::pubkeys::{set_fetch_status, upsert_pubkey};

/// Bridge a validated follow list into the transactional edge writer.
///
/// Resolves `vfl.follower_pubkey` to its surrogate id and every
/// `vfl.followee_pubkeys` entry to a followee id via
/// [`crate::store::pubkeys::upsert_pubkey`] — and that upsert IS the
/// discovery/enqueue mechanism (D-03): a previously-unseen followee lands as a
/// `discovered` row, which is the *only* insertion path besides the anchor seed,
/// so CRAWL-02 holds structurally — no reach-ability column or SQL predicate.
///
/// Then calls the unmodified [`crate::store::follows::apply_follow_list`] with
/// the resolved ids and returns its `bool` (whether the edge set / applied event
/// changed). The writer stamps `status = 'fetched'` + `last_fetched_at` itself
/// (FRESH-01), drops self-follows (D-08), and short-circuits to zero edge touches
/// on an unchanged event id (GRAPH-02 idempotency).
pub async fn apply_validated(
    pool: &PgPool,
    vfl: &ValidatedFollowList,
) -> Result<bool, StoreError> {
    let follower_id = upsert_pubkey(pool, &vfl.follower_pubkey.to_bytes()).await?;

    let mut followee_ids = Vec::with_capacity(vfl.followee_pubkeys.len());
    for fp in &vfl.followee_pubkeys {
        // CRAWL-02 (structural reachability): upsert_pubkey-on-followee is the
        // ONLY way a non-anchor pubkey becomes a `discovered` row. Discovery is a
        // side effect of writing the edge, not a separate enqueue step (D-03).
        followee_ids.push(upsert_pubkey(pool, &fp.to_bytes()).await?);
    }

    // The edge writer is CONSUMED unmodified — apply.rs orchestrates, it never
    // writes raw edge SQL (RESEARCH "consume, do not rebuild").
    apply_follow_list(
        pool,
        follower_id,
        vfl.event_id.as_bytes(),
        vfl.created_at,
        &followee_ids,
    )
    .await
}

/// Process one claimed batch: fan-out → union → single-ingest → per-author
/// terminal resolution.
///
/// `union_fetch` is the injected raw-event source: it returns the RAW
/// `Vec<Event>` union across ALL curated relays for `requested`. In production
/// this closure fans out per relay (each call rate-limits + paginates internally
/// via [`crate::relay::acquire_validated_lists_client`]'s fetch path) and
/// concatenates the raw events; in tests it is a deterministic offline scripted
/// source. Either way the union is run through ONE
/// [`crate::relay::acquire_validated_lists`] pass so `ingest_events` resolves
/// newest-wins over the whole cross-relay union exactly once (D-08), never per
/// relay.
///
/// Per-author resolution (D-07/D-09/D-10):
/// - a winning [`ValidatedFollowList`] for the author -> [`apply_validated`]
///   (the writer flips status to `fetched` + stamps `last_fetched_at`).
/// - relays answered but no list for that author -> `not_found` via
///   [`set_fetch_status`] (D-10; stamps `last_fetched_at`, FRESH-01).
/// - a genuine transient `RelayError` for the whole batch fetch -> every claimed
///   author goes through [`requeue_or_fail`] (D-09; the terminal `failed` path
///   stamps `last_fetched_at`, FRESH-01).
///
/// `now` is the wall clock of this fetch attempt (passed explicitly so the caller
/// owns the clock and tests are deterministic). Returns the number of authors
/// that resolved to a winning list (applied), for crawl bookkeeping.
#[allow(clippy::too_many_arguments)]
pub async fn process_batch<F, Fut>(
    pool: &PgPool,
    batch: &[ClaimedAuthor],
    want_kind: Kind,
    now: Timestamp,
    future_clamp_secs: u64,
    follow_cap: usize,
    max_attempts: i16,
    union_fetch: F,
) -> Result<usize, StoreError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Vec<Event>, crate::error::RelayError>>,
{
    if batch.is_empty() {
        return Ok(0);
    }

    // The author set this batch actually solicited — the ingest gate drops any
    // event from an unsolicited author (INGEST-01 / Pitfall 4). Convert each
    // claimed bytea pubkey to a nostr PublicKey at this fetch boundary.
    let requested: HashSet<PublicKey> = batch
        .iter()
        .filter_map(|c| PublicKey::from_slice(&c.pubkey).ok())
        .collect();

    // SINGLE ingest pass over the cross-relay raw union (D-08): acquire the raw
    // events from the injected fan-out closure, then run ingest_events ONCE.
    let now_ts = now;
    let result = acquire_validated_lists(
        &requested,
        want_kind,
        now_ts,
        future_clamp_secs,
        follow_cap,
        union_fetch,
    )
    .await;

    let stamp: DateTime<Utc> = crate::ingest::timestamp_to_datetime(now);

    let validated = match result {
        Ok(lists) => lists,
        Err(_relay_err) => {
            // A genuine transient RelayError for the batch fetch (D-09): every
            // claimed author is requeued (under cap) or terminally `failed`
            // (at cap, stamping last_fetched_at — FRESH-01). The ingest gate's
            // count-and-skip rejections never surface here as an error.
            for claimed in batch {
                requeue_or_fail(pool, claimed.id, max_attempts, stamp).await?;
            }
            return Ok(0);
        }
    };

    // Index the winning lists by their author so per-claimed-author resolution is
    // an O(1) lookup. The ingest gate already deduped to at most one winner per
    // author (newest-wins), so a HashMap keyed by pubkey is exact.
    let mut by_author: HashMap<PublicKey, &ValidatedFollowList> =
        HashMap::with_capacity(validated.len());
    for vfl in &validated {
        by_author.insert(vfl.follower_pubkey, vfl);
    }

    // The fetch returned Ok, so the relays answered (D-10): an author with no
    // winning list in the union is `not_found`, not a transient failure.
    let mut applied = 0usize;
    for claimed in batch {
        let author = match PublicKey::from_slice(&claimed.pubkey) {
            Ok(pk) => pk,
            // A row whose stored bytea is not a valid 32-byte x-only key can never
            // match a verified event; treat it as not_found so it terminates with
            // a stamp rather than bouncing forever.
            Err(_) => {
                set_fetch_status(pool, claimed.id, "not_found", stamp).await?;
                continue;
            }
        };

        match by_author.get(&author) {
            Some(vfl) => {
                apply_validated(pool, vfl).await?;
                applied += 1;
            }
            // Relays answered, no kind-3 for this author -> terminal not_found
            // (D-10; set_fetch_status stamps last_fetched_at, FRESH-01).
            None => {
                set_fetch_status(pool, claimed.id, "not_found", stamp).await?;
            }
        }
    }

    Ok(applied)
}
