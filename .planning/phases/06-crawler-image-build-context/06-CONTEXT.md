# Phase 6: Crawler Image & Build Context - Context

**Gathered:** 2026-06-16
**Status:** Ready for planning

<domain>
## Phase Boundary

Deliver a committed **multi-stage Dockerfile** and a **`.dockerignore`** that build a small, secure, runnable `crawler` image from source:

- Builder stage compiles the release binary; **no Rust/cargo toolchain in the runtime image**.
- Build needs **no live `DATABASE_URL`** — it compiles against the committed `.sqlx/` offline metadata.
- Runtime image runs the crawler as a **non-root user**.
- `.dockerignore` keeps `target/`, local config, and `.env` out of the build context so they can never be baked into the image.

Satisfies **IMAGE-01, IMAGE-02, IMAGE-03**.

**Out of this phase (Phase 7):** compose stack, how config / `--config` / `WOT__*` / the DB-URL secret are supplied at runtime, healthchecks, port publishing, graceful-drain wiring, `.env.example`, and "Run with Docker" docs. No change to crawl behavior, schema, or relay logic.

</domain>

<decisions>
## Implementation Decisions

### Runtime base & linking
- **D-01:** Build **glibc-dynamic** (no musl). Runtime image is **`gcr.io/distroless/cc-debian12:nonroot`** — ~20–25 MB, ships glibc + `ca-certificates` + a non-root user, no shell / package manager.
- **D-02:** Rationale: this crawler does heavy concurrent DNS resolution + thousands of relay sockets; musl's stub resolver/allocator quirks under that load are a real runtime risk, so musl-static→scratch was explicitly rejected. rustls is used everywhere (sqlx, reqwest) so **no OpenSSL/system TLS libs are needed** — `ca-certificates` (present in `distroless/cc`) is the only TLS runtime dependency.
- **D-03:** Debugging the running container is via `docker logs` + the `/metrics`, `/health/live`, `/health/ready` endpoints (Phase 7 / LOGS-03), **not** an in-container shell — distroless has none, and that's accepted.

### Builder & build caching
- **D-04:** Builder base is **`rust:1.94-bookworm`** (full image; cargo present). Matches `rust-toolchain.toml` (1.94.0) and `Cargo.toml` `rust-version = "1.94"`.
- **D-05:** Use **cargo-chef** for dependency-layer caching: a `planner` stage produces `recipe.json`, a `builder` stage runs `cargo chef cook --release` (cached dep layer for ~200 crates incl. nostr-sdk/sqlx), then copies real source and runs the release build. Source-only edits skip recompiling dependencies.
- **D-06:** The actual binary build sets **`SQLX_OFFLINE=true`** (env) so `sqlx::query!`/`migrate!` macros compile against committed `.sqlx/` — **no live DB at build time** (IMAGE-01, success criterion 1). Migrations are **embedded at compile time** (`sqlx::migrate!("./migrations")`, `src/store/mod.rs:45`), so the runtime image needs **only the binary** — not the `migrations/` directory.
- **D-07:** Final stage copies just the release binary (`/app/target/release/crawler`) from the builder into the distroless runtime.

### Non-root & rootfs posture
- **D-08:** Run as distroless's **built-in `nonroot` user (uid:gid 65532:65532)** via `USER nonroot`. No `useradd`, no custom UID — distroless has no shell for it and 65532 is a well-known fixed UID. The crawler owns no host volume (it writes nothing locally), so there is no volume-permission concern in Phase 7.
- **D-09:** Target a **read-only root filesystem**: the crawler holds no local state (graph is in Postgres, logs go to stdout, metrics/health are in-memory). Document the image as read-only-rootfs-safe so Phase 7 compose can set `read_only: true`. No tmpfs is assumed unless a dependency is later found to need `/tmp`.

### Version pinning / reproducibility
- **D-10:** **Tag-pin** both bases (`rust:1.94-bookworm`, `gcr.io/distroless/cc-debian12:nonroot`); **no `@sha256` digest pin**. Rationale: this milestone explicitly excludes CI / Renovate / registry automation, so a digest pin would have to be bumped by hand and would rot into a stale, unpatched base. Tag-pinning means each operator rebuild pulls the latest patch of each line, so OS/CVE fixes arrive automatically. Byte-for-byte reproducibility is deliberately traded away for zero manual upkeep + auto-patching.

### Build-context hygiene (`.dockerignore`)
- **D-11:** `.dockerignore` MUST exclude (required by IMAGE-03 / CONFIG-02): `target/`, `config.toml`, `config.*.toml`, `.env`.
- **D-12:** Also exclude `.git/` and `.planning/` (large, not needed by the build) to keep the cargo-chef `COPY . .` context small. The local config file pattern is `config.toml` / `config.*.toml`; `config.example.toml` is committed and intentionally NOT secret — confirm the ignore globs don't accidentally drop `config.example.toml` if it's ever needed in-image (it is not needed for Phase 6).
- **D-13:** Add `.env` to the repo `.gitignore` (currently only contains `/target`) — required so the gitignored-`.env` contract (CONFIG-02) holds. (Creating `.env.example` itself is Phase 7 / DOCS-02.)

