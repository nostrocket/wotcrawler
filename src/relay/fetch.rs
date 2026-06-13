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
use crate::relay::rate_limit::RateLimiterRegistry;

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
    // The `until` used by the PREVIOUS fetch, so this iteration can tell a fresh
    // page-back (until advanced) from a re-request of the SAME pinned boundary
    // second (until unchanged). `None` before the first fetch (CR-03 residual).
    let mut prev_until: Option<Timestamp> = None;
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
        // The `until` this fetch will carry — captured before the fetch so the
        // stall check below compares THIS window's until against the PRIOR one.
        let current_until = until;
        let filter = Filter::new()
            .authors(authors.iter().copied())
            .kind(kind)
            .limit(cap)
            .until(current_until);
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

        // Zero new ids: either genuine exhaustion OR an unresolvable boundary
        // stall (CR-03 residual / T-02-15 / CR-01-new, 02-VERIFICATION.md
        // gaps_remaining). Distinguish them:
        //
        // - Boundary-second STALL — the window is still capped (`returned >=
        //   cap`, so the relay may be truncating), the re-request yielded nothing
        //   new (`new_ids == 0`), AND page-back would re-pin the SAME `until`
        //   rather than advance it (`page_back(returned, cap, oldest) ==
        //   Some(current_until)`). The latter fires on the FIRST capped
        //   zero-new-id re-request of a pinned boundary second — including the
        //   no-newer-event case (CR-01-new) where EVERY pool event shares the
        //   boundary second `T`, so `until` becomes `T` on the first page-back and
        //   the relay re-serves the same cap-sized prefix while siblings at `T`
        //   remain. The prior `prev_until == Some(current_until)` 2-visit guard is
        //   retained (OR-combined) as the union/superset case; the page_back check
        //   is the stronger first-visit detector and no longer depends on
        //   prev_until. A deterministic newest-first relay re-serving the SAME
        //   cap-sized prefix for the pinned `until=T` while more events remain
        //   would silently truncate the follow list, so surface a requeue Err.
        //
        // - Genuine EXHAUSTION — any other zero-new-id case. A short window
        //   (`returned < cap`) makes page_back return None, so the stall condition
        //   is false. A window whose oldest event is at a second OLDER than
        //   `current_until` makes page_back return `Some(older)` != current_until
        //   (the window genuinely advanced into older data), so zero new ids there
        //   is the real end of data. Break with Ok — never turn legitimate
        //   exhaustion into an error.
        if new_ids == 0 {
            if returned >= cap
                && (prev_until == Some(current_until)
                    || page_back(returned, cap, oldest) == Some(current_until))
            {
                return Err(RelayError::FetchTimeout(format!(
                    "boundary-second stall: relay re-served the same cap-sized \
                     prefix for pinned until={} with more events remaining",
                    current_until.as_secs()
                )));
            }
            break;
        }
        match page_back(returned, cap, oldest) {
            Some(next) => until = next,
            None => break,
        }
        // Record the until THIS fetch used, so the next iteration can detect a
        // pinned (unchanged) boundary second vs a fresh page-back.
        prev_until = Some(current_until);
    }
    Ok(out)
}

