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
}

impl ScriptedRelay {
    /// Build a relay that returns `windows` in order. After the scripted windows
    /// are exhausted it returns an empty window (genuine completion).
    pub fn new(windows: Vec<Window>) -> Self {
        Self {
            windows: Rc::new(RefCell::new(windows.into_iter().collect())),
            untils: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// The `until` timestamp the relay saw on each REQ, in order. The test
    /// asserts the second REQ's `until` is strictly older than the first, i.e.
    /// the loop paged back rather than stopping on the capped first window.
    pub fn untils(&self) -> Vec<Option<Timestamp>> {
        self.untils.borrow().clone()
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
}
