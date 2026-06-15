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
use crate::ingest::relay_list::resolve_relay_list;
use crate::ingest::ValidatedFollowList;
use crate::relay::acquire_validated_lists;
use crate::relay::health::RelayHealthRegistry;
use crate::store::follows::apply_follow_list;
use crate::store::pubkeys::{set_fetch_status, upsert_pubkey};
use crate::store::relays::{apply_relay_list, lookup_write_relays};

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
/// - relays answered but no list for that author -> the RELAY-05 NIP-65 write-
///   relay fallback (see below) when `fallback_enabled`; only a fallback miss
///   stamps terminal `not_found` via [`set_fetch_status`] (D-10; stamps
///   `last_fetched_at`, FRESH-01).
/// - a genuine transient `RelayError` for the whole batch fetch -> every claimed
///   author goes through [`requeue_or_fail`] (D-09; the terminal `failed` path
///   stamps `last_fetched_at`, FRESH-01).
///
/// # RELAY-05 NIP-65 fallback (the `None` arm)
///
/// When the curated set has no kind-3 for an author and `fallback_enabled`, the
/// author's NIP-65 write relays are tried before stamping `not_found`:
///
/// 1. Look up the author's stored write relays
///    ([`crate::store::relays::lookup_write_relays`]; a bare r-tag = `both`
///    counts as a write relay).
/// 2. If none are known, run an ON-DEMAND PLAIN CURATED kind:10002 fetch via the
///    injected `relay_list_fetch` closure, re-resolve its winner through the same
///    verify/dedup/newest-wins gate ([`resolve_relay_list`]), persist it
///    ([`crate::store::relays::apply_relay_list`] — this is the SOLE
///    persist-on-kind:10002-winner-seen hook in the phase, since the fan-out
///    never passively fetches relay lists), then re-read the write relays. A
///    missing/failed on-demand fetch yields no write relays and falls straight to
///    `not_found` WITHOUT consuming the kind-3 retry budget (Open Question 1).
///    This on-demand fetch is a plain curated fetch, NOT routed back through the
///    kind-3 fallback — no recursion (Pitfall 4).
/// 3. Prefer healthier write relays (`health.score` desc) and cap to
///    `nip65_max_write_relays`.
/// 4. Fetch kind-3 from the selected write relays via the injected
///    `fallback_fetch` closure, then re-resolve the raw union through ONE
///    single-author [`acquire_validated_lists`] pass — write relays are just as
///    adversarial as curated ones, so verify/dedup/newest-wins/clamp MUST run.
/// 5. A recovered list -> [`apply_validated`] + `nip65_recovered` counter (NO
///    manual `_total` — the exporter appends it, WR-02). A miss (fetch `Err` or
///    empty after re-resolve) -> terminal `not_found`.
///
/// Both `union_fetch` and `fallback_fetch`/`relay_list_fetch` are injected
/// closures so `apply.rs` never imports the live relay `Client` (no circular dep)
/// and the whole path is `ScriptedGraph`-testable offline.
///
/// `now` is the wall clock of this fetch attempt (passed explicitly so the caller
/// owns the clock and tests are deterministic). Returns the number of authors
/// that resolved to a winning list (applied via the curated set OR recovered via
/// the fallback), for crawl bookkeeping.
#[allow(clippy::too_many_arguments)]
pub async fn process_batch<F, Fut, FB, FutB, RL, FutR>(
    pool: &PgPool,
    batch: &[ClaimedAuthor],
    want_kind: Kind,
    now: Timestamp,
    future_clamp_secs: u64,
    follow_cap: usize,
    max_attempts: i16,
    fallback_enabled: bool,
    nip65_max_write_relays: usize,
    health: &RelayHealthRegistry,
    union_fetch: F,
    fallback_fetch: FB,
    relay_list_fetch: RL,
) -> Result<usize, StoreError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Vec<Event>, crate::error::RelayError>>,
    FB: Fn(PublicKey, Vec<String>) -> FutB,
    FutB: Future<Output = Result<Vec<Event>, crate::error::RelayError>>,
    RL: Fn(PublicKey) -> FutR,
    FutR: Future<Output = Result<Vec<Event>, crate::error::RelayError>>,
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
            // Curated set answered but had no kind-3 for this author. Before
            // stamping the terminal `not_found` (D-10), try the RELAY-05 NIP-65
            // write-relay fallback when enabled.
            None => {
                let recovered = if fallback_enabled {
                    fallback_recover(
                        pool,
                        claimed,
                        author,
                        want_kind,
                        now,
                        future_clamp_secs,
                        follow_cap,
                        nip65_max_write_relays,
                        health,
                        &fallback_fetch,
                        &relay_list_fetch,
                    )
                    .await?
                } else {
                    None
                };

                match recovered {
                    Some(vfl) => {
                        apply_validated(pool, &vfl).await?;
                        // Un-suffixed: the Prometheus exporter appends `_total`
                        // (WR-02) -> exposed as `nip65_recovered_total`.
                        metrics::counter!("nip65_recovered").increment(1);
                        applied += 1;
                    }
                    // Curated miss AND fallback miss (or fallback disabled) ->
                    // terminal not_found (set_fetch_status stamps last_fetched_at,
                    // FRESH-01).
                    None => {
                        set_fetch_status(pool, claimed.id, "not_found", stamp).await?;
                    }
                }
            }
        }
    }

    Ok(applied)
}

