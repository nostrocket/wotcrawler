---
phase: 01-schema-data-contract
plan: 01
subsystem: project-scaffold
tags: [rust, cargo, sqlx, testcontainers, toolchain]
requires: []
provides:
  - "Rust crate skeleton (web-of-trust) compiling on toolchain 1.94.0"
  - "Locked dependency set (sqlx 0.9, tokio, thiserror, anyhow, config)"
  - "StoreError thiserror enum at src/error.rs"
  - "start_postgres() testcontainers bootstrap fixture for all later integration tests"
  - "src/store/mod.rs placeholder module (Plan 03 fills it)"
affects:
  - "Plan 02 (migrations) and Plan 03 (store layer) build on this crate + fixture"
tech-stack:
  added:
    - "Rust toolchain 1.94.0 (rustc 1.94.0 / cargo 1.94.0) via rustup"
    - "sqlx 0.9.0 (runtime-tokio, tls-rustls, postgres, macros, migrate, chrono)"
    - "tokio 1.52 (rt-multi-thread, macros)"
    - "thiserror 2.0.18, anyhow 1.0.102, config 0.15.23"
    - "testcontainers 0.27.3, testcontainers-modules 0.15.0 (postgres) [dev]"
  patterns:
    - "Raw-SQL-as-contract (sqlx, no ORM)"
    - "TEXT+CHECK status (planned for Plan 02 schema)"
    - "thiserror typed error at the store boundary"
key-files:
  created:
    - rust-toolchain.toml
    - Cargo.toml
    - Cargo.lock
    - .gitignore
    - src/lib.rs
    - src/store/mod.rs
    - src/error.rs
    - tests/common/mod.rs
    - tests/bootstrap.rs
  modified: []
decisions:
  - "Pinned toolchain to 1.94.0 (sqlx 0.9 MSRV), superseding CLAUDE.md's 1.84+ per RESEARCH correction"
  - "Committed Cargo.lock for reproducible builds of the locked dependency set"
  - "rust-docs component skipped (network timeout); minimal profile used — rustc/cargo unaffected"
metrics:
  duration: ~2h (dominated by first-build crate downloads + compile, ~21m, and test build/image pull)
  completed: 2026-06-12
---

# Phase 01 Plan 01: Project Scaffold Summary

Greenfield Rust crate `web-of-trust` now compiles on a pinned 1.94.0 toolchain with the locked dependency set (sqlx 0.9, tokio, thiserror, anyhow, config), exposes a `StoreError` typed error, and ships a reusable testcontainers Postgres bootstrap fixture proven green against real Postgres via Docker.

## Resolved Toolchain

- **rustc:** 1.94.0 (4a4ef493e 2026-03-02)
- **cargo:** 1.94.0 (85eff7c80 2026-01-15)
- Installed via the operator-approved rustup flow; pinned in `rust-toolchain.toml` (`channel = "1.94.0"`).

## Tasks Completed

| Task | Name | Commit | Files |
| ---- | ---- | ------ | ----- |
| 1 | Confirm/install Rust toolchain >= 1.94 | (no repo change — system install) | — |
| 2 | Cargo manifest, toolchain pin, crate skeleton | f101a9b | rust-toolchain.toml, Cargo.toml, Cargo.lock, .gitignore, src/lib.rs, src/store/mod.rs, src/error.rs |
| 3 | testcontainers Postgres bootstrap fixture (TDD) | 7ba8e66 | tests/common/mod.rs, tests/bootstrap.rs |

## Verification

- `cargo build` — exit 0 (`Finished dev profile` in 21m18s, first build with crate downloads).
- `cargo test --test bootstrap` — `1 passed; 0 failed` (started ephemeral Postgres via Docker, connected with sqlx, `SELECT 1` == 1).
- `Cargo.toml` contains `rust-version = "1.94"` and the sqlx feature list `["runtime-tokio", "tls-rustls", "postgres", "macros", "migrate", "chrono"]`.
- `Cargo.toml` contains no `diesel`, no `sea-orm`, no sqlx `uuid` feature.
- `.gitignore` contains `/target` and does NOT contain `.sqlx`.
- `src/error.rs` defines `StoreError` with `#[from] sqlx::Error` and `#[from] sqlx::migrate::MigrateError`.

## Bootstrap Helper Signature (for Plans 02 and 03)

```rust
// tests/common/mod.rs
pub async fn start_postgres()
    -> anyhow::Result<(testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>, String)>;
// Returns (container_handle, "postgres://postgres:postgres@127.0.0.1:{port}/postgres").
// Caller MUST keep the container handle alive for the test duration.
```

Later integration tests should `mod common;` and call `common::start_postgres()`, then connect with `sqlx::PgPool::connect(&url)` and run migrations against it.

## Final Dependency Versions (locked)

| Crate | Version | Notes |
| ----- | ------- | ----- |
| sqlx | 0.9.0 | default-features off; runtime-tokio, tls-rustls, postgres, macros, migrate, chrono |
| tokio | 1.52 | rt-multi-thread, macros |
| thiserror | 2.0.18 | |
| anyhow | 1.0.102 | |
| config | 0.15.23 | |
| testcontainers | 0.27.3 | dev-dependency |
| testcontainers-modules | 0.15.0 | dev-dependency; postgres feature |

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Transient crates.io network timeouts during first `cargo build`**
- **Found during:** Task 2
- **Issue:** Initial `cargo build` failed downloading `unicode-normalization` (transitive via `stringprep` ← `sqlx-postgres`) with repeated "operation timed out" errors. The package exists and is legitimate (audited OK in RESEARCH) — this was network flakiness, not a missing/slopsquatted package, so it did not warrant a package-legitimacy checkpoint.
- **Fix:** Re-ran `cargo build` with `CARGO_NET_RETRY=10` and `CARGO_HTTP_TIMEOUT=120`; cargo resumed the partial downloads and the build finished with exit 0.
- **Files modified:** none (environment-only)
- **Commit:** n/a

**2. [Rule 3 - Blocking] rust-docs component download timed out during toolchain install**
- **Found during:** Task 1
- **Issue:** `rustup toolchain install 1.94.0` timed out fetching the `rust-docs` component.
- **Fix:** rustc and cargo for 1.94.0 were already installed and functional; re-ran the install with `--profile minimal` to drop the non-essential docs component. Build/test toolchain unaffected.
- **Files modified:** none (environment-only)
- **Commit:** n/a

## Checkpoint / Auth Gates

- **Task 1 (checkpoint:decision, gate=blocking):** No Rust toolchain on PATH. Operator approved Option A (install via rustup). Resolved by this continuation run — toolchain 1.94.0 installed and pinned. No further pauses.

## Known Stubs

- `src/store/mod.rs` is an intentional empty placeholder module (doc comment only). It exists so the crate compiles and is filled by Plan 03 (PgPool wiring, run_migrations, edge-diff writer). Documented in the plan (`<action>` Task 2) — not a blocking stub.

## Self-Check: PASSED

- Files: all 9 created files FOUND on disk.
- Commits: f101a9b FOUND, 7ba8e66 FOUND.
