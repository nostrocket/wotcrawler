//! NIP-11 relay-information discovery + limit cache (RELAY-02).
//!
//! There is no nostr-sdk 0.44 accessor for a relay's NIP-11 document
//! (02-SPIKES RELAY-02): `RelayInformationDocument` is parse-only. So
//! [`fetch_limits`] does the HTTP GET the bundled `nostr` example does — convert
//! the relay's `wss://`/`ws://` url to its `https`/`http` origin, request it with
//! `Accept: application/nostr+json`, and parse the body with `from_json`.
//!
//! The `limitation` block (and every field in it) is optional; a relay may ship
//! a hostile or absent document. Every field falls back to a conservative
//! documented default ([`DEFAULT_MAX_LIMIT`] etc.), and non-positive advertised
//! values are treated as "use the default" — relay-supplied numbers are never
//! `unwrap()`ed (threat T-02-13). A missing/invalid document is non-fatal: the
//! crawler proceeds with defaults rather than aborting.
//!
//! [`LimitCache`] memoizes the per-relay [`RelayLimits`] so each relay's document
//! is fetched once and reused; the cached `max_limit` is the effective per-window
//! pagination cap consumed by [`super::fetch`].
//!
//! Implemented in plan 02-03 Task 2.

use std::collections::HashMap;
use std::sync::Mutex;

use nostr_sdk::nips::nip11::RelayInformationDocument;
use nostr_sdk::JsonUtil;

use crate::error::RelayError;

/// Default `max_limit` when a relay omits it (02-SPIKES RELAY-02).
///
/// The de-facto common relay cap; feeds the pagination planner's effective
/// per-window cap `min(requested_limit, relay_max_limit)`.
pub const DEFAULT_MAX_LIMIT: usize = 500;

/// Default `max_subscriptions` when a relay omits it (02-SPIKES RELAY-02).
///
/// Conservative lower bound so the crawler never assumes more concurrent REQs
/// than a silent relay likely allows.
pub const DEFAULT_MAX_SUBSCRIPTIONS: usize = 20;

/// Default `max_filters` when a relay omits it (02-SPIKES RELAY-02).
///
/// Conservative; with the one-filter-per-REQ author-chunked design the crawler
/// stays well under any real relay's filter cap.
pub const DEFAULT_MAX_FILTERS: usize = 10;

/// Per-relay NIP-11 limits the pagination planner consumes.
///
/// Populated from the relay's `limitation` block; any field a relay omits — or
/// advertises as non-positive — falls back to the documented default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelayLimits {
    /// The maximum number of events a relay returns per REQ (`max_limit`).
    pub max_limit: usize,
    /// The maximum number of concurrent subscriptions (`max_subscriptions`).
    pub max_subscriptions: usize,
    /// The maximum number of filters per REQ (`max_filters`).
    pub max_filters: usize,
}

impl RelayLimits {
    /// The all-defaults limits, used when a relay has no NIP-11 document.
    pub const fn defaults() -> Self {
        Self {
            max_limit: DEFAULT_MAX_LIMIT,
            max_subscriptions: DEFAULT_MAX_SUBSCRIPTIONS,
            max_filters: DEFAULT_MAX_FILTERS,
        }
    }
}

impl Default for RelayLimits {
    fn default() -> Self {
        Self::defaults()
    }
}

/// Clamp one advertised `Option<i32>` limit to a usable `usize`, defaulting on
/// absence or a non-positive (adversarial) value.
fn clamp_limit(advertised: Option<i32>, default: usize) -> usize {
    match advertised {
        Some(v) if v > 0 => v as usize,
        _ => default,
    }
}

/// Read [`RelayLimits`] out of a parsed [`RelayInformationDocument`].
///
/// Pure: no I/O. A document with no `limitation` block yields all defaults; a
/// block omitting (or non-positively advertising) any field defaults that field.
/// This is the seam the offline parse test drives.
pub fn limits_from_doc(doc: &RelayInformationDocument) -> RelayLimits {
    match &doc.limitation {
        None => RelayLimits::defaults(),
        Some(lim) => RelayLimits {
            max_limit: clamp_limit(lim.max_limit, DEFAULT_MAX_LIMIT),
            max_subscriptions: clamp_limit(lim.max_subscriptions, DEFAULT_MAX_SUBSCRIPTIONS),
            max_filters: clamp_limit(lim.max_filters, DEFAULT_MAX_FILTERS),
        },
    }
}

