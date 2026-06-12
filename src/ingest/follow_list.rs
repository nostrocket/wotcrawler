//! p-tag extraction, dedup, self-drop, and bounded cap (INGEST-04).
//!
//! Given the winning kind-3 event, extract its followee pubkeys via
//! `Tags::public_keys()` (which skips malformed/non-standard p-tags
//! automatically — Pitfall 6), dedup the set (a kind-3 may legally repeat a
//! p-tag), drop self-follows (D-08, defense in depth — the store drops them
//! too), and enforce the configurable `follow_cap`. Per the resolved Open
//! Question 4, the default disposition on an oversized list is REJECT-and-count
//! (`ingest_oversized_follow_list`), not silent truncation, because a 50k-tag
//! event is almost certainly a follow-bomb. Relay hints + petnames on p-tags
//! are discarded — only the pubkey set crosses into the store (D-06).
//!
//! Stub body in plan 02-01; implemented in plan 02-02 Task 3.

use std::collections::HashSet;

use nostr_sdk::{Event, PublicKey};

/// Extract the bounded, deduplicated, self-filtered followee set from a winning
/// kind-3 event.
///
/// Returns `Some(pubkeys)` when the extracted list is within `follow_cap`, or
/// `None` when it exceeds the cap (the default reject-and-count disposition,
/// Open Question 4). Never `unwrap()`s on tag contents — adversarial input must
/// never panic the pipeline.
///
/// Pipeline:
/// - `Tags::public_keys()` skips malformed/non-standard p-tags automatically
///   (Pitfall 6 — no manual tag walking, no panic on a malformed tag).
/// - self-follows are dropped (D-08, defense in depth — the store drops them
///   too).
/// - repeated p-tags are deduplicated (a kind-3 may legally repeat a p-tag).
/// - relay hints and petnames on p-tags are discarded — only the pubkey set
///   crosses into the store (D-06).
/// - the resulting set is bounded by `follow_cap`; exceeding it rejects the
///   whole list (truncation silently corrupts the graph) and counts
///   `ingest_oversized_follow_list`.
pub fn followee_pubkeys(event: &Event, follow_cap: usize) -> Option<Vec<PublicKey>> {
    let mut seen: HashSet<PublicKey> = HashSet::new();
    let mut followees: Vec<PublicKey> = Vec::new();

    for pk in event.tags.public_keys() {
        // Self-drop (D-08).
        if *pk == event.pubkey {
            continue;
        }
        // Dedup repeated p-tags.
        if seen.insert(*pk) {
            followees.push(*pk);
        }
    }

    // Bound the list. Default disposition is reject-and-count (Open Question 4):
    // a 50k-tag event is almost certainly a follow-bomb, and silently truncating
    // would corrupt the graph by dropping real follows.
    if followees.len() > follow_cap {
        metrics::counter!("ingest_oversized_follow_list").increment(1);
        return None;
    }

    Some(followees)
}
