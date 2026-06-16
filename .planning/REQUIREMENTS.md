# Requirements: Nostr Web-of-Trust Crawler — v1.1 Containerized Deployment

**Defined:** 2026-06-16
**Core Value:** From one anchor pubkey, maintain a complete and continuously fresh follow graph of everyone reachable through follows — fetched efficiently — so a downstream trust/spam layer can read it from a shared database at any time.

## v1.1 Requirements

Requirements for the Containerized Deployment milestone. Each maps to a roadmap phase. The "operator" is the single self-hosting operator.

### Image

- [ ] **IMAGE-01**: Operator can build the crawler image from source via a committed multi-stage Dockerfile (builder stage compiles the release binary; runtime stage carries only the binary + required runtime libraries).
- [ ] **IMAGE-02**: The runtime image runs the crawler as a non-root user and contains no Rust build toolchain (minimal size and attack surface).
- [ ] **IMAGE-03**: A `.dockerignore` excludes `target/`, local config, and `.env` so the build context is small and never bakes in secrets or local artifacts.

### Compose

- [ ] **COMPOSE-01**: Operator can bring up the full stack (Postgres + crawler) with a single `docker compose up` command.
- [ ] **COMPOSE-02**: Postgres data persists across container restarts and `docker compose down`/`up` cycles via a named volume.
- [ ] **COMPOSE-03**: The crawler starts only after Postgres is healthy (compose healthcheck + `depends_on`), and schema migrations are applied automatically on crawler startup.
- [ ] **COMPOSE-04**: The Postgres port is published on the host so the separate downstream spam-layer process can connect to the same database (read-only role per `SCHEMA.md`).
- [ ] **COMPOSE-05**: `docker compose down` (and SIGTERM to the crawler) triggers the existing graceful drain — the crawler stops claiming, drains in-flight leases to terminal status with zero orphaned `in_progress` rows, and the data volume is preserved.

### Config

- [ ] **CONFIG-01**: All crawler configuration (anchor pubkey, relays, TTLs, concurrency, and other tunables) is supplied via `WOT__*` environment variables in compose / an `.env` file — deploying with different settings requires no code change or image rebuild.
- [ ] **CONFIG-02**: `database_url` is injected via environment/secret and is never baked into the image or committed to git (`.env` is gitignored; an `.env.example` documents the required keys).
- [ ] **CONFIG-03**: The stack fails fast with an actionable, secret-free error if required config (anchor pubkey, non-empty relays, valid `database_url`, positive TTL) is missing or invalid — before any DB connection or relay traffic.

### Logs & Debugging

- [ ] **LOGS-01**: Crawler logs stream to stdout/stderr and are viewable live via `docker compose logs -f crawler`.
- [ ] **LOGS-02**: Operator can change log format (human/JSON) and log level via environment (`WOT__LOG_FORMAT` / `WOT__LOG_LEVEL` / `RUST_LOG`) without rebuilding the image.
- [ ] **LOGS-03**: The `/metrics`, `/health/live`, and `/health/ready` endpoints are reachable from the host for debugging the running container (the container binds `metrics_addr` to `0.0.0.0` and the port is published).

### Documentation

- [ ] **DOCS-01**: The README has a "Run with Docker" section covering build, configure (`.env`), bring-up, viewing live logs, reaching metrics/health, and how the downstream spam layer connects.
- [ ] **DOCS-02**: An `.env.example` documents every required and optional environment variable with safe placeholder values.

## Future (beyond v1.1)

Tracked in PROJECT.md → Active (v2 candidates); not in this milestone:

- **FRESH-04**: Adaptive per-pubkey refresh intervals from observed churn.
- **RELAY-07**: NIP-77 negentropy bulk sync with supporting relays.
- **RELAY-08**: Streaming live kind-3 subscriptions for near-real-time updates.
- Operator validation of the daemon against real relays at scale.

## Out of Scope

Explicitly excluded. Documented to prevent scope creep.

| Feature | Reason |
|---------|--------|
| Registry/CI image publishing (ghcr.io pushes, build pipelines) | Single-operator, local build only — chosen for this milestone |
| Kubernetes / Helm / Swarm orchestration | docker-compose is sufficient for single-operator self-hosting |
| Multi-host / clustered deployment | Single anchor, single operator; revisit only if the spam layer demands it |
| Containerizing the downstream spam layer | Separate project; it connects to the shared DB independently |
| Changes to crawl behavior, schema, or relay logic | v1.1 is deployment/ops tooling only — v1.0 crawler is functionally complete |

## Traceability

Which phases cover which requirements. Updated during roadmap creation.

| Requirement | Phase | Status |
|-------------|-------|--------|
| IMAGE-01 | Phase 6 | Pending |
| IMAGE-02 | Phase 6 | Pending |
| IMAGE-03 | Phase 6 | Pending |
| COMPOSE-01 | Phase 7 | Pending |
| COMPOSE-02 | Phase 7 | Pending |
| COMPOSE-03 | Phase 7 | Pending |
| COMPOSE-04 | Phase 7 | Pending |
| COMPOSE-05 | Phase 7 | Pending |
| CONFIG-01 | Phase 7 | Pending |
| CONFIG-02 | Phase 7 | Pending |
| CONFIG-03 | Phase 7 | Pending |
| LOGS-01 | Phase 7 | Pending |
| LOGS-02 | Phase 7 | Pending |
| LOGS-03 | Phase 7 | Pending |
| DOCS-01 | Phase 7 | Pending |
| DOCS-02 | Phase 7 | Pending |

**Coverage:**
- v1.1 requirements: 16 total
- Mapped to phases: 16 ✓ (Phase 6: 3 · Phase 7: 13)
- Unmapped: 0

---
*Requirements defined: 2026-06-16 for milestone v1.1 Containerized Deployment*
*Traceability mapped: 2026-06-16 — Phases 6–7*
