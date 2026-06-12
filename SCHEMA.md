# Public Schema Contract

This document is the **public contract** between the Web-of-Trust crawler (this
project) and any downstream consumer — chiefly the separate spam-scoring layer.
The contract is the set of **stable SQL views** described below, not the base
tables. Consumers query the views; the crawler is free to change base-table
internals as long as these views keep their shape (D-01).

The contract is documented redundantly (D-02): this file plus `COMMENT ON`
statements in `migrations/0001_graph_schema.sql`, so the same contract is
introspectable from `psql` via `\d+ <view>` / `pg_description`.

> **Versioning is informal (D-04):** there is no version table and no versioned
> view names — a single operator owns both projects. Breaking changes are
> recorded in the [Contract changes](#contract-changes) changelog at the bottom
> of this file.

---

## How a consumer reads the graph

1. Read directed edges from **`follow_edges`** as bare surrogate-id pairs
   `(follower_id, followee_id)`. This is the hot path; do **not** join the
   `pubkeys` table here.
2. Run the trust/spam computation over those integer ids.
3. Only at the **boundary** of the computation, resolve the ids you care about
   to their 32-byte pubkeys via **`pubkey_lookup`** (D-03).
4. Weight knowledge by age using **`pubkey_freshness`** (`status`,
   `last_fetched_at`) — including `discovered`-but-unfetched pubkeys, so you see
   honest knowledge boundaries during the multi-day initial crawl (D-12).

### Connection contract (security, V4 / T-03-03)

The consumer should connect with a **read-only role** that has `SELECT` on the
three contract views **only** — never on the base tables. The crawler process
holds the read/write role. Example operator setup:

```sql
CREATE ROLE spam_layer LOGIN PASSWORD '...';
REVOKE ALL ON ALL TABLES IN SCHEMA public FROM spam_layer;
GRANT SELECT ON follow_edges, pubkey_lookup, pubkey_freshness TO spam_layer;
```

Enforcement is operator DB configuration; this document prescribes it.

---

## Contract views

### `follow_edges` — directed follow edges (hot path)

Bare directed edges as surrogate `bigint` ids. No pubkey join, no per-edge
metadata — freshness lives per-pubkey, not per-edge (D-05).

| Column | Type | Semantics |
|--------|--------|-----------|
| `follower_id` | `bigint` | Surrogate id of the *following* pubkey. Resolve via `pubkey_lookup`. |
| `followee_id` | `bigint` | Surrogate id of the *followed* pubkey. Resolve via `pubkey_lookup`. |

- An edge `(A, B)` means "A follows B". The relation is directed.
- **Self-follows are never present** — they are dropped at ingest in the store
  layer before the edge diff, and the base table also enforces
  `CHECK (follower_id <> followee_id)` (D-08).
- Edges are unique: `(follower_id, followee_id)` is the base-table primary key.

Example — who does pubkey id `42` follow:

```sql
SELECT followee_id FROM follow_edges WHERE follower_id = 42;
```

Example — in-degree of pubkey id `42` (how many follow it):

```sql
SELECT count(*) FROM follow_edges WHERE followee_id = 42;
```

### `pubkey_lookup` — id ↔ 32-byte pubkey resolution boundary

Resolves surrogate ids to the 32-byte x-only nostr pubkeys, only at the edges of
a computation (D-03).

| Column | Type | Semantics |
|--------|--------|-----------|
| `id` | `bigint` | Surrogate id used by `follow_edges` and `pubkey_freshness`. |
| `pubkey` | `bytea` | 32-byte x-only pubkey. |

Example — resolve a set of result ids to pubkeys:

```sql
SELECT id, pubkey FROM pubkey_lookup WHERE id = ANY($1);
```

### `pubkey_freshness` — per-pubkey knowledge freshness

Lets the consumer weight knowledge by age and see honest knowledge boundaries.
Exposes freshness; **hides** internal crawl bookkeeping (counters, applied-event
detail, churn) per D-11.

| Column | Type | Semantics |
|--------|--------|-----------|
| `id` | `bigint` | Surrogate id of the pubkey. |
| `status` | `text` | Knowledge status — see [status semantics](#status-semantics). |
| `last_fetched_at` | `timestamptz` | Time of the most recent fetch attempt; `NULL` until first fetched. Use to weight knowledge by age. |

Example — pubkeys whose follow list is older than a day:

```sql
SELECT id FROM pubkey_freshness
WHERE status = 'fetched' AND last_fetched_at < now() - interval '1 day';
```

Example — pubkeys discovered but not yet fetched (honest knowledge boundary):

```sql
SELECT id FROM pubkey_freshness WHERE status = 'discovered';
```

---

## Status semantics

`status` is stored as `text` with a `CHECK` constraint (not a native Postgres
`ENUM`) — see [the TEXT-vs-enum decision](#text-vs-enum-status-decision). Domain:

| Value | Meaning |
|-------|---------|
| `discovered` | The pubkey has been **seen** (it appears in someone's follow list) but its own follow list has **not yet been fetched**. This is the honest "we know it exists, we don't know who it follows yet" boundary during the initial crawl (D-12). |
| `fetched` | The pubkey's follow list has been fetched from relays and applied. `last_fetched_at` is set. |
| `not_found` | The pubkey was sought on relays but no kind-3 follow list was found. Candidate for NIP-65 fallback (Phase 5). |
| `failed` | A fetch was attempted but failed (transient relay/network error). Eligible for retry by the staleness scanner (Phase 4). |

A pubkey first appears as `discovered` (created when it is seen as a followee),
and transitions to `fetched` / `not_found` / `failed` as the crawler works it.

## The self-follow rule (D-08)

A kind-3 follow list that includes the author's own pubkey is **not** a
self-follow edge. Self-follows are dropped at ingest in the store layer (filtered
out of the followee set before the edge diff), so `follow_edges` never contains a
row where `follower_id = followee_id`. The base `follows` table additionally
enforces this with `CHECK (follower_id <> followee_id)`.

## Bare edges and id resolution at the boundary (D-03 / D-05)

Edges carry only `(follower_id, followee_id)` and nothing else. Pubkeys are
stored once and referenced by surrogate `bigint` id everywhere; the 32-byte
pubkey is never duplicated into the (hundreds-of-millions-of-rows) edge data.
Consumers traverse edges by id and resolve to pubkeys via `pubkey_lookup` only at
the boundary of their computation. This keeps the hot trust-walk path free of
pubkey joins.

## TEXT-vs-enum status decision

`status` is `text` + `CHECK (status IN ('discovered','fetched','not_found',
'failed'))` rather than a native Postgres `ENUM`. This avoids sqlx
enum-mapping pitfalls (a name/case mismatch between the Rust type and the
Postgres type fails at query time) and keeps the value set trivially extensible
without a `CREATE TYPE` migration. Consumers can treat `status` as a plain string
from the closed domain above (D-09 / RESEARCH Pitfall 5).

## Concurrency contract (GRAPH-03)

The crawler writes continuously while the consumer reads. Postgres MVCC
guarantees readers never block writers and vice versa under the default Read
Committed isolation — no application-level coordination is required. This is
proven by the automated `reader_and_writer_do_not_block` integration test
(`tests/concurrency.rs`), in which a writer task upserts edges while a reader on
a separate connection pool runs ~100 `follow_edges` queries, none blocking.

---

## Contract changes

Informal changelog of breaking or notable changes to the contract surface
(D-04). Newest first.

- **2026-06-12 — Initial contract (Phase 1).** Introduced the three contract
  views `follow_edges`, `pubkey_lookup`, `pubkey_freshness` over the `pubkeys`
  and `follows` base tables (migration `0001_graph_schema.sql`). Status domain:
  `discovered`, `fetched`, `not_found`, `failed`. Self-follows dropped at ingest.
  Edges are bare surrogate-id pairs. No changes yet.
