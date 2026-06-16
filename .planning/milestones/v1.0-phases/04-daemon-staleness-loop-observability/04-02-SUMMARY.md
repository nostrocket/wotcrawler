---
phase: 04-daemon-staleness-loop-observability
plan: 02
subsystem: daemon-config
tags: [daemon, config, toml, env-overlay, validation, ops-01, serde, humantime]
requires:
  - "daemon module root (04-01)"
  - "crawl::DEFAULT_BATCH_SIZE / DEFAULT_CONCURRENCY / DEFAULT_MAX_ATTEMPTS (Phase 3)"
  - "relay::rate_limit::DEFAULT_REQS_PER_SECOND, relay::fetch::DEFAULT_FETCH_TIMEOUT (Phase 2)"
  - "config crate (TOML + Environment sources), nostr_sdk::PublicKey::parse"
provides:
  - "daemon::config::Config (full daemon tunable set, single source of truth)"
  - "daemon::config::LogFormat { Human, Json }"
  - "daemon::config::load_config(path) -> Result<Config> (TOML File + WOT__ env overlay)"
  - "daemon::config::validate(&Config) -> Result<()> (fail-fast anchor/relays/db_url/ttl)"
  - "config.example.toml (committed operator template, every field documented)"
  - "redacted Debug for Config (database_url never logged)"
affects:
  - "04-03 (observe: log_level/log_format/metrics_addr), 04-04 (loop: concurrency/batch_size/ttl/intervals), 04-05 (main: --config bootstrap, validate-then-run)"
tech-stack:
  added:
    - "serde 1.0.228 (derive) — promoted to a direct dep (was transitive via nostr-sdk)"
  patterns:
    - "config::Config::builder().add_source(File).add_source(Environment.prefix(WOT).separator(__)).try_deserialize()"
    - "#[serde(default = \"default_*\")] fns return the existing DEFAULT_* consts by name — never re-literal"
    - "#[serde(with = \"humantime_serde\")] on every Duration config field"
    - "hand-written Debug redacting a secret field (database_url -> <redacted>)"
    - "pure-unit config tests: unique tempfiles under temp_dir, --test-threads=1 for env mutation"
key-files:
  created:
    - "src/daemon/config.rs"
    - "config.example.toml"
  modified:
    - "src/daemon/mod.rs"
    - "tests/daemon_config.rs"
    - "Cargo.toml"
    - "Cargo.lock"
decisions:
  - "serde promoted to a direct dependency (CLAUDE.md stack table lists it for config); pinned to 1.0.228 already in the lock tree — not a new package, no version bump"
  - "PublicKey::parse (nostr 0.44.3) accepts hex AND bech32 — verified in registry source; no from_hex/from_bech32 fallback needed"
  - "database_url validated by a cheap non-empty + contains-:// shape check; the PgPool connect at startup is authoritative (per plan)"
  - "added an example_config_is_valid test so the committed template can never drift out of validity"
metrics:
  duration_min: 9
  tasks: 2
  files: 6
  completed: "2026-06-15"
---

# Phase 4 Plan 02: Daemon Config Summary

The daemon's configuration front-end (OPS-01): a serde `Config` struct loaded from TOML via the `config` crate with a `WOT__*` env overlay, every optional field defaulting to the existing `DEFAULT_*` constants, a fail-fast `validate()` for the anchor pubkey / relay set / DB URL / TTL, a committed `config.example.toml` documenting every field, and green pure-unit tests proving load precedence, default fill, and validation. This struct is the single source of truth every later daemon task (loop sizing, bind address, intervals, log format) consumes.

## What Was Built

**Task 1 — Config struct + load + validate (commit c182b94):** `src/daemon/config.rs` defines `pub struct Config` (the full tunable set: `anchor_pubkey`, `relays`, `database_url`, `ttl`, `concurrency`, `batch_size`, `max_attempts`, `fetch_timeout`, `reqs_per_second`, `metrics_addr`, `log_level`, `log_format`, `progress_interval`, `staleness_scan_interval`, `reclaim_interval`, `reclaim_age`, `idle_poll_interval`) and `pub enum LogFormat { Human, Json }` (serde `rename_all = "lowercase"`, default `Human`). Every Duration field uses `#[serde(with = "humantime_serde")]`; every optional field's `#[serde(default = "default_*")]` fn returns the existing const by name (`DEFAULT_CONCURRENCY`/`DEFAULT_BATCH_SIZE`/`DEFAULT_MAX_ATTEMPTS` from `crawl`, `DEFAULT_REQS_PER_SECOND` from `relay::rate_limit`, `DEFAULT_FETCH_TIMEOUT` from `relay::fetch`). `load_config` layers `config::File::with_name(path)` then `config::Environment::default().prefix("WOT").separator("__")`. `validate` parses the anchor via `nostr_sdk::PublicKey::parse` (accepts hex + bech32), and `ensure!`s non-empty relays, `ttl > 0`, and a non-empty URL-shaped `database_url`. `Config`'s `Debug` is hand-written to redact `database_url` (T-04-03). Registered `pub mod config` in `src/daemon/mod.rs`.

