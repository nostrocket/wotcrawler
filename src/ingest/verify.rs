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
///
/// `Event::verify()` (NOT `verify_signature()` alone) is used so that a relay
/// returning an event whose stored id does not match its content is caught: the
/// method recomputes the id AND checks the secp256k1 signature (RESEARCH
/// Pattern 4). No field is `unwrap()`-ed — adversarial input must never panic
/// (V5/V7).
pub fn accept(event: &Event, want_kind: Kind, requested: &HashSet<PublicKey>) -> bool {
    // Step 1: id recomputation + secp256k1 signature. A forged event (content
    // mutated after signing) fails id recomputation here.
    if event.verify().is_err() {
        metrics::counter!("ingest_invalid_signature").increment(1);
        return false;
    }

    // Step 2: relays are adversarial and may inject events of a kind we never
    // asked for, or authored by a pubkey outside the requested set (Pitfall 4).
    if event.kind != want_kind || !requested.contains(&event.pubkey) {
        metrics::counter!("ingest_unsolicited").increment(1);
        return false;
    }

    true
}
