# Nostr Web-of-Trust Crawler & Data Layer

A continuously running crawler that turns nostr's scattered, adversarial kind-3
follow data into a locally held, always-current picture of the social graph.

Starting from a single trusted **anchor pubkey**, it discovers everyone reachable
through follows, fetches each follow list from public relays roughly once,
remembers how current that knowledge is, and re-checks relays only when the
knowledge ages out. The result is a directed follow graph persisted in
PostgreSQL — the foundation for a separate trust/spam-scoring layer that reads
the graph directly from the shared database.

## What it does

- **Crawls from one anchor.** BFS over kind-3 follow lists discovers the full
  reachable component (designed for low-millions of pubkeys / hundreds of
  millions of edges) — not hop-limited.
- **Fetches efficiently and politely.** Each follow list is fetched roughly
  once; per-relay GCRA rate limiting and capped-exponential backoff protect
  relay goodwill. Re-fetches happen only when knowledge goes stale (TTL).
- **Validates adversarial input.** Signature + id verification, cross-relay
  dedup, newest-wins replaceable-event resolution, future-timestamp clamping,
  and self-follow dropping before anything is written.
- **NIP-65 outbox fallback.** When the curated relay set misses a pubkey's
  follow list, the crawler falls back to that pubkey's advertised write relays,
  gated by a per-relay EWMA health score.
- **Runs unattended.** A daemon loop keeps the graph fresh, reclaims crashed
  leases, and exposes Prometheus metrics + health endpoints for observability.
- **Exposes a stable contract.** Downstream consumers read three SQL views, not
  the base tables — see [SCHEMA.md](SCHEMA.md).

## Architecture at a glance

| Concern | Where |
|---------|-------|
| Relay acquisition + validation | `src/relay`, `src/ingest` |
| Graph writer + BFS frontier | `src/store`, `src/crawl` |
| Daemon, staleness loop, observability | `src/daemon` |
| Binary entry point (`crawler`) | `src/main.rs` |
| Schema (the public contract) | `migrations/`, `SCHEMA.md` |

**Tech stack:** Rust (1.94+), `nostr-sdk` 0.44, `tokio`, `sqlx` 0.9 +
PostgreSQL, `governor` (rate limiting), `tracing` + `metrics` /
`metrics-exporter-prometheus` (observability), `axum` (metrics/health server).

## Prerequisites

- **Rust** 1.94+ (`rust-toolchain.toml` pins the toolchain)
- **PostgreSQL** 16 or 17, reachable via a connection URL
- Network access to your curated nostr relays

## Setup

1. **Create the database and run migrations.** Migrations live in `migrations/`
   and are applied automatically by the daemon on startup (`sqlx migrate`). To
   apply them manually:

   ```sh
   createdb web_of_trust
   # optional: install sqlx-cli to run migrations by hand
   cargo install sqlx-cli --no-default-features --features rustls,postgres
   DATABASE_URL=postgres://crawler:changeme@localhost:5432/web_of_trust sqlx migrate run
   ```

2. **Write a config file.** Copy the annotated example and edit it:

   ```sh
   cp config.example.toml config.toml
   ```

   At minimum set `anchor_pubkey`, the `relays` list, `database_url`, `ttl`, and
   `metrics_addr`. Every field is documented inline in
   [`config.example.toml`](config.example.toml).

3. **Build:**

   ```sh
   cargo build --release
   ```

## Usage

Run the daemon with a config file:

```sh
./target/release/crawler --config config.toml
```

The daemon **fails fast**: a missing/malformed config, a bad anchor pubkey, an
empty relay set, a non-URL `database_url`, or a non-positive TTL prints an
actionable error to stderr and exits non-zero *before* any DB connection or
relay traffic. The `database_url` is never logged.

### Configuration overrides

Any config value can be overridden by a `WOT__<FIELD>` environment variable
(double-underscore for nesting). Env vars win over the file — useful for secrets
in production:

```sh
WOT__DATABASE_URL='postgres://crawler:secret@db.internal:5432/web_of_trust' \
WOT__CONCURRENCY=16 \
WOT__LOG_FORMAT=json \
  ./target/release/crawler --config config.toml
```

Key knobs (see `config.example.toml` for the full set and defaults):

- `ttl` — staleness window; a follow list older than this is re-enqueued.
- `concurrency` / `batch_size` — in-flight fetch parallelism and authors per batch.
- `reqs_per_second` — sustained per-relay request rate (politeness).
- `nip65_fallback_enabled`, `relay_health_threshold`, `per_relay_concurrency`,
  `health_alpha` — NIP-65 outbox routing and per-relay health behavior.
- `log_level` / `log_format` (`human` or `json`).

### Observability

The daemon serves an HTTP endpoint at `metrics_addr` (default
`127.0.0.1:9100`):

