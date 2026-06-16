# Phase 2: Relay Acquisition & Validation - Pattern Map

**Mapped:** 2026-06-12
**Files analyzed:** 18 (10 source + 6 test + 2 cross-cutting modified)
**Analogs found:** 12 / 18 (codebase analogs); 6 net-new (relay/transport — no Phase 1 analog, use RESEARCH patterns)

Phase 1 is a DB-only write layer. It provides strong analogs for **module structure, typed thiserror errors, in-Rust dedup/diff logic, boundary input-validation guards, and Rust test conventions** — these map directly onto the `ingest/` validation module and all of `tests/`. It provides **no analog for relay transport** (no nostr-sdk usage exists yet): the `relay/` module files must be built from RESEARCH.md Code Examples + nostr-sdk docs.

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match Quality |
|-------------------|------|-----------|----------------|---------------|
| `src/lib.rs` (modify) | config | — | `src/lib.rs` (self) | exact (extend the existing `pub mod` block) |
| `src/error.rs` (modify) | model | — | `src/error.rs` (self, `StoreError`) | exact (add `RelayError`/`IngestError` same shape) |
| `src/relay/mod.rs` | service | request-response | — | no analog (nostr-sdk Client wiring; net-new) |
| `src/relay/nip11.rs` | service | request-response | — | no analog (HTTP/SDK doc fetch + cache; net-new) |
| `src/relay/rate_limit.rs` | utility | event-driven | — | no analog (governor; net-new) |
| `src/relay/fetch.rs` | service | streaming/batch | — | partial (chunk-and-loop shape ~ `follows.rs` diff loop; transport net-new) |
| `src/ingest/mod.rs` | service | transform | `src/store/mod.rs` | role-match (module orchestrator + doc header) |
| `src/ingest/verify.rs` | service | transform | `src/store/pubkeys.rs` (boundary guard) | role-match (validate-and-reject-at-boundary) |
| `src/ingest/replaceable.rs` | utility | transform | `src/store/follows.rs` (in-Rust set logic) | role-match (pure in-Rust resolution, no I/O) |
| `src/ingest/follow_list.rs` | utility | transform | `src/store/follows.rs` lines 73-91 (HashSet dedup + self-drop) | exact (same dedup/self-drop idiom) |
| `tests/verify_gate.rs` | test | — | `tests/edge_diff.rs` | role-match (unit-style; no DB) |
| `tests/dedup.rs` | test | — | `tests/edge_diff.rs` | role-match |
| `tests/replaceable.rs` | test | — | `tests/edge_diff.rs` | role-match (pure-logic, no `start_postgres`) |
| `tests/follow_list_bounds.rs` | test | — | `tests/edge_diff.rs` | role-match |
| `tests/relay_list.rs` | test | — | `tests/edge_diff.rs` | role-match |
| `tests/pagination.rs` | test | — | `tests/common/mod.rs` (fixture pattern) | partial (needs mock relay, not Postgres) |
| `tests/common/mod.rs` (modify) | test | — | `tests/common/mod.rs` (self) | exact (add event/key/forged-event fixtures) |
| `Cargo.toml` (modify) | config | — | `Cargo.toml` (self) | exact (add nostr-sdk/governor/metrics deps) |

## Pattern Assignments

### `src/lib.rs` (modify — module registration)

**Analog:** `src/lib.rs` (self, lines 1-9)

The crate root is a flat `pub mod` list with a doc header and a re-export. Extend it the same way — do not restructure.

```rust
//! web-of-trust: nostr follow-graph crawler & data layer.
pub mod error;
pub mod store;
pub use error::StoreError;
```

**Apply:** add `pub mod relay;` and `pub mod ingest;` after `pub mod store;`. If new error types are added (see below), extend the re-export line (`pub use error::{StoreError, RelayError, IngestError};`).

---

### `src/error.rs` (modify — typed errors)

**Analog:** `src/error.rs` (self, `StoreError`, lines 1-20)

