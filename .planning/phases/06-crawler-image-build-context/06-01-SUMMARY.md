---
phase: 06-crawler-image-build-context
plan: 01
subsystem: infra
tags: [docker, dockerfile, cargo-chef, distroless, sqlx-offline, dockerignore, build-context]

# Dependency graph
requires:
  - phase: 01-schema-and-data-contract
    provides: committed .sqlx/ offline query metadata + migrations/ (builder compiles offline against these)
  - phase: 04-daemon-staleness-loop-and-observability
    provides: the `crawler` bin target (clap --config / --help) that the runtime image ships and runs
provides:
  - Multi-stage cargo-chef Dockerfile (chef -> planner -> builder -> distroless runtime) building the crawler image from source with no live DATABASE_URL
  - Minimal non-root (uid 65532) distroless runtime image carrying only the release binary — no Rust/cargo build toolchain
  - Secret-free, small build context via committed .dockerignore (target/, config.toml, config.*.toml, .env, .git/, .planning/ excluded; .sqlx/ and migrations/ retained)
  - .gitignore .env exclusion (CONFIG-02 gitignored-.env seed)
affects: [07-compose-stack-and-operator-workflow]

# Tech tracking
tech-stack:
  added: [rust:1.94-bookworm builder base, cargo-chef, gcr.io/distroless/cc-debian12:nonroot runtime base]
  patterns:
    - "Multi-stage cargo-chef build: chef installs cargo-chef -> planner emits recipe.json -> builder cargo chef cook --release (cached dependency layer) then SQLX_OFFLINE release build -> distroless runtime copies only the binary"
    - "Offline image build: SQLX_OFFLINE=true compiles against committed .sqlx/, so no DATABASE_URL is required at build time"
    - "Tag-pinned base images (no @sha256 digest, D-10); rustls-only stack means no libssl/pkg-config apt installs"

key-files:
  created:
    - Dockerfile
    - .dockerignore
  modified:
    - .gitignore

key-decisions:
  - "Stage names chef/planner/builder; runtime stage unnamed — matches CONTEXT.md endorsed skeleton"
  - "No BuildKit --mount=type=cache: cargo-chef already provides dependency-layer caching (D-05); a cache mount adds BuildKit-version coupling for marginal gain"
  - "ENTRYPOINT exec form [\"/crawler\"] — image is runnable-with-args, not self-starting; config supply is Phase 7"
  - "EXPOSE 9100 as a documentation-only metrics-port hint (publishes nothing)"
  - "config.*.toml glob intentionally matches the committed config.example.toml — acceptable for Phase 6 (build does not need it in-context); no narrower negation added (D-12)"
  - ".sqlx/ and migrations/ deliberately NOT excluded from the build context — the builder compiles against .sqlx/ offline and sqlx::migrate!(\"./migrations\") embeds migrations at compile time"

patterns-established:
  - "cargo-chef dependency caching: cargo chef cook --release runs before COPY . . so the ~200-crate dependency layer caches across source changes"
  - "Distroless debugging posture (D-03): no in-container shell — the image is verified by invoking the binary directly (docker run --rm <img> --help) and debugged via docker logs + Phase 7 metrics/health endpoints, not by shelling in"

requirements-completed: [IMAGE-01, IMAGE-02, IMAGE-03]

# Metrics
duration: ~3 min (execution; excludes human-verify checkpoint wait)
completed: 2026-06-17
---

# Phase 6 Plan 01: Crawler Image & Build Context Summary

**Multi-stage cargo-chef Dockerfile building a 16.2 MB non-root distroless `crawler` image offline (SQLX_OFFLINE, no DATABASE_URL), plus a secret-free `.dockerignore` and a `.env` `.gitignore` exclusion.**

## Performance

- **Duration:** ~3 min execution (Tasks 1–2 committed within ~1 min of each other; Task 3 was a blocking human-verify checkpoint awaiting operator/orchestrator action)
- **Started:** 2026-06-16T16:49:26Z (Task 1 commit)
- **Completed:** 2026-06-17 (checkpoint approved, plan closed)
- **Tasks:** 3 (2 auto + 1 human-verify checkpoint)
- **Files modified:** 3 (Dockerfile, .dockerignore created; .gitignore modified)

## Accomplishments

- Multi-stage `Dockerfile` (chef -> planner -> builder -> distroless runtime) that compiles the `crawler` release binary offline against committed `.sqlx/` metadata — **no live DATABASE_URL needed at build time** (IMAGE-01).
- Minimal **non-root** (`nonroot`, uid 65532) distroless runtime carrying **only the release binary** — verified 16.2 MB / 19 layers with no Rust/cargo/cargo-chef toolchain layers (IMAGE-02).
- Committed `.dockerignore` keeps `target/` (28 GB on disk), local config, and `.env` out of the build context (transfer was 24.12 kB) — never bakes in secrets or local artifacts (IMAGE-03).
- `.gitignore` now excludes `.env`, seeding the CONFIG-02 gitignored-`.env` contract that Phase 7 completes.

## Task Commits

Each task was committed atomically:

1. **Task 1: Build-context hygiene — .dockerignore and .gitignore** — `847a78c` (chore)
2. **Task 2: Multi-stage Dockerfile (cargo-chef builder -> distroless runtime)** — `21113ea` (feat)
3. **Task 3: Docker build/inspect/run verification** — human-verify checkpoint (no code; dynamic verification record below)

