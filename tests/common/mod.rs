//! Shared integration-test fixtures.
//!
//! This is a shared `mod common;` included by every integration-test binary.
//! Each binary only uses the subset of fixtures it needs, so unused-fixture
//! dead-code warnings are expected and intentionally suppressed here (the
//! ingest fixtures are consumed by plan 02-02's tests).
//!
//! Two fixture families:
//! - [`start_postgres`] — an ephemeral Postgres instance (testcontainers +
//!   Docker) for store integration tests.
//! - nostr event fixtures ([`keys`], [`signed_event`], [`forged_event`],
//!   [`same_created_at_pair`], [`future_dated_event`]) — deterministic,
//!   offline (no network, no Postgres) `Event`s every ingest test in plan 02
//!   reuses to exercise the validation gate.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;

use nostr_sdk::nips::nip65::RelayMetadata;
use nostr_sdk::{Event, EventBuilder, Keys, Kind, PublicKey, RelayUrl, SecretKey, Tag, Timestamp};
use testcontainers::runners::AsyncRunner;
use testcontainers::ContainerAsync;
use testcontainers_modules::postgres::Postgres;
use web_of_trust::crawl::frontier::ClaimedAuthor;
use web_of_trust::error::RelayError;
use web_of_trust::store;

/// Start an ephemeral Postgres container and return its handle plus a connection URL.
///
/// The caller MUST keep the returned [`ContainerAsync`] alive for the duration of
/// the test — dropping it stops and removes the container.
///
/// Requires a running Docker daemon.
pub async fn start_postgres() -> anyhow::Result<(ContainerAsync<Postgres>, String)> {
    let container = Postgres::default().start().await?;
    let port = container.get_host_port_ipv4(5432).await?;
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    Ok((container, url))
}

// ---------------------------------------------------------------------------
// nostr event fixtures (offline; no network, no Postgres)
//
// These mirror the deterministic `pk(seed)` idiom from `tests/edge_diff.rs`:
// every key derives from a single seed byte so tests are fully reproducible.
// Fixtures may `expect()` on construction (test-only) — production paths must
// never `unwrap()` on relay-shaped data.
// ---------------------------------------------------------------------------

/// Build deterministic nostr [`Keys`] from a single seed byte.
///
/// The 32-byte secret key is `[seed; 32]`; any non-zero seed yields a valid
/// secp256k1 secret key below the curve order, so callers should use seeds
/// `>= 1` (the analogue of `edge_diff::pk(seed)`).
pub fn keys(seed: u8) -> Keys {
    let secret = SecretKey::from_slice(&[seed; 32]).expect("non-zero seed is a valid secret key");
    Keys::new(secret)
}

/// Build a fully-signed [`Event`] of `kind` authored by `signer`, p-tagging
/// each pubkey in `p_tags`, dated at `created_at`.
///
/// The returned event's [`Event::verify`] succeeds (valid id + signature). Use
/// [`Kind::ContactList`] (kind 3) or [`Kind::RelayList`] (kind 10002).
pub fn signed_event(
    signer: &Keys,
    kind: Kind,
    created_at: Timestamp,
    p_tags: &[PublicKey],
) -> Event {
    let tags = p_tags.iter().map(|pk| Tag::public_key(*pk));
    EventBuilder::new(kind, "")
        .tags(tags)
        .custom_created_at(created_at)
        .sign_with_keys(signer)
        .expect("signing a fixture event must succeed")
}

/// Build an [`Event`] whose stored signature/id no longer matches its content,
/// so [`Event::verify`] returns `Err` (INGEST-01 forged-event fixture).
///
/// The event is signed validly, then its `content` is mutated in place: the
/// stored `id` was committed over the original content at sign time, so id
/// recomputation in `verify()` now fails (and the signature, which signs the
/// id, no longer matches either).
pub fn forged_event(signer: &Keys, kind: Kind, created_at: Timestamp) -> Event {
    let mut event = signed_event(signer, kind, created_at, &[]);
    event.content = "tampered-after-signing".to_string();
    event
}

