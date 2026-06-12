//! Author-chunked `until`-window pagination loop (RELAY-03).
//!
//! Defeats the EOSE-completeness trap (Pitfall 1): `fetch_events` auto-closes
//! on EOSE, but a relay silently caps at `max_limit`, so a returned window that
//! equals the cap is NOT proof of completeness. This loop chunks the author set
//! under `max_authors`, pages backwards with `until = oldest - 1`, and only
//! stops a window when it returns strictly fewer than the effective cap
//! (`min(requested_limit, relay_max_limit)`). Every fetch carries a timeout
//! (Pitfall 9); a timed-out window surfaces as [`RelayError::FetchTimeout`] so
//! the caller requeues those authors rather than treating them as done.
//!
//! The page-back decision is factored into [`paginate_chunk`] / [`page_back`]
//! over an injected async fetch fn, so the count-vs-cap logic is exercised
//! offline without a live websocket (see `tests/mock_relay`). Production
//! ([`fetch_complete`]) injects `Client::fetch_events`.
//!
//! This task emits ONLY the raw, still-unverified deduplicated event stream;
//! wiring it into the ingest gate is plan 02-04 (Wave 3).
//!
//! Implemented in plan 02-03 Task 3.

use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use nostr_sdk::{Client, Event, EventId, Filter, Kind, PublicKey, Timestamp};

use crate::error::RelayError;

/// Default per-fetch deadline (Pitfall 9). Config-overridable later (OPS-01).
pub const DEFAULT_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// The page-back decision for a single window (RELAY-03 / Pitfall 1).
///
/// Given how many events a window returned and the oldest `created_at` seen,
/// decide whether to fetch another (older) page and, if so, the next `until`.
/// EOSE is never consulted — only the count-vs-cap comparison drives this:
/// `>= cap` means the relay may have silently truncated, so page back; strictly
/// fewer than the cap means the window is genuinely exhausted.
///
/// Returns `Some(next_until)` to page again, `None` to stop this author chunk.
pub fn page_back(returned: usize, cap: usize, oldest: Option<Timestamp>) -> Option<Timestamp> {
    match (returned >= cap, oldest) {
        // Capped window: there may be more older events. Page back one second
        // before the oldest event so the next window does not re-include it.
        (true, Some(ts)) => Some(Timestamp::from_secs(ts.as_secs().saturating_sub(1))),
        // Fewer than the cap (or no events): the window is complete.
        _ => None,
    }
}

/// Paginate one author chunk over an injected async fetch fn (RELAY-03).
///
/// `fetch` receives the built [`Filter`] (carrying `until`/`limit`) and returns
/// the window's events. The loop applies [`page_back`] until a window returns
/// fewer than `cap` events. The effective cap is the caller's `cap`
/// (`min(requested_limit, relay_max_limit)`). Returns this chunk's raw events.
pub async fn paginate_chunk<F, Fut>(
    authors: &[PublicKey],
    kind: Kind,
    cap: usize,
    mut fetch: F,
) -> Result<Vec<Event>, RelayError>
where
    F: FnMut(Filter) -> Fut,
    Fut: Future<Output = Result<Vec<Event>, RelayError>>,
{
    let mut out: Vec<Event> = Vec::new();
    let mut until = Timestamp::now();
    loop {
        let filter = Filter::new()
            .authors(authors.iter().copied())
            .kind(kind)
            .limit(cap)
            .until(until);
        let events = fetch(filter).await?;
        let returned = events.len();
        let oldest = events.iter().map(|e| e.created_at).min();
        out.extend(events);
        match page_back(returned, cap, oldest) {
            Some(next) => until = next,
            None => break,
        }
    }
    Ok(out)
}

/// Deduplicate events by id (a pubkey can appear across author chunks / windows,
/// and the same event can arrive from multiple relays — Pitfall: cross-source
/// duplicates). Keeps first occurrence.
fn dedup_by_id(events: Vec<Event>) -> Vec<Event> {
    let mut seen: HashMap<EventId, ()> = HashMap::with_capacity(events.len());
    let mut out = Vec::with_capacity(events.len());
    for ev in events {
        if seen.insert(ev.id, ()).is_none() {
            out.push(ev);
        }
    }
    out
}

/// Fetch every stored event of `kind` for `authors` from the connected pool,
/// paging past relay `max_limit` caps until each author chunk's window is
/// exhausted (RELAY-03).
///
/// `max_limit` is the effective per-window cap (relay NIP-11 `max_limit` from
/// [`super::nip11`]); `max_authors` is the author-chunk size kept under the
/// relay cap. Every `fetch_events` carries [`DEFAULT_FETCH_TIMEOUT`]; a timed-out
/// window surfaces as [`RelayError::FetchTimeout`] so the caller requeues those
/// authors (Pitfall 9) rather than recording them complete. Returns the raw,
/// id-deduplicated, still-unverified events for the ingest gate
/// ([`crate::ingest`], wired in plan 02-04) to validate.
pub async fn fetch_complete(
    client: &Client,
    authors: &[PublicKey],
    kind: Kind,
    max_limit: usize,
    max_authors: usize,
) -> Result<Vec<Event>, RelayError> {
    fetch_complete_with_timeout(client, authors, kind, max_limit, max_authors, DEFAULT_FETCH_TIMEOUT)
        .await
}

/// [`fetch_complete`] with an explicit per-fetch timeout (Pitfall 9).
pub async fn fetch_complete_with_timeout(
    client: &Client,
    authors: &[PublicKey],
    kind: Kind,
    max_limit: usize,
    max_authors: usize,
    timeout: Duration,
) -> Result<Vec<Event>, RelayError> {
    let cap = max_limit.max(1);
    let chunk_size = max_authors.max(1);
    let mut all: Vec<Event> = Vec::new();
    for chunk in authors.chunks(chunk_size) {
        let events = paginate_chunk(chunk, kind, cap, |filter| async move {
            // Every fetch carries the deadline; a timed-out window is a requeue,
            // not completion (Pitfall 9). fetch_events auto-closes on EOSE, but
            // page_back — not EOSE — decides completeness.
            let events = client
                .fetch_events(filter, timeout)
                .await
                .map_err(RelayError::Client)?;
            Ok::<_, RelayError>(events.into_iter().collect())
        })
        .await?;
        all.extend(events);
    }
    Ok(dedup_by_id(all))
}
