# Phase 1: Schema & Data Contract - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-06-11
**Phase:** 1-Schema & Data Contract
**Areas discussed:** Consumer contract surface, Edge metadata richness, Freshness model shape, Migration scope strategy

---

## Consumer contract surface

| Option | Description | Selected |
|--------|-------------|----------|
| Base tables directly | Schema is the contract; internals and contract evolve together via migrations | |
| Stable views over tables | Spam layer reads views; internals can change without breaking the consumer | ✓ |
| Hybrid: tables + convenience views | Contract is base tables plus documented views for common queries | |

**User's choice:** Stable views over tables

| Option | Description | Selected |
|--------|-------------|----------|
| SCHEMA.md + SQL comments | Committed prose doc plus COMMENT ON in migrations | ✓ |
| SCHEMA.md only | Single prose document | |
| SQL COMMENT ON only | Database documents itself | |

**User's choice:** SCHEMA.md + SQL comments (recommended)

| Option | Description | Selected |
|--------|-------------|----------|
| Both ids and raw pubkeys | Views expose bigint id and 32-byte pubkey | |
| Bigint ids only | Lean edge views; separate id→pubkey lookup view | ✓ |
| Raw pubkeys only | Consumer never sees internal ids; expensive join per edge query | |

**User's choice:** Bigint ids only

| Option | Description | Selected |
|--------|-------------|----------|
| Informal — changelog in SCHEMA.md | "Contract changes" section; no machinery | ✓ |
| Schema version table | contract_version row checked by consumer at startup | |
| Versioned view names | v1/v2 views side by side during migration | |

**User's choice:** Informal — changelog in SCHEMA.md (recommended)

---

## Edge metadata richness

| Option | Description | Selected |
|--------|-------------|----------|
| Bare edges | Just two bigint ids; freshness lives per-pubkey | ✓ |
| Edges + first/last seen | Per-edge timestamps for churn analysis | |
| Edges + full provenance | Timestamps plus source event id per edge | |

**User's choice:** Bare edges (recommended)

| Option | Description | Selected |
|--------|-------------|----------|
| Discard both | Structure only; relay discovery is NIP-65's job | ✓ |
| Keep relay hints, drop petnames | Hints could supplement Phase 5 fallback | |
| Keep both | Full kind-3 fidelity | |

**User's choice:** Discard both (recommended)

| Option | Description | Selected |
|--------|-------------|----------|
| Keep event id + created_at only | Per-pubkey columns; enough for newest-wins and idempotency | ✓ |
| Derived edges only | No event identity at all | |
| Keep full raw event JSON | Newest raw kind-3 per pubkey | |

**User's choice:** Keep event id + created_at only (recommended)

| Option | Description | Selected |
|--------|-------------|----------|
| Drop at ingest | Self-edges carry no trust information; filter in store layer | ✓ |
| Store as-is | Maximum fidelity; consumer filters | |

**User's choice:** Drop at ingest (recommended)

---

## Freshness model shape

| Option | Description | Selected |
|--------|-------------|----------|
| Status enum + timestamps | discovered/fetched/not_found/failed + last_fetched_at + last_confirmed_at | ✓ |
| Timestamps only, status derived | Nullable timestamps; lifecycle states harder to distinguish | |
| Separate fetch_state table | Pubkeys pure identity; crawl state in 1:1 side table | |

**User's choice:** Status enum + timestamps (recommended)

| Option | Description | Selected |
|--------|-------------|----------|
| Yes, include now | last_changed_at, fetch_count, change_count in Phase 1 | ✓ |
| No, add in Phase 4 | Additive ALTER TABLE later | |

**User's choice:** Yes, include now (recommended) — FRESH-03 churn columns from day one

| Option | Description | Selected |
|--------|-------------|----------|
| Expose freshness, hide crawl state | status + last_fetched_at public; counters internal | ✓ |
| Internal only | Views expose pure graph structure | |
| Expose everything | All bookkeeping in views | |

**User's choice:** Expose freshness, hide crawl state (recommended)

| Option | Description | Selected |
|--------|-------------|----------|
| Visible with status | All known pubkeys in views; status marks knowledge boundary | ✓ |
| Only fetched pubkeys visible | Views filter to acquired follow lists | |

**User's choice:** Visible with status (recommended)

---

## Migration scope strategy

| Option | Description | Selected |
|--------|-------------|----------|
| Graph + freshness only | Frontier/relay/NIP-65 tables arrive additively in their phases | ✓ |
| Full schema up front | Every table for all 5 phases in initial migrations | |
| Graph + skeleton stubs | Placeholder tables reserve names; columns later | |

**User's choice:** Graph + freshness only (recommended)

| Option | Description | Selected |
|--------|-------------|----------|
| Automated concurrency test | Writer task + reader connection simultaneously in an integration test | ✓ |
| Manual psql demonstration | Documented manual procedure | |
| You decide | Planner picks | |

**User's choice:** Automated concurrency test (recommended)

| Option | Description | Selected |
|--------|-------------|----------|
| Minimal plumbing + basic ops | Pool, migrations, simple upserts; edge-diff writer in Phase 3 | |
| Plumbing only | Pool + migrations, no data operations | |
| Full write API now | Edge-diff writer built in Phase 1 | ✓ |

**User's choice:** Full write API now — user deliberately pulled the writer forward while the schema is fresh

| Option | Description | Selected |
|--------|-------------|----------|
| Build now, verify in Phase 3 | Phase 1 ships the API; GRAPH-02 verification stays in Phase 3 | ✓ |
| Move GRAPH-02 into Phase 1 | Formal roadmap re-mapping | |

**User's choice:** Build now, verify in Phase 3 (recommended)

---

## Claude's Discretion

- Index strategy (forward/reverse edge indexes, partial indexes on status/staleness)
- Exact Postgres column types and migration file naming
- Test database setup (testcontainers vs docker-compose vs local Postgres)
- Connection pool sizing

## Deferred Ideas

None — discussion stayed within phase scope.
