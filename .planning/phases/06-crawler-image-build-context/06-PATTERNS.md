# Phase 6: Crawler Image & Build Context - Pattern Map

**Mapped:** 2026-06-16
**Files analyzed:** 3 (1 new Dockerfile, 1 new .dockerignore, 1 modified .gitignore)
**Analogs found:** 0 / 3 (no in-repo container/build-tooling analog exists — see "No Analog Found")

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match Quality |
|-------------------|------|-----------|----------------|---------------|
| `Dockerfile` (new) | config (build/packaging) | batch (multi-stage compile → artifact copy) | none in repo | no-analog |
| `.dockerignore` (new) | config (build-context filter) | transform (context exclusion) | `.gitignore` (weak — different syntax/tool, same "ignore-glob" intent) | partial |
| `.gitignore` (modify) | config | transform | itself (edit in place) | exact (self) |

## Pattern Assignments

### `Dockerfile` (config, batch) — NET-NEW, NO IN-REPO ANALOG

There is **no existing Dockerfile, compose file, or container tooling anywhere in the repo** (verified: `find` across the tree, excluding `target/`, returns nothing; `ops/` contains only `grafana-dashboard.json`). The planner MUST NOT force a weak analog. Instead, build the Dockerfile from:

1. The **user-endorsed concrete shape** in `06-CONTEXT.md` `<specifics>` (lines 98–113) — this is the authoritative skeleton.
2. The **canonical build-input files** below, which the Dockerfile references. These are the real "patterns to copy from" — the Dockerfile's correctness is defined by matching these existing repo facts, not by mirroring another Dockerfile.

**Canonical build inputs the Dockerfile MUST stay consistent with** (all verified to exist):

| Build input | Verified fact | Dockerfile consequence |
|-------------|---------------|------------------------|
| `rust-toolchain.toml:2` | `channel = "1.94.0"` | Builder base tag must be `rust:1.94-bookworm` (D-04) — exact major.minor match. |
| `Cargo.toml:5` | `rust-version = "1.94"` | Confirms the 1.94 toolchain floor; builder image satisfies it. |
| `Cargo.toml:4` | `edition = "2021"` | No edition-2024 toolchain needed. |
| `Cargo.toml:7-9` | `[[bin]] name = "crawler"`, `path = "src/main.rs"` | Release artifact path is `/app/target/release/crawler`; `COPY --from=builder ... /crawler`; `ENTRYPOINT ["/crawler"]`. |
| `Cargo.lock` (present, 113 KB) | committed | Reproducible dep resolution; cargo-chef `cook` consumes it. |
| `.sqlx/` (27 query files, verified count) | committed offline metadata | Build sets `SQLX_OFFLINE=true` (D-06) so `sqlx::query!` macros compile with **no live `DATABASE_URL`** (IMAGE-01, success criterion 1). |
| `src/store/mod.rs:45` | `sqlx::migrate!("./migrations")` — verified exact line | Migrations are **embedded at compile time** → runtime image needs ONLY the binary; do NOT `COPY migrations/` into the final stage (D-06/D-07). |
| `migrations/` (`0001`–`0004`, verified) | present at build time | Must be in the **builder** context for the `migrate!` embed macro to read; not in the runtime image. |
| `src/main.rs:32-35` | binary requires `--config <path>` (clap, required arg) | Image is buildable + runnable-with-args, NOT self-starting; config/`WOT__*` supply is Phase 7. `ENTRYPOINT ["/crawler"]` only (no `CMD` config path). |
| `Cargo.toml:12,42` | sqlx `tls-rustls`, reqwest `rustls-tls`; no openssl feature anywhere | Runtime needs only `ca-certificates` (present in `distroless/cc`); builder needs NO `libssl-dev`/`pkg-config`/`apt-get` TLS dance. |