Phase 1's error convention is the load-bearing pattern for all new error types: one `#[derive(Debug, Error)]` enum per crate boundary, `#[error(transparent)]` + `#[from]` for wrapped library errors, and a typed variant per domain-specific failure carrying the offending value.

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),

    #[error("migration failed: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("invalid pubkey length: expected 32 bytes, got {0}")]
    InvalidPubkey(usize),
}
```

**Apply to** `RelayError` (wrap nostr-sdk client/pool errors with `#[from]`; typed variants for relay-not-found / NIP-11 fetch failure / timeout) and `IngestError` (typed variants for `InvalidSignature`, `UnsolicitedEvent { wanted: Kind, got: Kind }`, `FutureDated`, `OversizedFollowList(usize)` — mirror the `InvalidPubkey(usize)` shape that carries the bad value). Keep the per-variant `#[error("...")]` message convention. Note: validation *rejections* in `ingest/verify.rs` are expected-and-counted (return `false` + a `metrics::counter!`), NOT errors — see verify.rs below; reserve `IngestError` for true failures.

---

### `src/ingest/follow_list.rs` (utility, transform) — STRONGEST ANALOG

**Analog:** `src/store/follows.rs` lines 73-91 (the in-Rust dedup + self-drop + set-difference block)

This is an exact idiom match. The store already does HashSet-based dedup and self-drop in Rust; the ingest p-tag extractor does the same, just sourced from `Tags::public_keys()` instead of a slice. Copy the filtering style.

**Dedup + self-drop pattern** (`follows.rs` lines 73-78):
```rust
let new_set: HashSet<i64> = followee_ids
    .iter()
    .copied()
    .filter(|&id| id != follower_id)   // drop self-follow (D-08)
    .collect();
```

**Apply:** combine the above with RESEARCH Code Example "p-tag extraction" (lines 401-417). The new function operates on `PublicKey` (from `Tags::public_keys()`) before id-resolution:
```rust
let mut seen = HashSet::new();
let out: Vec<PublicKey> = event.tags.public_keys().copied()
    .filter(|pk| *pk != event.pubkey)   // self-drop (D-08, defense in depth)
    .filter(|pk| seen.insert(*pk))       // dedup
    .collect();
// then apply configurable follow_cap (reject vs truncate — Open Question 4)
```
Note D-06: relay hints + petnames are discarded — only the pubkey set crosses the boundary, matching the store's id-only signature.

---

### `src/ingest/replaceable.rs` (utility, transform)

