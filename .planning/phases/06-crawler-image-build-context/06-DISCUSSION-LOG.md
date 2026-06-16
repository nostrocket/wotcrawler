# Phase 6: Crawler Image & Build Context - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-06-16
**Phase:** 6-Crawler Image & Build Context
**Areas discussed:** Runtime base + linking, Builder cache strategy, Non-root & rootfs posture, Version pinning / reproducibility

---

## Runtime base + linking

| Option | Description | Selected |
|--------|-------------|----------|
| distroless/cc:nonroot | glibc dynamic build on rust:1.94 builder; runtime = gcr.io/distroless/cc-debian12:nonroot (~22MB, ca-certs + glibc + nonroot baked in, no shell) | ✓ |
| debian:bookworm-slim | Same glibc build; runtime = debian:bookworm-slim + apt ca-certificates + explicit useradd (~75–90MB, has shell/apt for debugging) | |
| musl static → scratch | Static musl build → scratch/distroless-static (~10–15MB) — rejected: musl resolver/allocator quirks under heavy concurrent DNS + socket load | |

**User's choice:** distroless/cc:nonroot (Recommended)
**Notes:** Workload does heavy concurrent DNS + thousands of relay sockets, making musl risky. rustls (no OpenSSL) means runtime needs only ca-certificates, which distroless/cc provides. In-container debugging via docker logs + /metrics + /health, not a shell.

---

## Builder cache strategy

| Option | Description | Selected |
|--------|-------------|----------|
| cargo-chef | planner stage → recipe.json → cook cached dep layer → copy source → build. Best incremental rebuilds; one extra tool. | ✓ |
| Manual manifest-copy trick | COPY Cargo.toml/lock + dummy main.rs, build deps, then copy real source. No extra tooling; fiddlier with lib+bin layout. | |
| Naive copy-all | COPY everything, single cargo build. Simplest; recompiles ~200 deps on every source edit. | |

**User's choice:** cargo-chef (Recommended)
**Notes:** Pure rebuild-speed decision; no effect on the final image. Final build runs with SQLX_OFFLINE=1.

---

## Non-root & rootfs posture

### Sub-question: User identity / UID

| Option | Description | Selected |
|--------|-------------|----------|
| Built-in nonroot 65532 | Use distroless's USER nonroot (uid:gid 65532:65532). Zero setup, well-known fixed UID. | ✓ |
| Pin a custom UID (e.g. 10001) | Override to a project-specific numeric UID. Only worth it with an external convention. | |

**User's choice:** Built-in nonroot 65532 (Recommended)

### Sub-question: Read-only root filesystem

| Option | Description | Selected |
|--------|-------------|----------|
| Yes — target read-only rootfs | Crawler writes nothing locally; document image as read-only-rootfs-safe so Phase 7 sets read_only: true. | ✓ |
| No — leave rootfs writable | Don't constrain it; simpler but gives up a cheap hardening win. | |

**User's choice:** Yes — target read-only rootfs (Recommended)
**Notes:** State is in Postgres, logs to stdout, metrics/health in-memory — no local writes expected.

---

## Version pinning / reproducibility

| Option | Description | Selected |
|--------|-------------|----------|
| Tag-pin to specific versions | rust:1.94-bookworm + distroless/cc-debian12:nonroot. Auto CVE fixes on rebuild; not byte-reproducible; no manual upkeep. | ✓ |
| Digest-pin (sha256) both bases | Byte-reproducible + tamper-evident, but pins rot without Renovate/CI to bump them — no auto patches. | |

**User's choice:** Tag-pin to specific versions (Recommended)
**Notes:** Milestone explicitly excludes CI/Renovate/registry automation, so digest pins would be hand-maintained and rot into stale unpatched bases. Tag-pinning gets OS/CVE patches automatically on each operator rebuild.

---

## Claude's Discretion

- Exact Dockerfile stage names/ordering and whether to add a BuildKit `--mount=type=cache` for the cargo registry/git cache on top of cargo-chef.
- Precise ENTRYPOINT form (default `ENTRYPOINT ["/crawler"]`); config/`WOT__*` wiring deferred to Phase 7.
- Whether to `EXPOSE` the metrics port as a documentation hint vs leaving port publishing to Phase 7.
- `.dockerignore` additions beyond the required (`target/`, `config.toml`, `config.*.toml`, `.env`): also exclude `.git/` and `.planning/` to keep the cargo-chef build context small.

## Deferred Ideas

- Digest-pinning + automated base-image updates (Renovate) — out of scope (no CI this milestone).
- musl-static / scratch image — rejected for v1.1; revisit only if ultra-minimal images outweigh DNS/allocator risk.
- All runtime wiring (entrypoint config supply, healthchecks, port publishing, graceful-drain, read_only: true, .env.example, "Run with Docker" docs) → Phase 7.