**Plan metadata:** committed separately (docs: complete plan).

## Files Created/Modified

- `Dockerfile` — 4-stage build: `chef` (rust:1.94-bookworm + cargo-chef) -> `planner` (recipe.json) -> `builder` (`cargo chef cook --release`, then `SQLX_OFFLINE` release build) -> runtime (`gcr.io/distroless/cc-debian12:nonroot`, `COPY --from=builder /app/target/release/crawler /crawler`, `USER nonroot`, `EXPOSE 9100` doc-only, `ENTRYPOINT ["/crawler"]`).
- `.dockerignore` — globs `target/`, `config.toml`, `config.*.toml`, `.env`, `.git/`, `.planning/`; deliberately does NOT exclude `.sqlx/` or `migrations/` (the builder needs both).
- `.gitignore` — added `.env` below the preserved `/target` line.

## Decisions Made

- Stage names `chef`/`planner`/`builder`, runtime stage unnamed (matches the CONTEXT.md endorsed skeleton).
- No BuildKit `--mount=type=cache` — cargo-chef already provides the dependency-layer caching D-05 requires; a cache mount would add BuildKit-version coupling for marginal gain.
- `ENTRYPOINT ["/crawler"]` (exec form) — runnable-with-args, not self-starting; runtime config/`WOT__*` wiring is Phase 7.
- `EXPOSE 9100` as a documentation-only metrics-port hint (publishes nothing).
- The `config.*.toml` glob intentionally matches `config.example.toml`; no narrower negation added (D-12) — the build does not need that file in-context.
- `.sqlx/` and `migrations/` intentionally retained in the build context (offline compile + `sqlx::migrate!("./migrations")` embed at compile time).

## Dynamic Verification Record (Task 3 — human-verify checkpoint, APPROVED)

All five checks passed against the committed Dockerfile's exact multi-stage logic:

1. **IMAGE-01** — `docker build` succeeded with **NO DATABASE_URL** set; the builder compiled offline via `SQLX_OFFLINE=true` against committed `.sqlx/` (`cargo chef cook --release`, then `cargo build --release`).
2. **IMAGE-02 (size/layers)** — final image **16.2 MB, 19 layers**; `docker history` shows only distroless base layers + a single `COPY /app/target/release/crawler /crawler` (18.6 MB layer) — **no cargo/rustc/cargo-chef toolchain layers**.
3. **IMAGE-02 (non-root)** — `docker inspect --format '{{.Config.User}}'` = **`nonroot`** (uid 65532).
4. **IMAGE-02 (runnable / link proof)** — `docker run --rm wot-crawler:phase6 --help` printed clap usage (`Usage: crawler --config <CONFIG>`) and exited 0 — the binary is present, executable, and dynamically links against distroless glibc + ca-certificates (no missing-library error).
5. **IMAGE-03** — build-context transfer was **24.12 kB** (`target/` = 28 GB on disk was excluded); `.dockerignore` respected.

## Deviations from Plan

None — plan executed exactly as written. Tasks 1 and 2 committed verbatim against the endorsed skeleton; the committed `Dockerfile`, `.dockerignore`, and `.gitignore` were NOT modified during checkpoint verification.

## Issues Encountered

**Docker-host build-network throttle (environmental, transport-only — no artifact change).** This Docker host's `docker build` network is throttled to ~1 MB/s and stalls rustup's pinned-toolchain (Rust 1.94.0) download, so a plain `docker build .` could not complete **in this environment**. The host network and `docker run` network are full speed.

To complete the dynamic verification, the orchestrator pre-seeded a local base image (synced the 1.94.0 toolchain + cargo-chef + a warm crate cache via the fast run-network) and built the committed Dockerfile's **identical** multi-stage logic against it offline (`CARGO_NET_OFFLINE=true`). This is a transport-only workaround — the committed `Dockerfile`, `.dockerignore`, and `.gitignore` were not touched, and the build completes normally via plain `docker build .` on any host with healthy crates.io / static.rust-lang.org access.

**Deployer note:** run a plain `docker build .` once on a normally-networked machine to confirm end-to-end. The slow path here is solely the pinned-toolchain/crate download over the throttled build-network; the Dockerfile logic itself is proven by the five checks above.

## User Setup Required

None — no external service configuration required for this plan. (Phase 7 introduces the `.env` / `.env.example` runtime config and compose stack.)

## Next Phase Readiness

- IMAGE-01/02/03 satisfied: a small, secure, runnable crawler image builds from source with a secret-free context. Phase 7 (Compose Stack & Operator Workflow) can now run this image as a one-command Postgres + crawler stack.
- CONFIG-02 partially seeded (`.env` gitignored); Phase 7 completes it with secret injection of `database_url` and a committed `.env.example`.
- Carried concern for Phase 7: the container must bind `metrics_addr` to `0.0.0.0` (compiled default `127.0.0.1:9100` is host-unreachable) and publish the metrics + Postgres host ports.

## Self-Check: PASSED

- FOUND: Dockerfile (HEAD, unmodified)
- FOUND: .dockerignore (HEAD, unmodified)
- FOUND: .gitignore (`.env` exclusion present)
- FOUND commit 847a78c (Task 1)
- FOUND commit 21113ea (Task 2)

---
*Phase: 06-crawler-image-build-context*
*Completed: 2026-06-17*
