//! `crawler` daemon binary entry point (OPS-01).
//!
//! Placeholder for the Phase 4 daemon bootstrap. The real entry — config load,
//! tracing init, metrics-recorder install, axum metrics/health server, PgPool +
//! relay-client construction, and the signal-driven daemon loop under a
//! `CancellationToken` — lands in plan 04-05. This stub exists now so the
//! `[[bin]] crawler` target compiles while the wiring is filled in by later
//! Phase 4 plans.
fn main() {
    // Real bootstrap wired in 04-05; intentionally a no-op for now so the
    // binary target builds against the new dependency surface.
}