/// Parse a raw NIP-11 JSON body into [`RelayLimits`], applying defaults.
///
/// Pure: no I/O. Drives both [`fetch_limits`] and the offline parse test.
pub fn limits_from_json(body: &str) -> Result<RelayLimits, RelayError> {
    let doc = RelayInformationDocument::from_json(body).map_err(|e| RelayError::Nip11Fetch {
        relay: "<json>".to_string(),
        reason: format!("invalid NIP-11 document: {e}"),
    })?;
    Ok(limits_from_doc(&doc))
}

/// Convert a relay's `ws(s)://` url to the `http(s)://` origin its NIP-11
/// document is served from. A url without a recognized ws scheme is returned
/// unchanged (best-effort — the GET will surface any real failure).
fn nip11_http_url(relay_url: &str) -> String {
    if let Some(rest) = relay_url.strip_prefix("wss://") {
        format!("https://{rest}")
    } else if let Some(rest) = relay_url.strip_prefix("ws://") {
        format!("http://{rest}")
    } else {
        relay_url.to_string()
    }
}

/// Fetch and parse a relay's NIP-11 limits over HTTP (RELAY-02).
///
/// GETs the relay's `http(s)` origin with `Accept: application/nostr+json`, then
/// extracts the `limitation` fields with omitted/non-positive fields defaulted.
/// A network failure or unparseable body surfaces as [`RelayError::Nip11Fetch`];
/// callers treat that as non-fatal and fall back to [`RelayLimits::defaults`].
pub async fn fetch_limits(relay_url: &str) -> Result<RelayLimits, RelayError> {
    let http_url = nip11_http_url(relay_url);
    let client = reqwest::Client::new();
    let resp = client
        .get(&http_url)
        .header(reqwest::header::ACCEPT, "application/nostr+json")
        .send()
        .await
        .map_err(|e| RelayError::Nip11Fetch {
            relay: relay_url.to_string(),
            reason: format!("HTTP request failed: {e}"),
        })?;
    let body = resp.text().await.map_err(|e| RelayError::Nip11Fetch {
        relay: relay_url.to_string(),
        reason: format!("reading response body failed: {e}"),
    })?;
    limits_from_json(&body).map_err(|_| RelayError::Nip11Fetch {
        relay: relay_url.to_string(),
        reason: "response was not a valid NIP-11 document".to_string(),
    })
}

/// Per-relay NIP-11 limit cache: fetch each relay's document once, reuse it.
///
/// [`Self::get_or_fetch`] returns the cached [`RelayLimits`] if present,
/// otherwise fetches them (falling back to defaults on any fetch failure, so the
/// crawl is never blocked by a bad/absent NIP-11 document) and memoizes the
/// result. Cheap to share behind an `Arc`.
#[derive(Default)]
pub struct LimitCache {
    entries: Mutex<HashMap<String, RelayLimits>>,
}

impl LimitCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the cached limits for a relay, or fetch + cache them.
    ///
    /// On a fetch failure the relay is cached with [`RelayLimits::defaults`] so a
    /// hostile/absent document degrades gracefully and is not re-fetched on every
    /// call.
    pub async fn get_or_fetch(&self, relay_url: &str) -> RelayLimits {
        if let Some(limits) = self
            .entries
            .lock()
            .expect("limit cache not poisoned")
            .get(relay_url)
            .copied()
        {
            return limits;
        }
        let limits = fetch_limits(relay_url)
            .await
            .unwrap_or_else(|_| RelayLimits::defaults());
        self.entries
            .lock()
            .expect("limit cache not poisoned")
            .insert(relay_url.to_string(), limits);
        limits
    }

    /// Directly seed a relay's limits (used in tests and when limits are known
    /// out-of-band). Returns the previous value if any.
    pub fn insert(&self, relay_url: &str, limits: RelayLimits) -> Option<RelayLimits> {
        self.entries
            .lock()
            .expect("limit cache not poisoned")
            .insert(relay_url.to_string(), limits)
    }

    /// The cached limits for a relay without fetching, if present.
    pub fn get(&self, relay_url: &str) -> Option<RelayLimits> {
        self.entries
            .lock()
            .expect("limit cache not poisoned")
            .get(relay_url)
            .copied()
    }
}