/// Build a forged "id-squat" [`Event`] that carries `target`'s claimed `id`
/// while failing [`Event::verify`] (CR-01 / T-02-14 attack fixture).
///
/// A hostile relay can send a forged event that reuses a genuine event's id to
/// try to consume that id in the dedup seen-set before the honest copy arrives.
/// This fixture models exactly that: it builds a tampered event (content mutated
/// after signing, so its committed id and signature no longer match its
/// content), then overwrites the stored `id` to `target.id`. The result still
/// *claims* `target.id`, but `verify()` recomputes the id from the (tampered)
/// content and finds a mismatch, so the gate rejects it. If dedup ran before
/// verification, this forgery would poison `target.id` in the seen-set.
pub fn id_squat_forgery(signer: &Keys, kind: Kind, created_at: Timestamp, target: &Event) -> Event {
    let mut event = forged_event(signer, kind, created_at);
    event.id = target.id;
    event
}

/// Build two valid signed events with the SAME `created_at` but different ids
/// (for the lowest-id tie-break test, INGEST-03 / Pitfall 3).
///
/// The two events differ in their p-tag sets, which changes the committed id
/// while keeping `created_at` identical. Both `verify()`.
pub fn same_created_at_pair(signer: &Keys, created_at: Timestamp) -> (Event, Event) {
    let a = signed_event(signer, Kind::ContactList, created_at, &[keys(201).public_key()]);
    let b = signed_event(signer, Kind::ContactList, created_at, &[keys(202).public_key()]);
    assert_eq!(a.created_at, b.created_at, "tie-break fixture must share created_at");
    assert_ne!(a.id, b.id, "tie-break fixture must have distinct ids");
    (a, b)
}

/// Build a valid signed event dated far in the future (for the future-clamp
/// rejection test, INGEST-03 / Pitfall 2).
///
/// `seconds_ahead` is added to "now"; pass a value well beyond any sane clamp
/// (e.g. one year) so the resolver rejects it.
pub fn future_dated_event(signer: &Keys, kind: Kind, seconds_ahead: u64) -> Event {
    let future = Timestamp::from(Timestamp::now().as_secs() + seconds_ahead);
    signed_event(signer, kind, future, &[])
}

// ---------------------------------------------------------------------------
// DB + scripted-graph fixtures (shared by tests/frontier.rs, tests/staleness.rs,
// tests/daemon_loop.rs). Promoted here from tests/frontier.rs (plan 04-01) so the
// injected-`fetch_union` seam + the Postgres harness are reused by every
// crawl/daemon-loop test binary, never duplicated.
// ---------------------------------------------------------------------------

/// A deterministic, offline, `Send` scripted relay graph for the end-to-end
/// crawl-loop tests. Maps each author's pubkey to the signed kind-3 event that
/// "the relays" return for it. Unlike `mock_relay::ScriptedRelay` (which is
/// `Rc<RefCell>` / `!Send` and so cannot cross a `tokio::spawn`), this is
/// `Arc`-backed so the bounded worker loop can hold it across spawned workers.
///
/// [`ScriptedGraph::fetch_fn`] returns a closure of the exact shape `run_crawl`
/// (and the Phase 4 daemon loop) expects: given an owned claimed batch it produces
/// the raw `Vec<Event>` union for those authors (modeling D-08's cross-relay union
/// before the single ingest pass).
///
/// Phase 5 (RELAY-05/06) extends this with two capabilities the fallback +
/// health-routing plans need:
/// - **relay-URL-aware** placement: events can be scripted to appear only on a
///   specific relay url (e.g. "author absent on curated relay A, present on
///   write relay B"), via [`ScriptedGraph::with_relay`]. The original
///   author-keyed [`ScriptedGraph::union_for`]/[`ScriptedGraph::fetch_fn`]
///   behavior is preserved unchanged for callers that ignore relay url.
/// - **error injection**: a registered relay url returns
///   `Err(RelayError::FetchTimeout(url))` (or `Err(RelayError::RelayNotFound)`,
///   a `Client`-shaped failure) from the relay-aware fetch closure, exercising
///   the health/timeout capture sites, via [`ScriptedGraph::fail_relay`].
///
/// Still `Arc`-backed + `Send + Clone + Sync + 'static` — it crosses
/// `tokio::spawn` in the daemon loop.
/// (relay_url, author pubkey bytes) -> the events that relay returns for that
/// author. Keyed map backing [`ScriptedGraph`]'s relay-URL-aware placement.
type RelayPlacements = HashMap<(String, Vec<u8>), Vec<Event>>;

