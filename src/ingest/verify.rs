//! Signature-verification gate + kind/author match (INGEST-01).
//!
//! The first step of ingest, run on every event from any relay before dedup or
//! resolution. `Event::verify()` recomputes the id and verifies the secp256k1
//! signature (delegated to nostr-sdk — NEVER hand-roll crypto, CLAUDE.md);
//! then the event's kind and author are matched against what was actually
//! requested, because relays are adversarial and may inject unsolicited events
//! (Pitfall 4). Failures are counted via `metrics` and the event is skipped
//! (the gate returns `false`), never propagated as an error (see
//! [`crate::error::IngestError`]).
//!
//! Stub body in plan 02-01; implemented in plan 02-02 Task 1.

use std::collections::HashSet;

use nostr_sdk::{Event, Kind, PublicKey};

/// Accept an event only if it verifies AND is a solicited (kind, author) pair.
///
/// Returns `true` for events that pass `Event::verify()` (id + sig) and whose
/// `kind == want_kind` and `pubkey ∈ requested`; otherwise increments the
/// relevant `metrics` counter (`ingest_invalid_signature` / `ingest_unsolicited`)
/// and returns `false`.
pub fn accept(_event: &Event, _want_kind: Kind, _requested: &HashSet<PublicKey>) -> bool {
    todo!("plan 02-02 Task 1: Event::verify() + kind/author match gate")
}
