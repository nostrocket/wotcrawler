# Roadmap: Nostr Web-of-Trust Crawler & Data Layer

## Overview

From one anchor pubkey, this project builds and continuously refreshes a complete directed follow graph of everyone reachable through nostr kind-3 follows, persisted in a shared PostgreSQL database the downstream spam layer reads directly.

## Milestones

- ✅ **v1.0 Crawler & Data Layer** — Phases 1–5 (shipped 2026-06-16) — full graph crawler + data layer: schema/contract, relay acquisition + validation, transactional graph writer + crash-safe BFS frontier, unattended daemon with staleness loop + observability, and NIP-65 outbox-routing fallback + relay health scoring. Full detail: [milestones/v1.0-ROADMAP.md](milestones/v1.0-ROADMAP.md).
- 🚧 **v1.1 Containerized Deployment** — Phases 6–7 — deployment/ops tooling only: a multi-stage Dockerfile for the `crawler` binary and a docker-compose stack bringing up Postgres + crawler, env-configured, with documented live-logs/metrics/health access and a preserved cross-process DB boundary for the downstream spam layer. No change to crawl behavior, schema, or relay logic.

## Phases

<details>
<summary>✅ v1.0 Crawler & Data Layer (Phases 1–5) — SHIPPED 2026-06-16</summary>

- [x] Phase 1: Schema & Data Contract (3/3 plans) — completed 2026-06-12
- [x] Phase 2: Relay Acquisition & Validation (12/12 plans) — completed 2026-06-13
- [x] Phase 3: Graph Writer & BFS Frontier (3/3 plans) — completed 2026-06-13
- [x] Phase 4: Daemon, Staleness Loop & Observability (5/5 plans) — completed 2026-06-15
- [x] Phase 5: NIP-65 Outbox Routing & Relay Health (4/4 plans) — completed 2026-06-16

Full phase details, success criteria, and milestone summary: [milestones/v1.0-ROADMAP.md](milestones/v1.0-ROADMAP.md).
Audit: [milestones/v1.0-MILESTONE-AUDIT.md](milestones/v1.0-MILESTONE-AUDIT.md) (status: tech_debt — 29/29 requirements satisfied, operator UAT deferred).

</details>

### v1.1 Containerized Deployment (Phases 6–7)

- [ ] **Phase 6: Crawler Image & Build Context** — Multi-stage Dockerfile producing a minimal, non-root runtime image with a secret-free build context.
- [ ] **Phase 7: Compose Stack & Operator Workflow** — One-command Postgres + crawler stack, env-driven config, live logs/metrics/health access, preserved DB boundary, and "Run with Docker" docs.

## Phase Details

### Phase 6: Crawler Image & Build Context

**Goal**: The operator can build a small, secure, runnable crawler image from source — no Rust toolchain in the runtime image, runs as non-root, and the build context can never leak local secrets or artifacts.
**Depends on**: v1.0 (the `crawler` binary and committed `.sqlx/` offline metadata already exist).
**Requirements**: IMAGE-01, IMAGE-02, IMAGE-03
**Success Criteria** (what must be TRUE):

  1. Running `docker build` against the committed multi-stage Dockerfile produces a runnable crawler image, and the build needs no live DATABASE_URL — the builder stage compiles the release binary against the committed `.sqlx/` offline metadata.
  2. The runtime image carries only the release binary plus required runtime libraries — no Rust/cargo build toolchain — and inspecting the running image shows the crawler process runs as a non-root user.
  3. A committed `.dockerignore` excludes `target/`, local `config.toml` / `config.*.toml`, and `.env`, so those are absent from the build context and never baked into the image.**Plans**: 1 plan
- [ ] 06-01-PLAN.md — Multi-stage Dockerfile, .dockerignore, and .gitignore .env exclusion (IMAGE-01/02/03)

### Phase 7: Compose Stack & Operator Workflow

**Goal**: The operator can bring up the full Postgres + crawler stack with one command, configure it entirely through environment variables (with the DB URL injected as a secret), watch live logs and reach metrics/health from the host, shut it down with a graceful drain, and follow README docs to do all of it — while the downstream spam layer can still connect to the same database read-only.
**Depends on**: Phase 6 (the compose stack runs the image built there).
**Requirements**: COMPOSE-01, COMPOSE-02, COMPOSE-03, COMPOSE-04, COMPOSE-05, CONFIG-01, CONFIG-02, CONFIG-03, LOGS-01, LOGS-02, LOGS-03, DOCS-01, DOCS-02
**Success Criteria** (what must be TRUE):

  1. Running `docker compose up` brings up Postgres and the crawler wired together; the crawler waits for a healthy Postgres (compose healthcheck + `depends_on`), auto-applies schema migrations on startup, and reaches a healthy `/health/ready` state.
  2. All crawler configuration is supplied via `WOT__*` environment variables in compose / a gitignored `.env` (with `database_url` injected as a secret and a committed `.env.example` documenting every required and optional key); changing settings requires no code change or image rebuild, and missing/invalid required config (anchor, relays, `database_url`, TTL) fails fast with an actionable, secret-free error before any DB or relay traffic.
  3. Crawler logs stream live via `docker compose logs -f crawler`, log format/level are changeable through the environment (`WOT__LOG_FORMAT` / `WOT__LOG_LEVEL` / `RUST_LOG`) without a rebuild, and `/metrics`, `/health/live`, `/health/ready` are reachable from the host (the container binds `metrics_addr` to `0.0.0.0` and the port is published).
  4. Postgres data survives `docker compose down`/`up` cycles via a named volume, the published Postgres host port lets a separate downstream spam-layer process connect read-only per `SCHEMA.md`, and `docker compose down` (SIGTERM to the crawler) triggers the existing graceful drain — leaving zero orphaned `in_progress` rows with the data volume preserved.
  5. The README has a "Run with Docker" section covering build, configure (`.env`), bring-up, viewing live logs, reaching metrics/health, and how the downstream spam layer connects.

**Plans**: TBD

## Progress

| Phase | Milestone | Plans Complete | Status | Completed |
|-------|-----------|----------------|--------|-----------|
| 1. Schema & Data Contract | v1.0 | 3/3 | Complete | 2026-06-12 |
| 2. Relay Acquisition & Validation | v1.0 | 12/12 | Complete | 2026-06-13 |
| 3. Graph Writer & BFS Frontier | v1.0 | 3/3 | Complete | 2026-06-13 |
| 4. Daemon, Staleness Loop & Observability | v1.0 | 5/5 | Complete | 2026-06-15 |
| 5. NIP-65 Outbox Routing & Relay Health | v1.0 | 4/4 | Complete | 2026-06-16 |
| 6. Crawler Image & Build Context | v1.1 | 0/1 | Not started | - |
| 7. Compose Stack & Operator Workflow | v1.1 | 0/? | Not started | - |

## Deferred to operator UAT (v1.0)

- Live-relay crawl run against real curated relays (Phase 4/5) — every automatable criterion passed in-session.
- Grafana dashboard render against a live Prometheus (OBS-05).

See `.planning/STATE.md` → Deferred Items.