**Analog:** `src/store/follows.rs` (pure in-Rust set/diff logic, no I/O in the function body that isn't a query)

`replaceable.rs` is pure logic (clamp + `max_by` + tie-break), so it follows the "compute in Rust, keep it testable without a DB" spirit of `follows.rs`'s diff computation (lines 80-91). No DB analog for the comparison itself — use RESEARCH Code Example "Replaceable-event resolution" (lines 381-398) verbatim as the core:

```rust
events
    .filter(|e| e.created_at.as_u64() <= max_ok)   // reject future-dated (Pitfall 2)
    .max_by(|a, b| {
        a.created_at.cmp(&b.created_at)              // newest by created_at
            .then_with(|| b.id.cmp(&a.id))           // tie -> LOWEST id wins (Pitfall 3)
    })
```

**Cross-link:** the winner's `event_id` + `created_at` must feed `store::follows::apply_follow_list(pool, follower_id, event_id, created_at, ...)` (follows.rs line 40-46) — the store short-circuits on an unchanged `applied_event_id`, so the resolver MUST emit the true winning id. Apply the identical resolver to `Kind::RelayList` (10002) for INGEST-05.

---

### `src/ingest/verify.rs` (service, transform — boundary guard)

**Analog:** `src/store/pubkeys.rs` lines 30-33 (reject-at-boundary input validation)

Phase 1's boundary-guard idiom: validate untrusted input at the entry point and reject before doing any work. `verify.rs` is the project's primary adversarial-input gate and applies the same "check first, then proceed" discipline.

**Boundary-guard pattern** (`pubkeys.rs` lines 30-33):
```rust
pub async fn upsert_pubkey(pool: &PgPool, pubkey: &[u8]) -> Result<i64, StoreError> {
    if pubkey.len() != PUBKEY_LEN {
        return Err(StoreError::InvalidPubkey(pubkey.len()));   // reject malformed at boundary
    }
    // ... only proceed on valid input
```

**Apply:** combine with RESEARCH Code Example "Signature-verification gate" (lines 365-379). Verify-first, then kind/author match, count rejects via `metrics::counter!`:
```rust
fn accept(event: &Event, want_kind: Kind, requested: &HashSet<PublicKey>) -> bool {
    if event.verify().is_err() { metrics::counter!("ingest_invalid_signature").increment(1); return false; }
    if event.kind != want_kind || !requested.contains(&event.pubkey) {
        metrics::counter!("ingest_unsolicited").increment(1); return false;
    }
    true
}
```
Never `unwrap()` on relay-supplied fields (V5/V7 — no panics from adversarial input), consistent with the store's defensive style.

---

### `src/ingest/mod.rs` (service, transform — orchestrator)

**Analog:** `src/store/mod.rs` lines 1-18 (module orchestrator: doc header stating responsibility split + `pub mod` submodule list + shared imports)

`store/mod.rs` is the template for a module root: a `//!` doc header that names what is delegated vs. custom, a `pub mod` block for submodules, and the module's wiring functions. Mirror this exactly.

```rust
//! Store layer: PgPool wiring ... the only custom logic is the
//! edge-diff computation in [`follows::apply_follow_list`].
pub mod follows;
pub mod pubkeys;

use crate::error::StoreError;
```

**Apply:** `ingest/mod.rs` doc header states the delegation split (nostr-sdk owns verify/parse; the module owns the gate orchestration: verify -> kind/author match -> dedup seen-set -> resolve -> extract). Declare `pub mod verify; pub mod replaceable; pub mod follow_list;`. The orchestrator runs the pipeline and emits the `ValidatedFollowList { follower_pubkey, event_id, created_at, followee_pubkeys }` value (the phase output contract). The cross-call/stream dedup seen-set (`HashSet<EventId>`, INGEST-02) lives here — same `HashSet` idiom as `follows.rs` line 74.

---

### `src/relay/mod.rs`, `src/relay/nip11.rs`, `src/relay/rate_limit.rs`, `src/relay/fetch.rs` (net-new transport)

**No codebase analog.** Phase 1 contains zero nostr-sdk/governor usage. Use RESEARCH.md Code Examples + the recommended structure. What carries over from Phase 1:
- **Module shape** — follow `store/mod.rs`'s doc-header + `pub mod` convention for `relay/mod.rs`.
- **Error convention** — surface failures through a `RelayError` enum shaped like `StoreError` (see error.rs above).
- **`fetch.rs` loop shape** — the author-chunk + page-back loop (RESEARCH lines 328-363) is structurally similar to the iterate-a-set-in-Rust style of `follows.rs`, but the transport (`Client::fetch_events`) is net-new.

RESEARCH references to copy from:
- `relay/mod.rs`: "Connect curated relay set with reconnect policy" (RESEARCH lines 313-326).
- `relay/fetch.rs`: "Paginated author-chunked fetch that never trusts EOSE" (RESEARCH lines 328-363) — count-vs-`max_limit` page-back is mandatory (Pitfall 1).
- `relay/rate_limit.rs`: governor GCRA `RateLimiter::direct(Quota::...)` + `.until_ready().await`; notice-driven backoff (RESEARCH Pattern 3, lines 195-201).
- `relay/nip11.rs`: `RelayInformationDocument` limit cache (RESEARCH RELAY-02 + Open Question 2 — confirm SDK accessor vs. `reqwest` fallback at plan time).

---

### Test files (`tests/*.rs`)

**Analog:** `tests/edge_diff.rs` (unit-style assertions, deterministic fixtures) and `tests/common/mod.rs` (shared fixture).

**Pure-logic tests** (`verify_gate.rs`, `dedup.rs`, `replaceable.rs`, `follow_list_bounds.rs`, `relay_list.rs`) test the `ingest/` module and need **no Postgres** — they do NOT call `common::start_postgres`. They follow the `edge_diff.rs` structure minus the DB setup:

**Deterministic fixture helper** (`edge_diff.rs` lines 13-15):
```rust
/// Deterministic 32-byte pubkey from a single seed byte.
fn pk(seed: u8) -> [u8; 32] { [seed; 32] }
```

**Test signature + assert style** (`edge_diff.rs` lines 31-53):
```rust
#[tokio::test]
async fn upsert_pubkey_is_idempotent() -> anyhow::Result<()> {
    // ... arrange
    assert_eq!(id1, id2, "same pubkey must return the same surrogate id");
    assert!(matches!(bad, Err(web_of_trust::StoreError::InvalidPubkey(16))));
    Ok(())
}
```

**Apply:** `#[tokio::test]` (or plain `#[test]` for sync pure logic), `-> anyhow::Result<()>`, descriptive assertion messages, `matches!` for typed-error/rejection assertions. Build signed `Event`s with known keys in-test (Wave 0 fixture gap) — add these helpers to `tests/common/mod.rs` alongside `start_postgres` (Keys/Event builders + a forged/invalid-sig event + same-`created_at` variants for tie-break).

**`tests/pagination.rs`** (RELAY-03) is the outlier: it needs a **mock/in-process relay** that returns capped result sets + EOSE (the hardest Wave 0 fixture), NOT Postgres. Mirror the `common/mod.rs` fixture *pattern* (a reusable async setup returning a handle the caller keeps alive) for the mock relay.

## Shared Patterns

### Typed errors (thiserror)
**Source:** `src/error.rs` lines 1-20
**Apply to:** `relay/*` (`RelayError`) and `ingest/*` (`IngestError`)
One enum per boundary; `#[error(transparent)] #[from]` for wrapped lib errors; a typed variant carrying the offending value for each domain failure (`InvalidPubkey(usize)` is the template).

### In-Rust HashSet dedup + self-drop
**Source:** `src/store/follows.rs` lines 73-91
**Apply to:** `ingest/follow_list.rs` (p-tag dedup + self-drop) and `ingest/mod.rs` (cross-call `HashSet<EventId>` seen-set, INGEST-02)
```rust
let set: HashSet<_> = items.iter().copied()
    .filter(|x| *x != self_value)   // self-drop
    .collect();                      // dedup
```

### Boundary input-validation (reject-first)
**Source:** `src/store/pubkeys.rs` lines 30-33
**Apply to:** `ingest/verify.rs` (the adversarial-input gate). Validate untrusted input at the entry point and reject before any work. For ingest, "reject" = return `false` + `metrics::counter!(...).increment(1)` (expected-and-counted), reserving `IngestError` for genuine failures. Never `unwrap()` on relay-supplied data (V5/V7).

### Module orchestrator convention
**Source:** `src/store/mod.rs` lines 1-18
**Apply to:** `ingest/mod.rs`, `relay/mod.rs`
`//!` header naming delegated-vs-custom responsibilities; `pub mod` submodule block; shared imports; wiring functions in the root.

### Test convention
**Source:** `tests/edge_diff.rs` + `tests/common/mod.rs`
**Apply to:** all `tests/*.rs`
`#[tokio::test] -> anyhow::Result<()>`; deterministic seed-based fixtures (`pk(seed)`); descriptive assert messages; `matches!` for typed rejections; reusable fixtures in `tests/common/mod.rs`. Pure-logic ingest tests skip `start_postgres`; only `pagination.rs` needs a mock-relay fixture.

## No Analog Found

Files with no close match in the codebase (planner should use RESEARCH.md Code Examples + nostr-sdk docs):

| File | Role | Data Flow | Reason |
|------|------|-----------|--------|
| `src/relay/mod.rs` | service | request-response | No nostr-sdk `Client` usage exists in Phase 1 (DB-only) |
| `src/relay/nip11.rs` | service | request-response | No HTTP/NIP-11 fetching exists; SDK accessor unconfirmed (Open Q2) |
| `src/relay/rate_limit.rs` | utility | event-driven | No `governor`/rate-limiting code exists yet |
| `src/relay/fetch.rs` | service | streaming/batch | Pagination loop is net-new; only the loop *shape* echoes `follows.rs` |
| `tests/pagination.rs` | test | — | Needs a mock relay fixture; Phase 1 only has a Postgres fixture |
| (`ValidatedFollowList` value type) | model | — | New output-contract struct; no Phase 1 analog (closest is the `apply_follow_list` arg list it must feed) |

## Metadata

**Analog search scope:** `src/` (lib.rs, error.rs, store/{mod,follows,pubkeys}.rs), `tests/` (edge_diff.rs, contract.rs, common/mod.rs), `Cargo.toml`
**Files scanned:** 8 source/test files (the full Phase 1 surface) + Cargo manifest
**Pattern extraction date:** 2026-06-12
