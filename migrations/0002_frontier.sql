-- Phase 3 frontier support: turn pubkeys.status into a crash-safe, DB-resident
-- BFS frontier (CRAWL-01..04, FRESH-01 / CONTEXT D-12).
--
-- This migration is idempotent and strictly ADDITIVE: re-running it against an
-- already-migrated database is a no-op. The status CHECK is dropped and
-- re-created under an explicit name (a CHECK constraint cannot be edited in
-- place — RESEARCH Pitfall 1), guarded by DROP CONSTRAINT IF EXISTS; the new
-- bookkeeping columns use ADD COLUMN IF NOT EXISTS; the contract view is
-- CREATE OR REPLACE. sqlx also wraps each migration in a transaction by default.
--
-- Scope (D-12): frontier lifecycle only — widen the status domain with the
-- transient 'in_progress' lease state, add the two internal lease/retry
-- bookkeeping columns (claimed_at, fetch_attempts), and keep 'in_progress' OUT
-- of the public pubkey_freshness contract by collapsing it to 'discovered'.
-- The follows table and every other Phase 1 object are untouched.
--
-- The Phase 1 partial index pubkeys_status_idx already covers the
-- WHERE status='discovered' claim scan; no 'in_progress' index is added — the
-- startup reclaim is a one-time scan, not a hot path (RESEARCH A3).

-- 1. Widen the status CHECK domain to include the transient lease state
--    'in_progress'. The Phase 1 CHECK was inline + auto-named
--    'pubkeys_status_check' (verified via pg_constraint against a 0001-migrated
--    DB, RESEARCH A1). Re-create it explicitly named so re-runs are clean.
ALTER TABLE pubkeys DROP CONSTRAINT IF EXISTS pubkeys_status_check;
ALTER TABLE pubkeys ADD CONSTRAINT pubkeys_status_check
    CHECK (status IN ('discovered','in_progress','fetched','not_found','failed'));

-- 2. Internal lease / retry bookkeeping columns (D-12). Not part of the public
--    contract; absent from every contract view.
ALTER TABLE pubkeys ADD COLUMN IF NOT EXISTS claimed_at     TIMESTAMPTZ;
ALTER TABLE pubkeys ADD COLUMN IF NOT EXISTS fetch_attempts SMALLINT NOT NULL DEFAULT 0;

-- 3. Collapse the transient lease state out of the public contract (D-12, Open
--    Question 2 — hide it). The documented contract domain stays
--    {discovered, fetched, not_found, failed}: a pubkey mid-lease ('in_progress')
--    is, from the consumer's perspective, still seen-but-not-yet-fetched, so it
--    reports as 'discovered'. The internal lease/retry columns are deliberately
--    NOT selected here.
CREATE OR REPLACE VIEW pubkey_freshness AS
    SELECT
        id,
        CASE WHEN status = 'in_progress' THEN 'discovered' ELSE status END AS status,
        last_fetched_at
    FROM pubkeys;

-- 4. Re-document the contract + label the new internal columns. The view
--    redefinition above clears the prior COMMENT ON VIEW, so re-issue the
--    PUBLIC CONTRACT comment to keep the four-value domain truthful.
COMMENT ON VIEW pubkey_freshness IS
    'PUBLIC CONTRACT: per-pubkey knowledge freshness. status=discovered means seen-but-not-yet-fetched (an honest knowledge boundary during the multi-day initial crawl; the transient in_progress lease state is intentionally collapsed to discovered here); last_fetched_at lets the consumer weight knowledge by age. Internal crawl counters and failure detail are intentionally hidden.';
COMMENT ON COLUMN pubkey_freshness.status IS
    'PUBLIC CONTRACT: knowledge status — one of discovered, fetched, not_found, failed.';

COMMENT ON COLUMN pubkeys.claimed_at IS
    'INTERNAL: timestamp this pubkey was leased into the in_progress frontier state; NULL when not leased. Used by the startup reclaim to detect orphaned leases (CRAWL-03 / D-12).';
COMMENT ON COLUMN pubkeys.fetch_attempts IS
    'INTERNAL: number of transient fetch attempts so far; bumped on requeue and compared against the retry cap before a pubkey is marked failed (D-09 / D-12).';
