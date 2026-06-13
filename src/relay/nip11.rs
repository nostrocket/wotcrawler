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
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use nostr_sdk::nips::nip11::RelayInformationDocument;
use nostr_sdk::JsonUtil;

use crate::error::RelayError;

/// Maximum NIP-11 response body the crawler will buffer/parse (CR-06, T-02-19).
///
/// A relay's information document is a small JSON object; 64 KiB is generous.
/// A body larger than this is rejected before it is buffered or parsed so a
/// hostile relay cannot stream an arbitrarily large payload to exhaust memory.
pub const MAX_NIP11_BYTES: usize = 64 * 1024;

/// Process-shared HTTP client for NIP-11 fetches, built once (WR / CR-06).
///
/// Carries a request `.timeout` and a `.connect_timeout` (project Pitfall 9 —
/// every network fetch carries a deadline) so a relay that accepts the
/// connection and never responds, or never completes the TLS handshake, cannot
/// hang the crawler (T-02-18). Built once via `LazyLock`, not per call.
static NIP11_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(5))
        .build()
        .expect("static reqwest client builds")
});

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

/// Upper bound on an advertised `max_limit` (WR-02, T-02-13, Pitfall 1).
///
/// A relay's advertised `max_limit` is clamped down to this ceiling. Real relays
/// cap REQ results in the low hundreds to low thousands; 5000 is comfortably
/// above any honest cap (and 10x [`DEFAULT_MAX_LIMIT`]) while small enough that a
/// hostile relay advertising e.g. `2_000_000_000` cannot produce a per-window cap
/// so large that the count-vs-cap pagination loop treats a single EOSE window as
/// complete (Pitfall 1) and stops paging early.
pub const MAX_ADVERTISED_LIMIT: usize = 5000;

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

/// Clamp an advertised `max_limit` to a usable `usize`, defaulting on absence or
/// a non-positive value, and upper-bounding any honest-but-absurd value at
/// [`MAX_ADVERTISED_LIMIT`] (WR-02, T-02-13).
///
/// Unlike [`clamp_limit`], the result is additionally capped at
/// [`MAX_ADVERTISED_LIMIT`] so a relay advertising e.g. `2_000_000_000` cannot
/// produce a pagination cap large enough to defeat count-vs-cap completeness
/// (Pitfall 1). Only `max_limit` carries this upper bound — `max_subscriptions`
/// and `max_filters` keep their existing default-on-non-positive behavior because
/// they gate the crawler's own request shaping, not relay completeness.
fn clamp_max_limit(advertised: Option<i32>) -> usize {
    clamp_limit(advertised, DEFAULT_MAX_LIMIT).min(MAX_ADVERTISED_LIMIT)
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
            max_limit: clamp_max_limit(lim.max_limit),
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

/// Enforce the [`MAX_NIP11_BYTES`] body bound, then parse the bytes into
/// [`RelayLimits`] (CR-06).
///
/// Pure: no I/O. This is the testable seam for the body bound — a slice larger
/// than [`MAX_NIP11_BYTES`] is rejected with [`RelayError::Nip11Fetch`] before
/// any UTF-8 decode or JSON parse, so an over-size body can be asserted to error
/// offline without standing up an HTTP server (T-02-19). A bounded body is
/// decoded as UTF-8 (lossily) and delegated to [`limits_from_json`].
pub fn limits_from_bytes(relay_url: &str, body: &[u8]) -> Result<RelayLimits, RelayError> {
    if body.len() > MAX_NIP11_BYTES {
        return Err(RelayError::Nip11Fetch {
            relay: relay_url.to_string(),
            reason: format!(
                "NIP-11 body of {} bytes exceeds MAX_NIP11_BYTES ({MAX_NIP11_BYTES})",
                body.len()
            ),
        });
    }
    let text = String::from_utf8_lossy(body);
    limits_from_json(&text)
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
    let resp = NIP11_CLIENT
        .get(&http_url)
        .header(reqwest::header::ACCEPT, "application/nostr+json")
        .send()
        .await
        .map_err(|e| RelayError::Nip11Fetch {
            relay: relay_url.to_string(),
            reason: format!("HTTP request failed: {e}"),
        })?;
    // Stream the body chunk-by-chunk and bail as soon as the accumulated length
    // exceeds MAX_NIP11_BYTES, so a hostile relay streaming an arbitrarily large
    // payload cannot be fully buffered into memory before rejection (T-02-19).
    let mut resp = resp;
    let mut body: Vec<u8> = Vec::new();
    loop {
        let chunk = resp.chunk().await.map_err(|e| RelayError::Nip11Fetch {
            relay: relay_url.to_string(),
            reason: format!("reading response body failed: {e}"),
        })?;
        match chunk {
            Some(bytes) => {
                if body.len() + bytes.len() > MAX_NIP11_BYTES {
                    return Err(RelayError::Nip11Fetch {
                        relay: relay_url.to_string(),
                        reason: format!(
                            "NIP-11 body exceeds MAX_NIP11_BYTES ({MAX_NIP11_BYTES})"
                        ),
                    });
                }
                body.extend_from_slice(&bytes);
            }
            None => break,
        }
    }
    limits_from_bytes(relay_url, &body).map_err(|_| RelayError::Nip11Fetch {
        relay: relay_url.to_string(),
        reason: "response was not a valid bounded NIP-11 document".to_string(),
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
