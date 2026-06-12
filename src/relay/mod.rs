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
//! plan 02-01; plans 02-03 / 02-04 fill them.

pub mod fetch;
pub mod nip11;
pub mod rate_limit;