/// [`paginate_chunk`] with every window REQ gated behind the per-relay GCRA
/// limiter (WR-03 / RELAY-04 — production wiring).
///
/// This is the production-and-testable seam the 02-VERIFICATION.md data-flow
/// trace flagged DISCONNECTED: the rate limiter existed but no production caller
/// reached [`RateLimiterRegistry::acquire`]. Here every window REQ first awaits
/// `registry.acquire(relay_url)`, so the politeness gate (threat T-02-10)
/// actually runs in production — not only in the limiter's own unit tests. The
/// injected `fetch` is wrapped so the gate sits immediately before each REQ; the
/// count-vs-cap page-back logic is unchanged ([`paginate_chunk`] still owns it).
///
/// `cap` is the effective per-window cap (the relay's cached NIP-11 `max_limit`
/// from [`super::nip11::LimitCache`], sourced by the caller — see
/// [`super::acquire_validated_lists_client`]).
pub async fn paginate_chunk_gated<F, Fut>(
    authors: &[PublicKey],
    kind: Kind,
    cap: usize,
    registry: &RateLimiterRegistry,
    relay_url: &str,
    mut fetch: F,
) -> Result<Vec<Event>, RelayError>
where
    F: FnMut(Filter) -> Fut,
    Fut: Future<Output = Result<Vec<Event>, RelayError>>,
{
    paginate_chunk(authors, kind, cap, move |filter| {
        // Gate BEFORE the REQ: await the per-relay token so every outbound
        // window passes the GCRA quota. The limiter for `relay_url` is shared
        // across all callers (CR-05), so concurrent chunks obey one quota.
        //
        // The future produced here re-borrows `fetch` for the duration of one
        // window (paginate_chunk awaits it before the next call), so the
        // `&mut fetch` cannot outlive the FnMut body across calls.
        let fut = fetch(filter);
        async move {
            registry.acquire(relay_url).await?;
            fut.await
        }
    })
    .await
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
///
/// `relay_url` is the caller's INDIVIDUAL relay url (threaded from
/// [`super::acquire_validated_lists_client`]). It is the per-relay GCRA limiter
/// key AND the [`RelayError::FetchTimeout`] label, so each relay drives its own
/// quota (WR-03 residual / RELAY-04 / threats T-02-10, T-02-17). It is NOT a
/// joined pool string — [`pool_label`] is diagnostics-only.
#[allow(clippy::too_many_arguments)]
pub async fn fetch_complete(
    client: &Client,
    relay_url: &str,
    authors: &[PublicKey],
    kind: Kind,
    max_limit: usize,
    max_authors: usize,
    registry: &RateLimiterRegistry,
) -> Result<Vec<Event>, RelayError> {
    fetch_complete_with_timeout(
        client,
        relay_url,
        authors,
        kind,
        max_limit,
        max_authors,
        DEFAULT_FETCH_TIMEOUT,
        registry,
    )
    .await
}

/// [`fetch_complete`] with an explicit per-fetch timeout (Pitfall 9).
///
/// `relay_url` is the caller's INDIVIDUAL relay url, used as BOTH the per-relay
/// GCRA limiter key (via [`paginate_chunk_gated`]) AND the
/// [`RelayError::FetchTimeout`] label. Keying on the individual url — not a
/// joined pool string — means each relay has its own quota and a relay
/// drop/reconnect preserves accrued GCRA state instead of minting a fresh
/// full-burst limiter (WR-03 residual / RELAY-04 / threats T-02-10, T-02-17).
/// `pool_label` is computed only for human-readable diagnostics in the
/// FetchTimeout message; it is NEVER passed to `registry.acquire()`.
///
/// `registry` gates every window REQ behind the per-relay GCRA limiter
/// ([`paginate_chunk_gated`], WR-03 / RELAY-04): the politeness quota actually
/// runs in production, not only in the limiter's unit tests. `max_limit` is the
/// effective per-window cap — the caller ([`super::acquire_validated_lists_client`])
/// sources it from the relay's NIP-11 [`super::nip11::LimitCache`] so the cap is
/// the relay's real, bounded limit (WR-03 / RELAY-02 / threat T-02-13).
#[allow(clippy::too_many_arguments)]
pub async fn fetch_complete_with_timeout(
    client: &Client,
    relay_url: &str,
    authors: &[PublicKey],
    kind: Kind,
    max_limit: usize,
    max_authors: usize,
    timeout: Duration,
    registry: &RateLimiterRegistry,
) -> Result<Vec<Event>, RelayError> {
    let cap = max_limit.max(1);
    let chunk_size = max_authors.max(1);
    // Diagnostics ONLY (T-02-01): the joined connected-pool urls give the
    // operator pool-level context, folded into the FetchTimeout label below.
    // They NEVER key the limiter or the requeue identity — that is the per-relay
    // `relay_url` parameter. Keying the limiter on this joined string was the
    // WR-03 residual bug (T-02-10/T-02-17): it collapsed every relay into one
    // shared quota and reset GCRA state on pool churn.
    let pool_diag = pool_label(client).await;
    // The requeue label leads with the individual relay_url (the actionable
    // key) and appends the pool context for the operator.
    let timeout_label = format!("{relay_url} (pool: {pool_diag})");
    let mut all: Vec<Event> = Vec::new();
    for chunk in authors.chunks(chunk_size) {
        // Gate every window REQ behind the per-relay limiter (WR-03 / T-02-10),
        // keyed on the caller's individual relay_url.
        let events = paginate_chunk_gated(chunk, kind, cap, registry, relay_url, |filter| {
            let timeout_label = timeout_label.as_str();
            // Every fetch carries the deadline; a timed-out window is a requeue,
            // not completion (Pitfall 9). The SDK returns a partial Ok on
            // timeout, so fetch_window_with_deadline's elapsed check — not EOSE,
            // not an SDK error — is what surfaces RelayError::FetchTimeout. The
            // label leads with the per-relay url, enriched with pool context.
            fetch_window_with_deadline(filter, timeout, timeout_label, |filter| async move {
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

/// A human-readable label for the connected pool, used only for diagnostics
/// (tracing / operator context). Joins the pool's relay urls; never embeds
/// secrets (T-02-01) and NEVER keys the per-relay limiter — that is the
/// individual `relay_url` threaded from the caller.
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
