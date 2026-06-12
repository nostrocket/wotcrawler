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

use std::time::Duration;

use nostr_sdk::{Client, RelayOptions};

use crate::error::RelayError;

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
