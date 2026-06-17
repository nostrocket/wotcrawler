---
phase: 06-crawler-image-build-context
reviewed: 2026-06-17T00:00:00Z
depth: standard
files_reviewed: 3
files_reviewed_list:
  - Dockerfile
  - .dockerignore
  - .gitignore
findings:
  critical: 0
  warning: 2
  info: 2
  total: 4
status: issues_found
summary: "Two reproducibility warnings (unpinned cargo-chef version, missing --locked on cargo commands); two informational items; no critical security or correctness issues."
---

# Phase 6: Code Review Report

**Reviewed:** 2026-06-17
**Depth:** standard
**Files Reviewed:** 3 (Dockerfile, .dockerignore, .gitignore)
**Status:** issues_found

## Summary

All three phase-6 artifacts were reviewed against the locked decisions (D-01 through D-13) and the endorsed skeleton in `06-CONTEXT.md`. The overall posture is sound: the Dockerfile correctly implements the cargo-chef multi-stage pattern; the distroless non-root runtime is correct; `SQLX_OFFLINE=true` is scoped to the builder stage only and does not leak into the runtime image; the `.dockerignore` retains `.sqlx/` and `migrations/` as required while excluding `target/`, local config, and `.env`; and the `.gitignore` seeds the CONFIG-02 contract correctly.

Two reproducibility-grade warnings were found in the Dockerfile: `cargo install cargo-chef` has no version pin, and neither `cargo chef cook` nor `cargo build` uses `--locked`. Both of these are well-known Rust Docker best-practice items; neither causes incorrect behavior on a first build against a consistent committed `Cargo.lock`, but they weaken the build-to-build reproducibility guarantee.

No critical security, data loss, or correctness issues were found. The locked decisions (D-01 through D-13) are all faithfully implemented.

Out-of-scope items confirmed NOT flagged: no digest pin (@sha256) — deliberate per D-10; USER nonroot redundancy with `:nonroot` image variant — intentional belt-and-suspenders; absence of compose/healthchecks/runtime-config — Phase 7.

---

## Warnings

### WR-01: `cargo install cargo-chef` has no version pin

**File:** `Dockerfile:12`

**Issue:** `RUN cargo install cargo-chef` installs the latest version of cargo-chef at build time. The `chef` stage is shared by both `planner` and `builder` (both `FROM chef AS ...`), so within a single build run both stages use the same binary — no cross-stage mismatch. The risk is across rebuilds: if the `chef` layer is invalidated (e.g., after a `docker build --no-cache` or a `rust:1.94-bookworm` base image update) a new version of cargo-chef is silently installed. If that new version has a different `recipe.json` format, the previously cached `cargo chef cook` layer — if somehow reused from an older build — will be incompatible. In practice, cargo-chef has maintained format stability, but the guarantee is absent from the Dockerfile.

**Fix:** Pin to the current known-good version:
```dockerfile
RUN cargo install cargo-chef --version 0.1.71 --locked
```
(Verify current stable version at crates.io; `--locked` on `cargo install` itself prevents the installer from updating the lockfile of the installed crate.)

---

### WR-02: `cargo build --release` and `cargo chef cook --release` omit `--locked`

**File:** `Dockerfile:24` (cook), `Dockerfile:27` (build)

**Issue:** Neither `cargo chef cook --release --recipe-path recipe.json` nor `cargo build --release` passes `--locked`. Without `--locked`, if `Cargo.toml` and `Cargo.lock` ever drift (e.g., a dependency version constraint in `Cargo.toml` is updated but `Cargo.lock` is not re-committed), cargo silently resolves to different — potentially newer or incompatible — dependency versions instead of failing the build. For a security-sensitive daemon that handles live Nostr relay connections and writes to a shared database, silent dependency version drift is a real supply-chain concern. The committed `Cargo.lock` is the practical safeguard, but `--locked` enforces it as a build-time invariant and surfaces drift as a loud build failure rather than a silent behavior change.

**Fix:**
```dockerfile
RUN cargo chef cook --release --locked --recipe-path recipe.json
```
```dockerfile
RUN cargo build --release --locked
```

---

## Info

### IN-01: `USER nonroot` is redundant with the `:nonroot` image variant

**File:** `Dockerfile:35`

**Issue:** The runtime base `gcr.io/distroless/cc-debian12:nonroot` already sets `USER=nonroot` (uid 65532) in its image configuration. The explicit `USER nonroot` directive on line 35 is therefore a no-op — the same user is already active. This is not a bug; it is conventional belt-and-suspenders documentation that makes the non-root intent visible without reading the base image's metadata.

**Fix:** No change required. Optionally add a comment clarifying the redundancy is intentional:
```dockerfile
# :nonroot base already sets USER=nonroot (uid 65532); explicit here for documentation.
USER nonroot
```

---

### IN-02: `ops/`, `tests/`, and documentation files are included in the build context

**File:** `.dockerignore`

**Issue:** `ops/` (grafana dashboard JSON, ~12 kB) and `tests/` (~284 kB of integration test source files) are not excluded from the build context, and neither are top-level documentation files (`README.md`, `SCHEMA.md`, `AGENTS.md`, `CLAUDE.md`). The SUMMARY records the total context transfer as 24.12 kB — confirming these files are present but that `target/` exclusion dominates and the total is already small. This is not a correctness or security issue; it is a minor hygiene item for future builds if the test suite grows significantly.

**Fix:** Optionally add to `.dockerignore` for a leaner context:
```
ops/
tests/
*.md
```
Defer to Phase 7 or a future housekeeping task if context size is not a concern.

---

_Reviewed: 2026-06-17_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