### Claude's Discretion
- Exact Dockerfile stage names/ordering, whether to add a BuildKit `--mount=type=cache` for the cargo registry/git cache on top of cargo-chef, and the precise `ENTRYPOINT` form. Default: `ENTRYPOINT ["/crawler"]`; config/`WOT__*` wiring is Phase 7, so the image alone is buildable + runnable-with-args, not self-starting.
- Whether to expose/`EXPOSE` the metrics port in the image (documentation-only hint) vs leaving port publishing entirely to Phase 7 compose.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Build inputs (must exist & be used)
- `Cargo.toml` — bin target is `crawler` (`path = "src/main.rs"`); `edition = "2021"`, `rust-version = "1.94"`. All TLS via rustls (`sqlx` `tls-rustls`, `reqwest` `rustls-tls`) — no OpenSSL.
- `rust-toolchain.toml` — pins channel `1.94.0`; builder image tag must match.
- `Cargo.lock` — committed; reproducible dependency resolution for the build.
- `.sqlx/` — 27 committed offline query files; the builder compiles against these with `SQLX_OFFLINE=true` (enables build without a live DB).
- `src/main.rs` — entry point; binary requires `--config <path>` and applies a `WOT__*` env overlay (runtime config supply is Phase 7).
- `src/store/mod.rs` §`run_migrations` (line ~45) — `sqlx::migrate!("./migrations")` **embeds** migrations at compile time → runtime image does NOT need `migrations/`.
- `migrations/` (`0001`–`0004`) — present at build time for the embed macro; not copied into the runtime image.

### Requirements / scope
- `.planning/REQUIREMENTS.md` — IMAGE-01/02/03 (this phase); CONFIG-02 (`.env` gitignored contract this phase seeds); Out-of-Scope table (no registry/CI publishing, no k8s).
- `.planning/ROADMAP.md` §"Phase 6" — goal + 3 success criteria.

### Cross-process boundary (context, mostly Phase 7)
- `SCHEMA.md` — the DB schema is the cross-project contract; the read-only `spam_layer` role must remain reachable. Relevant when reasoning about runtime behavior, not built into the image here.

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- Committed `.sqlx/` (27 query files) → enables offline compile; the builder just needs `SQLX_OFFLINE=true`.
- Embedded migrations (`sqlx::migrate!`) → runtime image is binary-only; no need to ship SQL files.

### Established Patterns
- rustls everywhere (no OpenSSL) → runtime needs only `ca-certificates`, satisfied by `distroless/cc`. No `apt-get install libssl`/`pkg-config` dance.
- Daemon writes nothing to local disk (state in Postgres, logs to stdout, metrics in-memory) → read-only rootfs is viable.
- `metrics_addr` compiled default is `127.0.0.1:9100` (must be set to `0.0.0.0:...` via `WOT__METRICS_ADDR` for host reachability — a **Phase 7** concern, flagged in STATE.md blockers; noted here so the image isn't expected to fix it).

### Integration Points
- Image is consumed by the Phase 7 compose stack; entrypoint + config injection + port publishing happen there.

</code_context>

<specifics>
## Specific Ideas

- Concrete shape the user endorsed (preview-confirmed):
  ```dockerfile
  FROM rust:1.94-bookworm AS chef
  RUN cargo install cargo-chef
  FROM chef AS planner
  COPY . .
  RUN cargo chef prepare --recipe-path recipe.json
  FROM chef AS builder
  COPY --from=planner /app/recipe.json .
  RUN cargo chef cook --release          # cached dep layer
  COPY . .
  RUN SQLX_OFFLINE=1 cargo build --release
  FROM gcr.io/distroless/cc-debian12:nonroot
  COPY --from=builder /app/target/release/crawler /crawler
  USER nonroot
  ENTRYPOINT ["/crawler"]
  ```
  (Illustrative — stage names/cache mounts are at Claude's discretion per D-Discretion.)

</specifics>

<deferred>
## Deferred Ideas

- **Digest-pinning + automated base-image updates** (Renovate/dependabot) — deliberately out of scope this milestone (no CI). Revisit only if image publishing/CI is added in a later milestone.
- **musl-static / scratch image** — rejected for v1.1 due to DNS/allocator risk under this workload; could be re-evaluated if a future need for ultra-minimal images outweighs the runtime risk.
- **All runtime wiring** (entrypoint config supply, healthchecks, port publishing, graceful-drain on SIGTERM, `read_only: true`, `.env.example`, "Run with Docker" docs) → **Phase 7**.

None of the above are scope creep into Phase 6 — discussion stayed within image/build-context scope.

</deferred>

---

*Phase: 6-Crawler Image & Build Context*
*Context gathered: 2026-06-16*