#[derive(Clone)]
pub struct ScriptedGraph {
    /// author pubkey bytes -> the signed event that author publishes on the
    /// curated union (relay-url-agnostic back-compat path).
    events: Arc<HashMap<Vec<u8>, Event>>,
    /// (relay_url, author pubkey bytes) -> the events that relay returns for
    /// that author. Models per-relay placement (RELAY-05 fallback tests).
    by_relay: Arc<RelayPlacements>,
    /// relay_url -> the error this relay injects instead of returning events
    /// (RELAY-06 health/timeout capture tests).
    failures: Arc<HashMap<String, RelayFailure>>,
}

/// The error a designated relay injects from the relay-aware fetch closure.
#[derive(Clone, Copy, Debug)]
pub enum RelayFailure {
    /// Inject `RelayError::FetchTimeout(url)` (the explicit per-fetch timeout).
    Timeout,
    /// Inject `RelayError::RelayNotFound(url)` (a connect-shaped failure that a
    /// caller maps to a connect/client failure for health scoring).
    NotFound,
}

impl ScriptedGraph {
    pub fn new(events: Vec<Event>) -> Self {
        let map = events
            .into_iter()
            .map(|e| (e.pubkey.to_bytes().to_vec(), e))
            .collect();
        Self {
            events: Arc::new(map),
            by_relay: Arc::new(HashMap::new()),
            failures: Arc::new(HashMap::new()),
        }
    }

    /// Build a relay-URL-aware graph: `placements` maps a relay url to the
    /// events that relay returns. An author present on relay B but absent from
    /// the curated `events` union models the RELAY-05 fallback scenario.
    pub fn with_relay(placements: Vec<(&str, Vec<Event>)>) -> Self {
        let mut by_relay: RelayPlacements = HashMap::new();
        for (url, events) in placements {
            for e in events {
                by_relay
                    .entry((url.to_string(), e.pubkey.to_bytes().to_vec()))
                    .or_default()
                    .push(e);
            }
        }
        Self {
            events: Arc::new(HashMap::new()),
            by_relay: Arc::new(by_relay),
            failures: Arc::new(HashMap::new()),
        }
    }

    /// Register a relay url that injects `failure` instead of returning events
    /// from [`ScriptedGraph::relay_fetch`]. Consumes + returns self so it chains
    /// after [`ScriptedGraph::with_relay`]/[`ScriptedGraph::new`].
    pub fn fail_relay(mut self, relay_url: &str, failure: RelayFailure) -> Self {
        let mut map = (*self.failures).clone();
        map.insert(relay_url.to_string(), failure);
        self.failures = Arc::new(map);
        self
    }

    /// Build the raw cross-relay union for a claimed batch: every scripted event
    /// whose author is in the batch (an author with no scripted event contributes
    /// nothing — modeling a `not_found`).
    pub fn union_for(&self, batch: &[ClaimedAuthor]) -> Vec<Event> {
        batch
            .iter()
            .filter_map(|c| self.events.get(&c.pubkey).cloned())
            .collect()
    }

    /// Relay-aware fetch: return the events `relay_url` holds for `batch`'s
    /// authors, or the injected error if `relay_url` is a registered failure.
    /// A relay that simply has no events for an author returns an empty union
    /// (modeling a miss-on-that-relay).
    pub fn relay_fetch(
        &self,
        relay_url: &str,
        batch: &[ClaimedAuthor],
    ) -> Result<Vec<Event>, RelayError> {
        if let Some(failure) = self.failures.get(relay_url) {
            return Err(match failure {
                RelayFailure::Timeout => RelayError::FetchTimeout(relay_url.to_string()),
                RelayFailure::NotFound => RelayError::RelayNotFound(relay_url.to_string()),
            });
        }
        let mut out = Vec::new();
        for c in batch {
            if let Some(events) = self.by_relay.get(&(relay_url.to_string(), c.pubkey.clone())) {
                out.extend(events.iter().cloned());
            }
        }
        Ok(out)
    }

