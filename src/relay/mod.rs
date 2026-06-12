//! Relay-acquisition layer (Phase 2, RELAY-01..04).
//!
//! This module is the "acquisition half" of the crawler. It owns the policy
//! that sits on top of nostr-sdk's transport: connecting the curated relay set
//! with an explicit reconnect policy (RELAY-01), discovering and caching each
//! relay's NIP-11 advertised limits ([`nip11`], RELAY-02), the per-relay
//! `governor` rate limiter and rate-limited-notice backoff ([`rate_limit`],
//! RELAY-04), and the author-chunked `until`-window pagination loop that never
//! trusts EOSE as a completeness signal ([`fetch`], RELAY-03).
//!
//! Delegation split: nostr-sdk owns websocket framing, reconnect, secp256k1,
//! and relay-message parsing; this module owns the four acquisition policies
//! above plus the fetch→ingest seam (wired by plan 02-04). Bodies are stubs in
//! plan 02-01; plan 02-03 fills them.

pub mod fetch;
pub mod nip11;
pub mod rate_limit;

use std::collections::HashSet;
use std::future::Future;
use std::time::Duration;

use nostr_sdk::{Client, Event, Kind, PublicKey, RelayOptions, Timestamp};

use crate::error::RelayError;
use crate::ingest::{self, ValidatedFollowList};

/// Default base retry interval handed to nostr-sdk's socket reconnect.
///
/// Mirrors nostr-relay-pool's own `DEFAULT_RETRY_INTERVAL` (10s). The SDK layers
/// its LINEAR `1 + (attempts-successes)/2` growth + ±3s jitter on top (02-SPIKES
/// RELAY-01); the *exponential* backoff RELAY-01 mandates lives at the
/// acquisition layer in [`rate_limit::backoff_delay`], not here.
pub const DEFAULT_RETRY_INTERVAL: Duration = Duration::from_secs(10);

/// Reconnect policy applied to every relay in the curated set (RELAY-01).
///
/// This is a thin, *inspectable* wrapper over [`RelayOptions`] — the SDK's own
/// fields are `pub(super)` and so cannot be asserted on from outside the crate,
/// which is why the connect path is configured through this struct and tests
/// assert on it directly. [`Self::to_relay_options`] produces the `RelayOptions`
/// the pool actually receives.
///
/// The socket-level reconnect handled here is LINEAR + jittered (the SDK
/// default we keep enabled). RELAY-01's exponential-with-jitter requirement is
/// satisfied separately, at the fetch re-arm / rate-limited-notice layer, by
/// [`rate_limit::backoff_delay`] (02-SPIKES RELAY-01).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconnectPolicy {
    /// Whether nostr-sdk auto-reconnects dropped sockets. Always `true` for the
    /// crawler — we never want a silently-dead relay to stay dead (Pitfall 9).
    pub reconnect: bool,
    /// Base retry interval the SDK grows from.
    pub retry_interval: Duration,
    /// Whether the SDK adapts the retry interval by failure count (its linear
    /// growth + jitter). Kept `true` so repeated failures don't hammer a relay.
    pub adjust_retry_interval: bool,
}

impl ReconnectPolicy {
    /// The crawler's default reconnect policy: reconnect on, 10s base, adaptive.
    pub const fn crawler_default() -> Self {
        Self {
            reconnect: true,
            retry_interval: DEFAULT_RETRY_INTERVAL,
            adjust_retry_interval: true,
        }
    }

    /// Build the [`RelayOptions`] the pool receives from this policy.
    pub fn to_relay_options(&self) -> RelayOptions {
        RelayOptions::new()
            .reconnect(self.reconnect)
            .retry_interval(self.retry_interval)
            .adjust_retry_interval(self.adjust_retry_interval)
    }
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self::crawler_default()
    }
}

/// Connect a configurable curated relay set with the reconnect policy (RELAY-01).
///
/// Builds a signer-less nostr-sdk [`Client`] (the crawler is read-only), adds
/// every url in `relays` to the pool with `policy`'s [`RelayOptions`] (so the
/// SDK socket reconnect is enabled), then calls `connect()` (non-blocking — the
/// pool manages sockets + reconnect in the background). Returns the connected
/// client for the fetch loop ([`fetch`]) to drive.
///
/// The curated set is passed in now as `&[String]`; config-sourcing it is
/// OPS-01 (later phase). A url that fails to add surfaces as [`RelayError`].
pub async fn connect_curated(
    relays: &[String],
    policy: ReconnectPolicy,
) -> Result<Client, RelayError> {
    let client = Client::builder().build();
    let opts = policy.to_relay_options();
    for url in relays {
        // pool().add_relay applies our custom RelayOptions; Client::add_relay
        // would silently use the client-default options instead.
        client
            .pool()
            .add_relay(url.as_str(), opts.clone())
            .await
            .map_err(nostr_sdk::client::Error::from)?;
    }
    client.connect().await;
    Ok(client)
}

