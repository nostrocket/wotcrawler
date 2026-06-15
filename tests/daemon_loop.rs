//! Daemon continuous-loop + graceful-shutdown verification (filled in 04-04).
//!
//! Wave 0 scaffold: named `#[ignore]` stubs that plan 04-04 fills once
//! `daemon::loop_::run_daemon_loop` exists. They reuse the promoted
//! [`common::ScriptedGraph`] mock + the injected-`fetch_union` seam and the
//! [`common::fresh_db`] Postgres harness, plus an injected `CancellationToken`
//! (never real signals).

mod common;

/// OPS-02: cancelling the loop drains in-flight workers and leaves zero
/// `in_progress` leases (no orphans).
#[tokio::test]
#[ignore = "filled in 04-04"]
async fn graceful_drain_no_orphan_leases() {
    unimplemented!("04-04: cancel -> drain -> zero in_progress leases");
}

/// The loop idles on an empty frontier and resumes once the staleness scanner
/// re-enqueues a stale row.
#[tokio::test]
#[ignore = "filled in 04-04"]
async fn idle_then_resume_after_reenqueue() {
    unimplemented!("04-04: idle-poll on empty frontier, resume on re-enqueue");
}
