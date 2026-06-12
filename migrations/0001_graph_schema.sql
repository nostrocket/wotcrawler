-- Phase 1 graph + freshness schema (GRAPH-01, GRAPH-04 / CONTEXT D-13).
--
-- This migration is idempotent: re-running it against an already-migrated
-- database is a no-op (CREATE TABLE/INDEX ... IF NOT EXISTS, CREATE OR REPLACE
-- VIEW). sqlx also wraps each Postgres migration in a transaction by default;
-- the IF NOT EXISTS guards are the additional safety net (RESEARCH Pattern 1,
-- Pitfall 3).
--
-- Scope (D-13): graph (pubkeys, follows) + per-pubkey freshness/churn only.
-- Frontier (Phase 3), relay registry/health (Phase 5), and kind:10002 storage
-- (Phase 2) arrive as additive migrations in their own phases.
--
-- status is stored as TEXT + CHECK rather than a native Postgres ENUM to avoid
-- sqlx enum-mapping pitfalls and keep the value set trivially extensible
-- (RESEARCH Pitfall 5 / D-09).

CREATE TABLE IF NOT EXISTS pubkeys (
    id                 BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    pubkey             BYTEA NOT NULL UNIQUE,                       -- 32-byte x-only key
    status             TEXT  NOT NULL DEFAULT 'discovered'
                       CHECK (status IN ('discovered','fetched','not_found','failed')),
    last_fetched_at    TIMESTAMPTZ,                                 -- FRESH-01 (D-09)
    last_confirmed_at  TIMESTAMPTZ,                                 -- FRESH-01 (D-09)
    last_changed_at    TIMESTAMPTZ,                                 -- FRESH-03 (D-10)
    fetch_count        BIGINT NOT NULL DEFAULT 0,                   -- FRESH-03 (D-10)
    change_count       BIGINT NOT NULL DEFAULT 0,                   -- FRESH-03 (D-10)
    applied_event_id   BYTEA,                                       -- INGEST-03 newest-wins (D-07)
    applied_created_at TIMESTAMPTZ                                  -- INGEST-03 (D-07)
);

CREATE TABLE IF NOT EXISTS follows (
    follower_id BIGINT NOT NULL REFERENCES pubkeys(id),
    followee_id BIGINT NOT NULL REFERENCES pubkeys(id),
    PRIMARY KEY (follower_id, followee_id),
    CHECK (follower_id <> followee_id)                              -- self-follow guard (D-08)
);

-- Reverse / in-degree lookups ("who follows X"); forward lookups ("who does X
-- follow") are covered by the PK's leading column (follower_id).
CREATE INDEX IF NOT EXISTS follows_followee_idx ON follows (followee_id);

-- Partial index for the Phase 4 staleness scanner: only the not-yet-current
-- statuses are scanned for re-fetch candidates.
CREATE INDEX IF NOT EXISTS pubkeys_status_idx ON pubkeys (status)
    WHERE status IN ('discovered','not_found','failed');

-- Contract views: the public API for the downstream spam layer (GRAPH-04 /
-- D-01, D-03, D-11, D-12). Consumers read these views, never the base tables.
CREATE OR REPLACE VIEW follow_edges AS
    SELECT follower_id, followee_id FROM follows;                  -- bare ids, hot path (D-05)

CREATE OR REPLACE VIEW pubkey_lookup AS
    SELECT id, pubkey FROM pubkeys;                                -- id -> 32-byte boundary (D-03)

CREATE OR REPLACE VIEW pubkey_freshness AS
    SELECT id, status, last_fetched_at FROM pubkeys;               -- honest aging (D-11, D-12)

-- Introspectable contract (D-02): COMMENT ON every contract view and contract
-- column, labelled PUBLIC CONTRACT so `\d+` / pg_description / obj_description
-- surface the contract from psql. Internal bookkeeping columns are labelled
-- INTERNAL and are deliberately absent from the contract views.
COMMENT ON VIEW follow_edges IS
    'PUBLIC CONTRACT: directed follow edges as (follower_id, followee_id) surrogate bigint ids. Resolve ids to 32-byte pubkeys via pubkey_lookup only at the boundary of the computation; never join pubkeys on this hot path.';
COMMENT ON VIEW pubkey_lookup IS
    'PUBLIC CONTRACT: maps surrogate bigint id -> 32-byte x-only pubkey (bytea). Use only to resolve ids at the edges of a computation.';
COMMENT ON VIEW pubkey_freshness IS
    'PUBLIC CONTRACT: per-pubkey knowledge freshness. status=discovered means seen-but-not-yet-fetched (an honest knowledge boundary during the multi-day initial crawl); last_fetched_at lets the consumer weight knowledge by age. Internal crawl counters and failure detail are intentionally hidden.';

COMMENT ON COLUMN follow_edges.follower_id IS
    'PUBLIC CONTRACT: surrogate id of the following pubkey (resolve via pubkey_lookup).';
COMMENT ON COLUMN follow_edges.followee_id IS
    'PUBLIC CONTRACT: surrogate id of the followed pubkey (resolve via pubkey_lookup).';
COMMENT ON COLUMN pubkey_lookup.id IS
    'PUBLIC CONTRACT: surrogate bigint id for the pubkey.';
COMMENT ON COLUMN pubkey_lookup.pubkey IS
    'PUBLIC CONTRACT: 32-byte x-only pubkey (bytea).';
COMMENT ON COLUMN pubkey_freshness.id IS
    'PUBLIC CONTRACT: surrogate bigint id for the pubkey.';
COMMENT ON COLUMN pubkey_freshness.status IS
    'PUBLIC CONTRACT: knowledge status — one of discovered, fetched, not_found, failed.';
COMMENT ON COLUMN pubkey_freshness.last_fetched_at IS
    'PUBLIC CONTRACT: timestamp of the most recent fetch attempt for this pubkey; NULL until first fetched. Use to weight knowledge by age.';

-- Internal bookkeeping (NOT part of the contract; hidden from contract views per D-11).
COMMENT ON COLUMN pubkeys.applied_event_id IS
    'INTERNAL: id of the kind-3 event currently applied for this pubkey; used for newest-wins resolution and the same-event-id idempotency check (GRAPH-02).';
COMMENT ON COLUMN pubkeys.applied_created_at IS
    'INTERNAL: created_at of the currently applied kind-3 event; used for newest-wins resolution (INGEST-03).';
COMMENT ON COLUMN pubkeys.last_changed_at IS
    'INTERNAL: timestamp the follow list last actually changed (FRESH-03 churn).';
COMMENT ON COLUMN pubkeys.fetch_count IS
    'INTERNAL: number of times this pubkey has been fetched (FRESH-03 churn).';
COMMENT ON COLUMN pubkeys.change_count IS
    'INTERNAL: number of times the follow list actually changed across fetches (FRESH-03 churn).';
