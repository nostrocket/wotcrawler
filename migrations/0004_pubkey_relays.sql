-- Phase 5: NIP-65 (kind:10002) advertised relay storage (RELAY-05).
--
-- This migration is idempotent and strictly ADDITIVE: re-running it against an
-- already-migrated database is a no-op. It uses CREATE TABLE / CREATE INDEX
-- IF NOT EXISTS and an explicitly-named CHECK constraint (mirroring the 0002
-- named-CHECK convention so a re-run is clean); sqlx also wraps each migration
-- in a transaction by default.
--
-- Scope: a single new internal routing-bookkeeping table. Per-pubkey NIP-65
-- write/read relays are persisted newest-wins (a fresh winning kind:10002
-- replaces that pubkey's prior relay rows wholesale — see
-- store::relays::apply_relay_list). The pubkeys/follows tables and every other
-- Phase 1/3/4 object are untouched.
--
-- NOT part of the public contract: pubkey_relays is internal outbox-routing
-- state consumed by the crawler's fallback path, NOT by the spam layer. It is
-- deliberately absent from the pubkey_freshness contract view (GRAPH-04) and
-- from every other contract view.

CREATE TABLE IF NOT EXISTS pubkey_relays (
    pubkey_id BIGINT      NOT NULL REFERENCES pubkeys(id),
    url       TEXT        NOT NULL,
    marker    TEXT        NOT NULL
              CONSTRAINT pubkey_relays_marker_check
              CHECK (marker IN ('read','write','both')),
    seen_at   TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (pubkey_id, url)
);

-- Per-pubkey lookup for the fallback path ("write relays for author X").
CREATE INDEX IF NOT EXISTS pubkey_relays_pubkey_idx ON pubkey_relays (pubkey_id);

COMMENT ON TABLE pubkey_relays IS
    'INTERNAL: per-pubkey NIP-65 (kind:10002) advertised relays for outbox-style fallback routing (RELAY-05). Newest-wins replaced per pubkey on each winning kind:10002. NOT part of the public contract.';
