//! Validation / ingest layer (Phase 2, INGEST-01..05) and the phase output
//! contract.
//!
//! Delegation split: nostr-sdk owns `Event::verify()` (id recomputation +
//! secp256k1 signature), p-tag parsing (`Tags::public_keys()`), and within-call
//! dedup (the `Events` set); this module owns the gate *orchestration*:
//!
//!   verify ([`verify`]) -> kind/author match -> dedup seen-set ->
//!   replaceable resolve ([`replaceable`]) -> p-tag extract/bound
//!   ([`follow_list`]) -> emit a [`ValidatedFollowList`].
//!
//! Routine adversarial-input rejections are counted via `metrics` and skipped
//! (the gate returns `false`), never propagated as [`crate::error::IngestError`]
//! — see that enum's doc comment for the split.
//!
//! Bodies are stubs in plan 02-01; plan 02-02 fills the orchestrator and the
//! submodule gates, and plan 02-04 calls [`ingest_events`] from the fetch path.

pub mod follow_list;
pub mod replaceable;
pub mod verify;

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, TimeZone, Utc};
use nostr_sdk::{Event, EventId, Kind, PublicKey, Timestamp};

/// The Phase 2 output contract: a verified, deduplicated, newest-wins follow
/// list ready to hand to the Phase 1 store writer.
///
/// This is the seam between the acquisition/validation halves of the crawler
/// and `store::follows::apply_follow_list`. The four fields map directly onto
/// that writer's arguments:
/// - [`follower_pubkey`](Self::follower_pubkey) -> resolved to `follower_id`
///   via `store::pubkeys::upsert_pubkey` by the caller (plan 02-04 / Phase 3).
/// - [`event_id`](Self::event_id) -> `apply_follow_list`'s `event_id: &[u8]`
///   (the winning replaceable event's id; feeds the GRAPH-02 idempotency
///   short-circuit). Convert with [`EventId::as_bytes`].
/// - [`created_at`](Self::created_at) -> `apply_follow_list`'s
///   `created_at: DateTime<Utc>`. Stored already-converted from the nostr
///   `Timestamp` (see [`ValidatedFollowList::from_event`] /
///   [`timestamp_to_datetime`]) so the store boundary never re-derives it.
/// - [`followee_pubkeys`](Self::followee_pubkeys) -> each resolved to a
///   `followee_id` (relay hints + petnames discarded at this boundary, D-06).
#[derive(Debug, Clone)]
pub struct ValidatedFollowList {
    /// The author of the winning follow-list event (kind 3 / kind 10002).
    pub follower_pubkey: PublicKey,
    /// The winning replaceable event's id (newest-wins, lowest-id tie-break).
    pub event_id: EventId,
    /// The winning event's `created_at`, converted to the `DateTime<Utc>` the
    /// store writer requires.
    pub created_at: DateTime<Utc>,
    /// The deduplicated, self-filtered, capped set of followed pubkeys.
    pub followee_pubkeys: Vec<PublicKey>,
}

/// Convert a nostr `Timestamp` (seconds since the Unix epoch) to the
/// `chrono::DateTime<Utc>` the Phase 1 store writer requires.
///
/// nostr `Timestamp` is a non-negative seconds count, so the conversion is
/// total; an out-of-range value (only possible for absurd far-future inputs
/// already rejected by the future clamp in [`replaceable`]) saturates to the
/// epoch rather than panicking.
pub fn timestamp_to_datetime(ts: Timestamp) -> DateTime<Utc> {
    Utc.timestamp_opt(ts.as_secs() as i64, 0)
        .single()
        .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().expect("epoch is valid"))
}

impl ValidatedFollowList {
    /// Assemble the contract value from a winning event and its extracted,
    /// already-bounded followee set.
    ///
    /// The caller (the [`ingest_events`] orchestrator, plan 02-02) supplies the
    /// event that won replaceable resolution and the followee set produced by
    /// [`follow_list`]; this constructor only performs the `Timestamp` ->
    /// `DateTime<Utc>` conversion so the conversion lives in exactly one place.
    pub fn from_event(event: &Event, followee_pubkeys: Vec<PublicKey>) -> Self {
        Self {
            follower_pubkey: event.pubkey,
            event_id: event.id,
            created_at: timestamp_to_datetime(event.created_at),
            followee_pubkeys,
        }
    }
}