- `GET /metrics` — Prometheus exposition (frontier depth, crawl coverage ratio,
  fetch duration, staleness age, relay failure counts, active relay count).
- `GET /health/live` — liveness probe.
- `GET /health/ready` — readiness probe.

> **Security:** binding `metrics_addr` to a public interface exposes internal
> crawl topology. Keep it on loopback/private network and scrape it privately.

## Run with Docker

A multi-stage [`Dockerfile`](Dockerfile) builds a minimal, non-root runtime
image from source — no live `DATABASE_URL` is needed at build time (the release
binary compiles against committed offline `sqlx` metadata via `SQLX_OFFLINE`):

```sh
docker build -t wot-crawler .
```

The result is a ~16 MB [distroless](https://github.com/GoogleContainerTools/distroless)
image: just the `crawler` binary on `gcr.io/distroless/cc-debian12:nonroot`,
running as uid `65532` with no shell or package manager, and no Rust/cargo
toolchain layers. `.dockerignore` keeps `target/`, local `config.*.toml`, and
`.env` out of the build context.

Run it by supplying a config file (mount it) and a reachable database. The
image's entrypoint is the binary itself, so flags and `WOT__*` env overrides
work exactly as they do for the native build:

```sh
docker run --rm \
  -v "$PWD/config.toml:/config.toml:ro" \
  -e WOT__DATABASE_URL='postgres://crawler:secret@db.internal:5432/web_of_trust' \
  -p 9100:9100 \
  wot-crawler --config /config.toml
```

The container has no shell — debug it via `docker logs` and the
`/metrics` + `/health/*` endpoints, not an interactive shell.

> A one-command Postgres + crawler **compose stack**, an `.env.example`, a
> container healthcheck, and fuller operator docs are the next milestone
> ([Phase 7](#roadmap--next-steps)) — the image above is the building block.

## Consuming the graph (downstream layer)

The downstream spam/trust layer reads PostgreSQL directly — there is no API or
library boundary. Read the three stable views:

- **`follow_edges`** — directed `(follower_id, followee_id)` surrogate-id pairs (hot path).
- **`pubkey_lookup`** — resolve surrogate ids to 32-byte pubkeys at the boundary.
- **`pubkey_freshness`** — `status` + `last_fetched_at` to weight knowledge by age.

Connect with a **read-only role** granted `SELECT` on those views only. Full
contract, semantics, and the recommended role setup are in
[SCHEMA.md](SCHEMA.md).

## Development

```sh
cargo test            # runs the suite (integration tests use testcontainers + Postgres)
cargo sqlx prepare -- --all-targets   # regenerate offline query metadata after schema/query changes
```

## Project status

**v1.0 — Crawler & data layer: complete.** Schema/contract (Phase 1), relay
acquisition + validation (Phase 2), transactional graph writer + crash-safe BFS
frontier (Phase 3), the unattended daemon with staleness loop + observability
(Phase 4), and NIP-65 outbox routing + relay health scoring (Phase 5).

**v1.1 — Containerized deployment: in progress.** Phase 6 (crawler image &
build context) is complete — see [Run with Docker](#run-with-docker). Phase 7
(compose stack & operator workflow) is the remaining work.

Full planning artifacts — roadmap, per-phase plans, summaries, and verification
reports — live under [`.planning/`](.planning/). Start at
[`.planning/ROADMAP.md`](.planning/ROADMAP.md) and
[`.planning/STATE.md`](.planning/STATE.md) to resume work.

## Roadmap & next steps

For an engineer picking this up:

**Next milestone — Phase 7: Compose Stack & Operator Workflow.** Build on the
Phase 6 image to deliver:

- A `docker-compose.yml` running a one-command Postgres + crawler stack, with
  the crawler waiting on a healthy database.
- Env-driven runtime config: an `.env.example` and `WOT__*` wiring so an
  operator configures the stack without editing files inside the image.
- A container `HEALTHCHECK` hitting `/health/ready`, and published metrics/health
  ports for private scraping.
- A "Run with Docker" / operator runbook expanding the section above.
- Preserve the DB-as-contract boundary: the downstream layer connects to the
  same Postgres with a read-only role (see [SCHEMA.md](SCHEMA.md)).

**Verify the image build once on a healthy network.** The Phase 6 image was
verified end-to-end, but a plain `docker build .` should be confirmed on a
machine with normal `crates.io` / `static.rust-lang.org` access (some build
environments throttle the in-build network and stall the toolchain download).

**Hardening backlog (from code review, optional).**

- Pin the build tool: `cargo install cargo-chef --version <x> --locked`.
- Add `--locked` to `cargo chef cook --release` and `cargo build --release` in
  the `Dockerfile` so a `Cargo.toml`/`Cargo.lock` drift fails the build loudly.

**Beyond v1.1.** The graph this crawler maintains is the foundation for a
separate trust/spam-scoring layer that propagates trust over the follow graph by
reading the shared database directly.
