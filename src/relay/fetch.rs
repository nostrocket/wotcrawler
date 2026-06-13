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

use std::collections::HashSet;
use std::future::Future;
use std::time::{Duration, Instant};

use nostr_sdk::{Client, Event, EventId, Filter, Kind, PublicKey, Timestamp};

use crate::error::RelayError;

/// Default per-fetch deadline (Pitfall 9). Config-overridable later (OPS-01).
pub const DEFAULT_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Hard per-author-chunk page budget (CR-04 / T-02-16).
///
/// The inclusive boundary page-back (CR-03) plus the cross-window dedup make a
/// well-behaved relay terminate via the zero-new-id guard. But an adversarial
/// relay can ignore `until` and return a full-cap window of *new* ids forever,
/// driving an unbounded loop with unbounded `out` growth. This budget caps the
/// number of windows paged per author chunk: when reached, `paginate_chunk`
/// errors so the caller requeues rather than looping or exhausting memory.
///
/// Sized generously: against the relay `max_limit` cap (≤ DEFAULT_MAX_LIMIT =
/// 500 in [`super::nip11`]), the absolute worst-case `out` for one chunk before
/// the error fires is `MAX_PAGES_PER_CHUNK * cap` ≈ 5M events — far above any
/// legitimate per-author follow-list history, so this never truncates honest
/// pagination, yet bounds a hostile relay to a finite, recoverable failure.
pub const MAX_PAGES_PER_CHUNK: usize = 10_000;

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
        // Capped window: there may be more older events. Page back INCLUSIVELY
        // to the oldest event's second (CR-03). An exclusive `oldest - 1` would
        // skip any sibling event sharing that same second that the relay's cap
        // cut off, opening a permanent hole at the boundary second. Returning
        // `oldest` re-requests that second; `paginate_chunk` dedups the already-
        // seen oldest event by id and its zero-new-id guard stops the loop when
        // the boundary re-request yields nothing genuinely new.
        (true, Some(ts)) => Some(ts),
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
    // Cross-window seen-set (CR-03/CR-04): the inclusive boundary page-back
    // re-serves the oldest event of the prior window, and a relay may echo the
    // same id across windows. Dedup by id so the union holds each event once,
    // and use the count of NEWLY-seen ids as the page-progress signal.
    let mut seen: HashSet<EventId> = HashSet::new();
    let mut until = Timestamp::now();
    let mut pages: usize = 0;
    loop {
        // Hard page budget (CR-04 / T-02-16): a relay ignoring `until` and
        // returning full-cap windows of new ids would otherwise loop forever and
        // grow `out` without bound. Error out so the caller requeues.
        if pages >= MAX_PAGES_PER_CHUNK {
            return Err(RelayError::FetchTimeout(format!(
                "page budget ({MAX_PAGES_PER_CHUNK}) exceeded for author chunk"
            )));
        }
        let filter = Filter::new()
            .authors(authors.iter().copied())
            .kind(kind)
            .limit(cap)
            .until(until);
        let events = fetch(filter).await?;
        pages += 1;
        let returned = events.len();
        let oldest = events.iter().map(|e| e.created_at).min();

        // Keep only ids not yet seen across all prior windows; the NEW-id count
        // drives progress, not the raw returned count.
        let mut new_ids = 0usize;
        for ev in events {
            if seen.insert(ev.id) {
                out.push(ev);
                new_ids += 1;
            }
        }

        // Zero new ids means the (possibly capped) boundary re-request returned
        // only already-seen events — genuine exhaustion. Stop even when
        // `returned >= cap`, so a relay echoing a full duplicate window cannot
        // keep the loop alive (CR-04).
        if new_ids == 0 {
            break;
        }
        match page_back(returned, cap, oldest) {
            Some(next) => until = next,
            None => break,
        }
    }
    Ok(out)
}

/// Run one window fetch under a deadline, converting a partial-`Ok` timeout into
/// an explicit [`RelayError::FetchTimeout`] requeue signal (CR-02 / Pitfall 9).
///
/// nostr-relay-pool 0.44.1 drops the activity sender when the per-fetch timeout
/// fires and the event stream simply ends — `client.fetch_events` returns a
/// partial `Ok`, NOT an error. Treating that partial window as "complete" would
/// silently record a truncated follow list. This wrapper records the wall-clock
/// start, awaits the injected `fetch`, and if the elapsed time meets or exceeds
/// the deadline it returns `Err(FetchTimeout(relay_url))` so the caller requeues
/// those authors. The elapsed check is the ONLY reliable timeout signal here.
pub async fn fetch_window_with_deadline<F, Fut>(
    filter: Filter,
    timeout: Duration,
    relay_url: &str,
    fetch: F,
) -> Result<Vec<Event>, RelayError>
where
    F: FnOnce(Filter) -> Fut,
    Fut: Future<Output = Result<Vec<Event>, RelayError>>,
{
    let started = Instant::now();
    let events = fetch(filter).await?;
    if started.elapsed() >= timeout {
        return Err(RelayError::FetchTimeout(relay_url.to_string()));
    }
    Ok(events)
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
/// still-unverified events for the ingest gate ([`crate::ingest`]) to validate.
///
/// No pre-verify dedup is performed here (CR-01 fetch half): authoritative
/// cross-source dedup must follow `verify::accept` in the ingest gate, or a
/// hostile relay's forged id-squat copy could suppress the genuine event before
/// its signature is ever checked. Within a chunk, [`paginate_chunk`] dedups by
/// id only to drive page progress, never across the verification boundary.
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
    // Label for FetchTimeout: the connected pool's relay urls. A timeout is a
    // pool-level requeue signal (the SDK fans the fetch across the pool); listing
    // the connected relays gives the operator the actionable context without
    // embedding secrets (T-02-01).
    let relay_url = pool_label(client).await;
    let mut all: Vec<Event> = Vec::new();
    for chunk in authors.chunks(chunk_size) {
        let events = paginate_chunk(chunk, kind, cap, |filter| {
            let relay_url = relay_url.as_str();
            // Every fetch carries the deadline; a timed-out window is a requeue,
            // not completion (Pitfall 9). The SDK returns a partial Ok on
            // timeout, so fetch_window_with_deadline's elapsed check — not EOSE,
            // not an SDK error — is what surfaces RelayError::FetchTimeout.
            fetch_window_with_deadline(filter, timeout, relay_url, |filter| async move {
                let events = client
                    .fetch_events(filter, timeout)
                    .await
                    .map_err(RelayError::Client)?;
                Ok::<_, RelayError>(events.into_iter().collect())
            })
        })
        .await?;
        all.extend(events);
    }
    // No pre-verify dedup (CR-01): cross-source dedup happens AFTER verification
    // in the ingest gate. Return the raw union.
    Ok(all)
}

/// A human-readable label for the connected pool, used only for
/// [`RelayError::FetchTimeout`] context. Joins the pool's relay urls; never
/// embeds secrets (T-02-01).
async fn pool_label(client: &Client) -> String {
    let relays = client.relays().await;
    if relays.is_empty() {
        return "pool (no connected relays)".to_string();
    }
    relays
        .keys()
        .map(|url| url.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}