/// Attempt the RELAY-05 NIP-65 write-relay recovery for a single curated-miss
/// author, returning the recovered [`ValidatedFollowList`] on a hit or `None` on
/// a miss (the caller then stamps terminal `not_found`).
///
/// Flow (05-RESEARCH Pattern 4; see [`process_batch`] for the full contract):
/// resolve write relays (on-demand-resolve+persist a curated kind:10002 when
/// unknown — the sole kind:10002 persist hook, plain curated fetch, no
/// recursion), prefer healthier relays capped at `nip65_max_write_relays`, fetch
/// kind-3 via the injected closure, and re-resolve through ONE single-author
/// [`acquire_validated_lists`] pass (write relays are adversarial — the full gate
/// must still run). A missing/failed on-demand kind:10002 fetch proceeds to
/// `None` WITHOUT consuming the kind-3 retry budget (Open Question 1).
#[allow(clippy::too_many_arguments)]
async fn fallback_recover<FB, FutB, RL, FutR>(
    pool: &PgPool,
    claimed: &ClaimedAuthor,
    author: PublicKey,
    want_kind: Kind,
    now: Timestamp,
    future_clamp_secs: u64,
    follow_cap: usize,
    nip65_max_write_relays: usize,
    health: &RelayHealthRegistry,
    fallback_fetch: &FB,
    relay_list_fetch: &RL,
) -> Result<Option<ValidatedFollowList>, StoreError>
where
    FB: Fn(PublicKey, Vec<String>) -> FutB,
    FutB: Future<Output = Result<Vec<Event>, crate::error::RelayError>>,
    RL: Fn(PublicKey) -> FutR,
    FutR: Future<Output = Result<Vec<Event>, crate::error::RelayError>>,
{
    // 1. Resolve the author's stored write relays.
    let mut write_relays = lookup_write_relays(pool, claimed.id).await?;

    // 2. If unknown, run a PLAIN curated on-demand kind:10002 fetch (NOT routed
    //    through this fallback — no recursion, Pitfall 4), re-resolve its winner
    //    through the same verify/dedup/newest-wins gate, persist it (the single
    //    CONTEXT persist-on-winner-seen hook), then re-read the write relays. A
    //    missing/failed fetch yields no write relays and falls to `not_found`
    //    WITHOUT consuming the kind-3 retry budget (Open Question 1).
    if write_relays.is_empty() {
        if let Ok(raw) = relay_list_fetch(author).await {
            if let Some(vrl) = resolve_relay_list(raw, author, now, future_clamp_secs) {
                let pubkey_id = upsert_pubkey(pool, &author.to_bytes()).await?;
                apply_relay_list(pool, pubkey_id, &vrl.relays, vrl.created_at).await?;
            }
        }
        write_relays = lookup_write_relays(pool, claimed.id).await?;
    }

    if write_relays.is_empty() {
        return Ok(None);
    }

    // 3. Prefer healthier write relays, then cap to nip65_max_write_relays.
    write_relays.sort_by(|a, b| health.score(b).total_cmp(&health.score(a)));
    write_relays.truncate(nip65_max_write_relays);

    // 4. Fetch kind-3 from the selected write relays via the injected closure,
    //    then re-resolve through ONE single-author acquire_validated_lists pass
    //    (write relays are adversarial — verify/dedup/newest-wins/clamp run).
    let raw = match fallback_fetch(author, write_relays).await {
        Ok(events) => events,
        Err(_) => return Ok(None),
    };

    let one: HashSet<PublicKey> = HashSet::from([author]);
    let resolved = acquire_validated_lists(
        &one,
        want_kind,
        now,
        future_clamp_secs,
        follow_cap,
        || std::future::ready(Ok(raw)),
    )
    .await;

    // A re-resolve error or an empty result is a miss -> None (caller stamps
    // not_found). On a hit, return the single recovered list.
    Ok(resolved.ok().and_then(|mut v| v.pop()))
}