**Endorsed Dockerfile skeleton** (`06-CONTEXT.md` lines 98–113 — copy this shape; stage names / BuildKit `--mount=type=cache` / `EXPOSE` are Claude's discretion per D-Discretion lines 48–50):

```dockerfile
FROM rust:1.94-bookworm AS chef
RUN cargo install cargo-chef
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json
FROM chef AS builder
COPY --from=planner /app/recipe.json .
RUN cargo chef cook --release          # cached dep layer (~200 crates: nostr-sdk, sqlx, ...)
COPY . .
RUN SQLX_OFFLINE=1 cargo build --release
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /app/target/release/crawler /crawler
USER nonroot
ENTRYPOINT ["/crawler"]
```

**Locked decisions that constrain the build (from `<decisions>`):**
- D-01: runtime base `gcr.io/distroless/cc-debian12:nonroot` (glibc-dynamic, ships ca-certificates + nonroot user, no shell/pkg-mgr).
- D-02: glibc-dynamic, NOT musl/scratch (DNS/allocator risk under heavy concurrent resolution); rustls everywhere so no OpenSSL.
- D-05: cargo-chef dependency-layer caching (planner → recipe.json → `cargo chef cook --release` → real source build).
- D-06: `SQLX_OFFLINE=true` at build; no live DB.
- D-07: final stage copies ONLY `/app/target/release/crawler`.
- D-08: `USER nonroot` (built-in uid:gid 65532:65532; no `useradd`).
- D-09: read-only-rootfs-safe (no local writes); document for Phase 7 `read_only: true`. No tmpfs assumed.
- D-10: tag-pin both bases (`rust:1.94-bookworm`, `...:nonroot`); NO `@sha256` digest pin.
- D-Discretion: default `ENTRYPOINT ["/crawler"]`; `EXPOSE` of metrics port is optional documentation-only.

---

### `.dockerignore` (config, transform) — NET-NEW, partial analog: `.gitignore`

**Analog:** `.gitignore` (weak/partial). It is the only existing ignore-glob file in the repo. The `.dockerignore` syntax is similar (glob-per-line) but the tool, purpose, and required entries differ — treat `.gitignore` as a *format reference only*, not a content source.

**Current `.gitignore` content** (verified, full file — 1 line):
```
/target
```

**`.dockerignore` required entries** (D-11, IMAGE-03 / CONFIG-02):
```
target/
config.toml
config.*.toml
.env
```

**`.dockerignore` additional entries** (D-12 — shrink the cargo-chef `COPY . .` context):
```
.git/
.planning/
```

**Glob-safety note (D-12, verified):** `config.example.toml` IS committed at repo root and is intentionally NOT secret. The `config.*.toml` glob WOULD match `config.example.toml` — that is acceptable for Phase 6 because the build does not need it in-image. The planner must NOT add a narrower exclude that breaks this; just confirm the build doesn't require `config.example.toml` (it does not).

---

### `.gitignore` (config, transform) — MODIFY, self-analog

**Analog:** itself (edit in place). Current content is the single line `/target` (verified).

**Change required** (D-13, CONFIG-02): add `.env` so the gitignored-`.env` contract holds. Resulting file:
```
/target
.env
```
(Creating `.env.example` is Phase 7 / DOCS-02 — out of scope here.)

## Shared Patterns

### Tag-pinning, no digest (applies to both `FROM` lines)
**Source of decision:** D-10. **Apply to:** both Dockerfile base images.
Pin to mutable tags (`rust:1.94-bookworm`, `gcr.io/distroless/cc-debian12:nonroot`); deliberately NO `@sha256` digest pin (no CI/Renovate this milestone → auto-patching wins over byte-reproducibility).

### rustls-only TLS posture (applies to builder + runtime)
**Source:** `Cargo.toml:12` (sqlx `tls-rustls`), `Cargo.toml:42` (reqwest `rustls-tls`); no openssl feature in the manifest.
**Apply to:** builder stage (no `apt-get install libssl-dev pkg-config`) and runtime stage (only `ca-certificates`, already in `distroless/cc`).

### Offline-build contract (applies to builder stage)
**Source:** `.sqlx/` (27 files) + `src/store/mod.rs:45` `sqlx::migrate!("./migrations")`.
**Apply to:** the release-build step — set `SQLX_OFFLINE=true`; ensure `.sqlx/` and `migrations/` are present in the builder context (NOT excluded by `.dockerignore`), and that `migrations/` is NOT copied to the runtime stage.

## No Analog Found

The primary deliverables have no existing in-repo pattern to mirror. The repo has **never contained container tooling** (confirmed by tree-wide `find` for `Dockerfile`/`*compose*`/`.dockerignore` → empty; `ops/` holds only `grafana-dashboard.json`).

| File | Role | Data Flow | Reason / planner guidance |
|------|------|-----------|---------------------------|
| `Dockerfile` | config (build) | batch | No prior Dockerfile/compose in repo. Build from `06-CONTEXT.md` `<specifics>` skeleton + the canonical build-input files table above (rust-toolchain, Cargo.toml bin target, `.sqlx/`, `src/store/mod.rs:45`). Do NOT invent a non-existent analog. |
| `.dockerignore` | config (filter) | transform | Only `.gitignore` exists as a format reference (glob-per-line). Content is driven by D-11/D-12 required entries, not by mirroring another file. |

## Metadata

**Analog search scope:** repo root, `ops/`, `src/`, `migrations/`, `.sqlx/`, full tree via `find` (excluding `target/`).
**Files scanned/verified:** `.gitignore`, `rust-toolchain.toml`, `Cargo.toml`, `src/store/mod.rs` (head, line 45 confirmed), `src/main.rs` (head, `--config` required arg confirmed), `.sqlx/` (27 files confirmed), `migrations/` (0001–0004 confirmed), `ops/` (grafana JSON only).
**Pattern extraction date:** 2026-06-16
