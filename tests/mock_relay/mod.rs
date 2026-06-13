//! In-process mock-relay harness for the pagination test (RELAY-03).
//!
//! A full nostr-sdk 0.44 websocket relay mock is impractical to stand up
//! offline, so — per the plan's documented alternative — this fixture drives the
//! pagination loop's page-back decision through an **injected async fetch
//! function** that returns scripted windows. The harness mirrors the
//! reusable-setup-returning-a-handle pattern from `common/mod.rs`: the caller
//! builds a [`ScriptedRelay`] and hands its [`ScriptedRelay::fetch_fn`] to
//! [`web_of_trust::relay::fetch::paginate_chunk`].
//!
//! The scripted relay returns a CAPPED window (exactly `cap` events) on the
//! first call and FEWER than the cap on the next, each honoring the filter's
//! `until` so the page-back arithmetic (`until = oldest - 1`) is exercised. EOSE
//! is implicit (each `fetch_events` returns and "closes"); the test proves the
//! loop pages again on a capped window despite that close.

#![allow(dead_code)]

use std::cell::RefCell;
use std::rc::Rc;

use nostr_sdk::{Event, EventBuilder, Filter, Keys, Kind, SecretKey, Timestamp};
use web_of_trust::error::RelayError;

/// Build deterministic [`Keys`] from a seed byte (mirrors common::keys).
fn keys(seed: u8) -> Keys {
    let secret = SecretKey::from_slice(&[seed; 32]).expect("non-zero seed is valid");
    Keys::new(secret)
}

/// A signed kind-3 event authored by seed `signer_seed`, dated `created_at`.
pub fn event_at(signer_seed: u8, created_at: u64) -> Event {
    let signer = keys(signer_seed);
    EventBuilder::new(Kind::ContactList, "")
        .custom_created_at(Timestamp::from_secs(created_at))
        .sign_with_keys(&signer)
        .expect("signing a mock event must succeed")
}

/// One scripted window: the events the relay returns for the Nth REQ.
pub type Window = Vec<Event>;

/// A scripted "relay" that returns pre-built windows in order, recording the
/// `until` of each filter it received so a test can assert the loop paged back.
pub struct ScriptedRelay {
    windows: Rc<RefCell<std::collections::VecDeque<Window>>>,
    untils: Rc<RefCell<Vec<Option<Timestamp>>>>,
    limits: Rc<RefCell<Vec<Option<usize>>>>,
}

impl ScriptedRelay {
    /// Build a relay that returns `windows` in order. After the scripted windows
    /// are exhausted it returns an empty window (genuine completion).
    pub fn new(windows: Vec<Window>) -> Self {
        Self {
            windows: Rc::new(RefCell::new(windows.into_iter().collect())),
            untils: Rc::new(RefCell::new(Vec::new())),
            limits: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// The `until` timestamp the relay saw on each REQ, in order. The test
    /// asserts the second REQ's `until` is strictly older than the first, i.e.
    /// the loop paged back rather than stopping on the capped first window.
    pub fn untils(&self) -> Vec<Option<Timestamp>> {
        self.untils.borrow().clone()
    }

    /// The filter `limit` the relay saw on each REQ, in order. Used by the
    /// production-wiring test to assert the per-window cap is the cached
    /// NIP-11 `max_limit` (WR-03 / RELAY-02).
    pub fn limits_seen(&self) -> Vec<Option<usize>> {
        self.limits.borrow().clone()
    }

    /// Produce the injectable async fetch fn for `paginate_chunk`. Each call pops
    /// the next scripted window (empty once exhausted) and records the filter's
    /// `until`.
    pub fn fetch_fn(
        &self,
    ) -> impl FnMut(Filter) -> std::future::Ready<Result<Vec<Event>, RelayError>> + '_ {
        let windows = Rc::clone(&self.windows);
        let untils = Rc::clone(&self.untils);
        move |filter: Filter| {
            untils.borrow_mut().push(filter.until);
            let next = windows.borrow_mut().pop_front().unwrap_or_default();
            std::future::ready(Ok(next))
        }
    }

    /// Like [`Self::fetch_fn`] but also records the filter's `limit` on each REQ
    /// so a test can assert the per-window cap sourced from the NIP-11 cache.
    pub fn limit_capturing_fetch_fn(
        &self,
    ) -> impl FnMut(Filter) -> std::future::Ready<Result<Vec<Event>, RelayError>> + '_ {
        let windows = Rc::clone(&self.windows);
        let untils = Rc::clone(&self.untils);
        let limits = Rc::clone(&self.limits);
        move |filter: Filter| {
            untils.borrow_mut().push(filter.until);
            limits.borrow_mut().push(filter.limit);
            let next = windows.borrow_mut().pop_front().unwrap_or_default();
            std::future::ready(Ok(next))
        }
    }
}

/// Records the `until` each REQ carried, shared with the fetch fn returned by
/// [`prefix_for_until_fetch_fn`] so a test can assert the loop pinned `until`
/// across iterations.
pub type UntilLog = Rc<RefCell<Vec<Option<Timestamp>>>>;

/// A *deterministic newest-first relay* fetch fn for the CR-03 residual
/// (02-VERIFICATION.md gap #1): unlike [`ScriptedRelay`], which hands each REQ
/// the next pop-front window (and so can be hand-fed the cut sibling), this
/// models a real relay that, for **any** filter, returns the cap-sized
/// newest-first prefix of a fixed event pool clamped to `filter.until`.
///
/// Because the pool is fixed and the prefix is recomputed per call, a pinned
/// `until=T` always yields the **same** cap-sized prefix and NEVER volunteers a
/// third sibling sharing the boundary second `T` — exactly the deterministic
/// behavior that silently truncates a follow list at the cap boundary. Genuine
/// page-back to an older `until` advances into older events as a real relay
/// would.
///
/// `pool` is sorted newest-first internally; `cap` is the per-window cap. The
/// returned closure pushes each `filter.until` into `untils` (the same recording
/// pattern as [`ScriptedRelay::untils`]) so a test can assert `until` stayed
/// pinned at the boundary second across iterations.
pub fn prefix_for_until_fetch_fn(
    pool: Vec<Event>,
    cap: usize,
    untils: UntilLog,
) -> impl FnMut(Filter) -> std::future::Ready<Result<Vec<Event>, RelayError>> {
    // Sort newest-first once; the relay always serves from this fixed view.
    let mut sorted = pool;
    sorted.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    move |filter: Filter| {
        untils.borrow_mut().push(filter.until);
        // Clamp to `until` (inclusive: created_at <= until), then take the
        // newest `cap`. An absent `until` means "no upper bound" (window 1).
        let window: Vec<Event> = sorted
            .iter()
            .filter(|e| match filter.until {
                Some(until) => e.created_at <= until,
                None => true,
            })
            .take(cap)
            .cloned()
            .collect();
        std::future::ready(Ok(window))
    }
}
