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

use nostr_sdk::{Event, EventBuilder, Keys, Kind, PublicKey, SecretKey, Tag, Timestamp};
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
#[derive(Clone)]
pub struct ScriptedGraph {
    /// author pubkey bytes -> the signed kind-3 event that author publishes.
    events: Arc<HashMap<Vec<u8>, Event>>,
}

impl ScriptedGraph {
    pub fn new(events: Vec<Event>) -> Self {
        let map = events
            .into_iter()
            .map(|e| (e.pubkey.to_bytes().to_vec(), e))
            .collect();
        Self {
            events: Arc::new(map),
        }
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
}

/// Build a signed kind-3 event for `author` (seed) following each `followees`
/// seed, dated `created_at`. Mirrors [`signed_event`] but takes seeds so the BFS
/// graph reads declaratively in the tests.
pub fn follows_event(author_seed: u8, followees: &[u8], created_at: u64) -> Event {
    let author = keys(author_seed);
    let p_tags: Vec<PublicKey> = followees.iter().map(|&s| keys(s).public_key()).collect();
    signed_event(&author, Kind::ContactList, Timestamp::from_secs(created_at), &p_tags)
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