/// Run a batch of raw, untrusted relay events through the full validation gate
/// and emit one [`ValidatedFollowList`] per author whose newest valid event of
/// `want_kind` survives.
///
/// Pipeline: verify + kind/author gate ([`verify::accept`], INGEST-01)
/// -> dedup by id (cross-relay [`HashSet<EventId>`] seen-set, INGEST-02; runs
/// AFTER verify so only verified ids occupy the seen-set, CR-01/T-02-14)
/// -> group by
/// author -> resolve the replaceable winner per author ([`replaceable::pick_winner`],
/// INGEST-03/05) -> extract and bound its followees
/// ([`follow_list::followee_pubkeys`], INGEST-04) -> build the contract value.
///
/// `want_kind` is `Kind::ContactList` (3) or `Kind::RelayList` (10002, validated
/// under the identical rules — INGEST-05). `requested` is the set of authors the
/// fetch actually asked for; events from any other author are dropped as
/// unsolicited (Pitfall 4). `now` + `future_clamp_secs` drive the future clamp;
/// `follow_cap` bounds an oversized follow list.
///
/// Because every event in a group has already passed the gate (verified id +
/// matching kind + solicited author), the same resolver and extractor handle
/// kind:3 and kind:10002 identically — for kind:10002 the returned
/// [`ValidatedFollowList::followee_pubkeys`] are the relay-list p-tag pubkeys;
/// callers wanting the relay urls re-parse the winning event.
pub fn ingest_events(
    events: impl IntoIterator<Item = Event>,
    want_kind: Kind,
    requested: &HashSet<PublicKey>,
    now: Timestamp,
    future_clamp_secs: u64,
    follow_cap: usize,
) -> Vec<ValidatedFollowList> {
    // Cross-relay dedup: each event id is processed at most once even if two
    // relays return it (INGEST-02). `fetch_events` dedupes within one call; this
    // covers the multi-call / multi-relay path.
    let mut seen: HashSet<EventId> = HashSet::new();

    // Group surviving events by author so the replaceable resolver runs per
    // pubkey. All events here share `want_kind` (the gate enforces it), so the
    // (pubkey, kind) grouping collapses to grouping by pubkey.
    let mut by_author: HashMap<PublicKey, Vec<Event>> = HashMap::new();

    for event in events {
        // SECURITY-CRITICAL ORDERING (CR-01 / T-02-14): dedup MUST follow
        // verification, never precede it. Verify FIRST so only events with a
        // recomputed-matching id + valid signature can reach the seen-set. A
        // hostile relay can forge an event that claims a genuine event's id
        // (id-squat); if dedup ran first, that forged id would enter the
        // seen-set, the forgery would then fail verify, and the genuine copy
        // arriving later would be skipped as a "duplicate" — inverting
        // INGEST-02 into a censorship primitive. Gating insertion on
        // `verify::accept` means a forged copy never consumes the id.
        if !verify::accept(&event, want_kind, requested) {
            continue; // forged or unsolicited — counted inside the gate.
        }
        if !seen.insert(event.id) {
            continue; // genuine duplicate id (verified) — already handled.
        }
        by_author.entry(event.pubkey).or_default().push(event);
    }

    let mut out: Vec<ValidatedFollowList> = Vec::with_capacity(by_author.len());

    for (_author, group) in by_author {
        // Resolve the replaceable winner (future-clamp + newest-wins + lowest-id
        // tie-break). `None` means every candidate was future-dated junk.
        let Some(winner) = replaceable::pick_winner(group.iter(), now, future_clamp_secs) else {
            continue;
        };

        // Extract the bounded, deduped, self-dropped followee set. `None` means
        // the list exceeded `follow_cap` (rejected + counted) — drop the author.
        let Some(followees) = follow_list::followee_pubkeys(winner, follow_cap) else {
            continue;
        };

        out.push(ValidatedFollowList::from_event(winner, followees));
    }

    out
}