/// The fetch -> ingest seam: turn a raw relay event stream into validated
/// follow lists (the Phase 2 goal made observable end-to-end).
///
/// This is the connection plans 02-02 (ingest gate) and 02-03 (relay transport)
/// build *toward* but deliberately leave unwired — the RESEARCH anti-pattern
/// "building the two halves but never connecting them". This function owns NO
/// validation logic of its own: it runs the raw, still-unverified events
/// produced by the pagination loop ([`fetch`], RELAY-03) through the ingest
/// orchestrator ([`ingest::ingest_events`], INGEST-01..05) so that — and only
/// then — a [`ValidatedFollowList`] emerges. verify / dedup / replaceable
/// resolve / follow-list bounds all live in [`crate::ingest`]; the seam merely
/// composes fetch -> ingest.
///
/// `fetch` is the raw-event source. In production this is a closure over
/// [`fetch::fetch_complete`] driving the connected [`Client`]
/// (see [`acquire_validated_lists_client`]); in tests it is the in-process
/// scripted mock relay (`tests/mock_relay`). Either way the seam consumes the
/// FULL paged output — count-vs-cap page-back (RELAY-03) is handled inside
/// `fetch` *before* this seam sees the stream — and hands the entire union to a
/// single resolution pass, so a relay cannot split a pubkey's events across
/// window boundaries to defeat newest-wins (T-02-15).
///
/// `requested` is the set of authors actually solicited; the ingest gate drops
/// events from any other author as unsolicited (INGEST-01 / Pitfall 4 —
/// T-02-14). `want_kind`, `now`, `future_clamp_secs`, and `follow_cap` are
/// passed straight through to [`ingest::ingest_events`].
///
/// A fetch failure surfaces as [`RelayError`] (never swallowed); the ingest
/// gate's routine adversarial-input rejections are count-and-skip inside the
/// orchestrator and so do not produce an error here.
pub async fn acquire_validated_lists<F, Fut>(
    requested: &HashSet<PublicKey>,
    want_kind: Kind,
    now: Timestamp,
    future_clamp_secs: u64,
    follow_cap: usize,
    fetch: F,
) -> Result<Vec<ValidatedFollowList>, RelayError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Vec<Event>, RelayError>>,
{
    // 1. Acquire the raw, deduplicated, still-UNVERIFIED event stream. All
    //    RELAY-03 paging happens inside `fetch`; the seam never re-implements it.
    let raw_events: Vec<Event> = fetch().await?;

    // 2. Route EVERY raw event through the ingest gate — there is no direct path
    //    from fetch to ValidatedFollowList (T-02-14). The orchestrator verifies
    //    id+sig, drops unsolicited authors/kinds, dedups by id, resolves the
    //    replaceable winner over the full union, and bounds the follow list.
    Ok(ingest::ingest_events(
        raw_events,
        want_kind,
        requested,
        now,
        future_clamp_secs,
        follow_cap,
    ))
}

/// [`acquire_validated_lists`] driven by a live, connected [`Client`].
///
/// The production entry point: it closes over [`fetch::fetch_complete`] (the
/// author-chunked until-window pagination loop, RELAY-03) so the curated pool
/// supplies the raw stream, then composes it through the ingest gate. The
/// requested-author set handed to the ingest gate is exactly `authors` — the
/// set the fetch actually solicited.
#[allow(clippy::too_many_arguments)]
pub async fn acquire_validated_lists_client(
    client: &Client,
    authors: &[PublicKey],
    want_kind: Kind,
    max_limit: usize,
    max_authors: usize,
    now: Timestamp,
    future_clamp_secs: u64,
    follow_cap: usize,
) -> Result<Vec<ValidatedFollowList>, RelayError> {
    let requested: HashSet<PublicKey> = authors.iter().copied().collect();
    acquire_validated_lists(
        &requested,
        want_kind,
        now,
        future_clamp_secs,
        follow_cap,
        || fetch::fetch_complete(client, authors, want_kind, max_limit, max_authors),
    )
    .await
}