**Task 2 — config.example.toml + tests (commit 13af02b):** `config.example.toml` documents every field with an inline comment, a real-looking anchor, a curated relay array, a placeholder `database_url` (with a "never logged" note), `ttl = "24h"`, all five interval fields, `metrics_addr = "127.0.0.1:9100"` (with a public-bind security warning), and the `DEFAULT_*`-valued optionals. `tests/daemon_config.rs` fills the Wave 0 `#[ignore]` stubs with `default_fill`, `override_precedence` (env beats file), `invalid_anchor_rejected`, `ttl_zero_rejected`, `empty_relays_rejected`, plus an added `example_config_is_valid` proving the committed template loads and validates. Tests are pure-unit (no DB): each writes a unique tempfile under `temp_dir()`; the suite runs single-threaded because `override_precedence` mutates process env.

## Verification Results

- `SQLX_OFFLINE=true cargo build --lib` — exit 0.
- `SQLX_OFFLINE=true cargo build --all-targets` — exit 0.
- `SQLX_OFFLINE=true cargo test --test daemon_config -- --test-threads=1` — 6 passed (`default_fill`, `override_precedence`, `invalid_anchor_rejected`, `ttl_zero_rejected`, `empty_relays_rejected`, `example_config_is_valid`).
- `cargo clippy --all-targets` — no warnings on `src/daemon/config.rs` or `tests/daemon_config.rs`.
- Acceptance greps: `pub struct Config`, `pub fn load_config`, `pub fn validate`, `DEFAULT_*` (4 matches), `humantime_serde` (7 matches), `prefix("WOT")` + `separator("__")` all present.
- `database_url` review: appears in `config.rs` only as the struct field, the redacted `Debug` (`<redacted>`), the validation `ensure!`s, and doc comments — never in a tracing field.
- No new `sqlx::query!` macros added (config is pure-unit) — `.sqlx/` unchanged, no `cargo sqlx prepare` needed.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] `serde` not declared as a direct dependency**
- **Found during:** Task 1 (`cargo build --lib`).
- **Issue:** `#[derive(Deserialize)]` and `use serde::Deserialize` failed (`unresolved import serde`, `cannot find attribute serde`). `serde` was only present transitively (via nostr-sdk/sqlx), not in `[dependencies]`, so its proc-macro/attribute were not in scope for the crate.
- **Fix:** Added `serde = { version = "1.0.228", features = ["derive"] }` to `[dependencies]`, pinned to the version already resolved in `Cargo.lock` (1.0.228, matching the CLAUDE.md stack table which lists serde "used directly for config"). This is a dependency *declaration* of an already-present, audited crate — not a new package install, no version change, no lock churn beyond the direct edge.
- **Files modified:** `Cargo.toml`, `Cargo.lock`.
- **Commit:** c182b94.

### Added beyond the plan

**2. [Rule 2 - Missing critical functionality] `example_config_is_valid` test**
- **Found during:** Task 2.
- **Rationale:** `config.example.toml` is the operator's starting template; a template that fails to load or validate is a latent correctness defect. Added a test that `load_config` + `validate` the committed example so it can never silently drift out of validity. Not in the plan's named behavior list but consistent with its intent (the example must document a *valid* config).
- **Files modified:** `tests/daemon_config.rs`.
- **Commit:** 13af02b.

## Threat Mitigations Applied

- **T-04-03** (Information Disclosure — database_url in logs / config echo): `Config`'s `Debug` is hand-written to print `database_url` as `<redacted>`; this is the only `Debug` impl, so `tracing::info!(?config, ...)` config-echo can never leak the URL. Verified no tracing field carries `database_url`.
- **T-04-04** (Tampering / DoS — malformed config silently accepted): `validate()` fail-fasts on a bad anchor, empty relays, empty/non-URL `database_url`, and `ttl <= 0`, each with an actionable message — proven by `invalid_anchor_rejected` / `ttl_zero_rejected` / `empty_relays_rejected`.

## Known Stubs

None. The Wave 0 `tests/daemon_config.rs` `#[ignore]` stubs this plan owned are now real, passing tests (zero `#[ignore]` remain in that file). The `metrics_addr`, `log_level`, `log_format`, and interval/`concurrency`/`batch_size`/`ttl` fields are defined-and-validated here and consumed by later plans (04-03 observe, 04-04 loop, 04-05 main) as planned — they are forward wiring points, not stubs.

## Self-Check: PASSED

- Created files exist: `src/daemon/config.rs`, `config.example.toml` — both FOUND.
- Commits present in git history: c182b94 (Task 1), 13af02b (Task 2) — both FOUND.