    /// A `fetch_union` closure for `run_crawl` / the daemon loop (no instrumentation).
    pub fn fetch_fn(
        &self,
    ) -> impl Fn(Vec<ClaimedAuthor>) -> std::future::Ready<Result<Vec<Event>, RelayError>>
           + Clone
           + Send
           + Sync
           + 'static {
        let me = self.clone();
        move |batch: Vec<ClaimedAuthor>| std::future::ready(Ok(me.union_for(&batch)))
    }

    /// A relay-URL-keyed fetch closure for the fallback path: given a relay url
    /// and a claimed batch it returns that relay's scripted events (or the
    /// injected error). `Send + Clone + Sync + 'static` so it crosses spawns.
    pub fn relay_fetch_fn(
        &self,
    ) -> impl Fn(String, Vec<ClaimedAuthor>) -> std::future::Ready<Result<Vec<Event>, RelayError>>
           + Clone
           + Send
           + Sync
           + 'static {
        let me = self.clone();
        move |relay_url: String, batch: Vec<ClaimedAuthor>| {
            std::future::ready(me.relay_fetch(&relay_url, &batch))
        }
    }
}

/// Build a signed kind-3 event for `author` (seed) following each `followees`
/// seed, dated `created_at`. Mirrors [`signed_event`] but takes seeds so the BFS
/// graph reads declaratively in the tests.
pub fn follows_event(author_seed: u8, followees: &[u8], created_at: u64) -> Event {
    let author = keys(author_seed);
    let p_tags: Vec<PublicKey> = followees.iter().map(|&s| keys(s).public_key()).collect();
    signed_event(&author, Kind::ContactList, Timestamp::from_secs(created_at), &p_tags)
}

/// Build a signed kind:10002 (NIP-65 RelayList) event for `author_seed` whose
/// r-tags advertise each `(url, marker)` pair, dated `created_at`.
///
/// `marker` is one of `"read"`, `"write"`, `"both"` (or any empty string for a
/// bare r-tag); `"both"`/empty produce a bare r-tag with NO read/write token,
/// which NIP-65 (and `nip65::extract_relay_list`) treats as both read+write.
/// Tags are built via the canonical `Tag::relay_metadata` constructor (never
/// hand-assembled), so a round-trip through `ingest::relay_list::extract_relay_pairs`
/// yields the same pairs (with `both`/empty normalizing back to `"both"`).
pub fn relay_list_event(author_seed: u8, relays: &[(&str, &str)], created_at: u64) -> Event {
    let author = keys(author_seed);
    let tags = relays.iter().map(|(url, marker)| {
        let relay_url = RelayUrl::parse(url).expect("fixture relay url must parse");
        let metadata = match *marker {
            "read" => Some(RelayMetadata::Read),
            "write" => Some(RelayMetadata::Write),
            // "both" or "" -> bare r-tag (no marker token) = read+write.
            _ => None,
        };
        Tag::relay_metadata(relay_url, metadata)
    });
    EventBuilder::new(Kind::RelayList, "")
        .tags(tags)
        .custom_created_at(Timestamp::from_secs(created_at))
        .sign_with_keys(&author)
        .expect("signing a relay-list fixture event must succeed")
}

/// Deterministic 32-byte pubkey from a single seed (mirrors concurrency::pk).
pub fn pk(seed: u16) -> [u8; 32] {
    let mut k = [0u8; 32];
    k[0] = (seed & 0xff) as u8;
    k[1] = (seed >> 8) as u8;
    k
}

/// Connect + migrate a fresh testcontainers Postgres, returning the live pool.
/// The container handle is returned alongside so the caller keeps it alive.
pub async fn fresh_db() -> anyhow::Result<(ContainerAsync<Postgres>, sqlx::PgPool)> {
    let (pg, url) = start_postgres().await?;
    let pool = store::connect(&url).await?;
    store::run_migrations(&pool).await?;
    Ok((pg, pool))
}

/// Read a pubkey's current status string.
pub async fn status_of(pool: &sqlx::PgPool, id: i64) -> anyhow::Result<String> {
    let s = sqlx::query_scalar::<_, String>("SELECT status FROM pubkeys WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await?;
    Ok(s)
}
