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
/// Pipeline (plan 02-02 fills the body): verify each event ([`verify::accept`]),
/// drop unsolicited kinds/authors, dedup by id, group by author, resolve the
/// replaceable winner per author ([`replaceable::pick_winner`]), extract and
/// bound its followees ([`follow_list::followee_pubkeys`]), and build the
/// contract value. `want_kind` is `Kind::ContactList` (3) or `Kind::RelayList`
/// (10002, validated under the identical rules — INGEST-05).
pub fn ingest_events(
    _events: impl IntoIterator<Item = Event>,
    _want_kind: Kind,
    _now: Timestamp,
    _future_clamp_secs: u64,
    _follow_cap: usize,
) -> Vec<ValidatedFollowList> {
    todo!("plan 02-02: orchestrate verify -> dedup -> resolve -> extract -> emit")
}
