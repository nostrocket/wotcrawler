---
phase: 05-nip-65-outbox-routing-relay-health
plan: 01
subsystem: database
tags: [nip65, kind10002, postgres, sqlx, relay-routing, testcontainers, scriptedgraph]

# Dependency graph
requires:
  - phase: 02-ingest-validation
    provides: ValidatedFollowList + timestamp_to_datetime + pick_winner (kind-agnostic replaceable resolver, reused unchanged for kind:10002)
  - phase: 01-graph-store
    provides: PgPool wiring, run_migrations, upsert_pubkey surrogate-id pattern, apply_follow_list transactional newest-wins analog
  - phase: 04-relay-health-staleness
    provides: additive/idempotent migration conventions (0002/0003), .sqlx --all-targets regen lesson, RelayError variants
provides:
  - "migration 0004 pubkey_relays (internal, NOT-contract NIP-65 relay storage)"
  - "ValidatedRelayList type + ingest::relay_list::extract_relay_pairs/from_event (nip65 r-tag extraction)"
  - "store::relays::apply_relay_list (transactional newest-wins full replace) + lookup_write_relays (write+both)"
  - "relay-URL-aware + error-injecting ScriptedGraph seam (with_relay/fail_relay/relay_fetch_fn)"
  - "relay_list_event kind:10002 fixture"
  - "named-ignored tests/nip65_fallback.rs Wave 0 scaffold"
affects: [05-02-relay-health-registry, 05-03-nip65-fallback, 05-04-health-routing-concurrency]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Companion validated-type + transform module (ValidatedRelayList mirrors ValidatedFollowList; relay_list.rs sibling of follow_list.rs)"
    - "Transactional newest-wins FULL REPLACE (DELETE-all + per-row INSERT ON CONFLICT DO NOTHING in one tx) for tiny per-pubkey tables"
    - "Relay-URL-aware + error-injecting ScriptedGraph (Arc-backed Send+Clone), back-compat union path preserved"

key-files:
  created:
    - migrations/0004_pubkey_relays.sql
    - src/ingest/relay_list.rs
    - src/store/relays.rs
    - tests/relay_lists_store.rs
    - tests/nip65_fallback.rs
  modified:
    - src/ingest/mod.rs
    - src/store/mod.rs
    - tests/common/mod.rs
    - tests/relay_list.rs
    - .sqlx/

key-decisions:
  - "pubkey_relays is INTERNAL routing bookkeeping, deliberately absent from every contract view (GRAPH-04)"
  - "Bare NIP-65 r-tag maps to marker 'both'; lookup_write_relays selects marker IN ('write','both') so bare-r-tag write relays are recovered (Pitfall 2)"
  - "Full-replace (not edge-diff) for apply_relay_list — relay lists are a handful of rows (RESEARCH A2)"
  - "nip65 module is reached via nostr_sdk::nips::nip65 (not nostr_sdk::nip65 — the latter is not re-exported)"

patterns-established:
  - "Pattern: transform module re-reads the winning event's r-tags via the built-in nip65 helper, never hand-rolling tag parsing"
  - "Pattern: ScriptedGraph.with_relay/fail_relay model per-relay event placement + injected RelayError for fallback/health tests"

requirements-completed: [RELAY-05]

# Metrics
duration: ~50min
completed: 2026-06-15
---

# Phase 5 Plan 01: NIP-65 Relay Storage & Extraction Foundation Summary

**Migration 0004 `pubkey_relays` + `ValidatedRelayList`/`extract_relay_pairs` (built-in nip65 r-tag extraction) + transactional `apply_relay_list`/`lookup_write_relays`, plus a relay-URL-aware/error-injecting `ScriptedGraph` seam and the named-ignored fallback scaffold every downstream Phase 5 plan depends on.**

## Performance

- **Duration:** ~50 min
- **Started:** 2026-06-15T09:44:41Z
- **Completed:** 2026-06-15T~10:35Z
- **Tasks:** 4
- **Files modified:** 10 (5 created, 5 modified incl. .sqlx)

## Accomplishments
- Migration 0004 brings a fresh DB to the `pubkey_relays` schema (additive/idempotent, named CHECK, PK `(pubkey_id, url)`, per-pubkey index, INTERNAL comment, absent from all contract views) — RELAY-05 storage prerequisite.
- `ValidatedRelayList` + `ingest::relay_list::{extract_relay_pairs, from_event}` extract a winning kind:10002 event's r-tags via the built-in `nostr nip65::extract_relay_list` helper (bare→both, read, write; url normalized) — no hand-rolled parsing.
- `store::relays::apply_relay_list` persists a pubkey's relays as a transactional newest-wins full replace; `lookup_write_relays` returns `marker IN ('write','both')`.
- Relay-URL-aware + error-injecting `ScriptedGraph` (`with_relay`, `fail_relay`, `relay_fetch`/`relay_fetch_fn`) + `relay_list_event` kind:10002 fixture, with the original author-keyed `union_for`/`fetch_fn` back-compat path preserved.
- `tests/relay_lists_store.rs` (3 green testcontainers tests) + named-ignored `tests/nip65_fallback.rs` Wave 0 scaffold; extended `tests/relay_list.rs` with 3 green offline extraction asserts.

## Task Commits

Each task was committed atomically:

1. **Task 1: Migration 0004 pubkey_relays** - `a564f20` (feat)
2. **Task 2: ValidatedRelayList + r-tag extraction** - `70a320e` (feat)
3. **Task 3: store::relays apply_relay_list + lookup_write_relays + .sqlx** - `812bde2` (feat)
4. **Task 4: Wave 0 test seams (ScriptedGraph, fixtures, store tests, scaffold)** - `60de0a7` (test)

**Plan metadata:** _(this docs commit)_

## Files Created/Modified
- `migrations/0004_pubkey_relays.sql` - Internal `pubkey_relays` table (pubkey_id, url, marker CHECK, seen_at) + per-pubkey index; additive/idempotent; not a contract view.
- `src/ingest/mod.rs` - Added `ValidatedRelayList` companion type; registered `pub mod relay_list`.
- `src/ingest/relay_list.rs` - `extract_relay_pairs`/`from_event` via `nostr_sdk::nips::nip65::extract_relay_list`; `marker_of` (None→both).
- `src/store/mod.rs` - Registered `pub mod relays`.
- `src/store/relays.rs` - `apply_relay_list` (txn DELETE-all + INSERT ON CONFLICT DO NOTHING) + `lookup_write_relays` (write+both).
- `tests/common/mod.rs` - URL-aware/error-injecting `ScriptedGraph` (`with_relay`/`fail_relay`/`relay_fetch`/`relay_fetch_fn`, `RelayFailure`, `RelayPlacements` alias) + `relay_list_event` fixture.
- `tests/relay_list.rs` - 3 offline extraction asserts (bare/read/write markers, trailing-slash normalization, from_event round-trip).
- `tests/relay_lists_store.rs` - `migration_0004_idempotent`, `apply_relay_list_newest_wins_replace`, `lookup_write_relays_write_and_both` (testcontainers).
- `tests/nip65_fallback.rs` - Named-ignored scaffold: `fallback_recovers_via_write_relay`, `fallback_miss_stamps_not_found`, `no_deadlock_single_permit`.
- `.sqlx/` - Regenerated offline metadata for the 3 new `pubkey_relays` queries (`cargo sqlx prepare -- --all-targets`).

## Decisions Made
- `pubkey_relays` is INTERNAL routing state, not part of the public contract (consistent with RESEARCH A3 / Pattern 1); grep gate confirms absence from all `CREATE VIEW`s and the runtime test confirms it is not in `pubkey_freshness`.
- Bare r-tag → `'both'`; `lookup_write_relays` includes `'both'` so bare-r-tag write relays fire the fallback (Pitfall 2).
- Full-replace rather than edge-diff for the tiny per-pubkey relay table (RESEARCH A2).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Corrected the nip65 import path**
- **Found during:** Task 2 (ValidatedRelayList + extraction)
- **Issue:** The plan/research wrote `use nostr_sdk::nip65::{self, RelayMetadata}`, but `nip65` is not re-exported at `nostr_sdk::nip65`; the offline build failed with `no nip65 in the root` (and a cascading E0282 type-inference error in the `.map` closure).
- **Fix:** Imported via the actual re-export path `nostr_sdk::nips::nip65` (and `nostr_sdk::nips::nip65::RelayMetadata`), confirmed against the installed `nostr-0.44.3` / `nostr-sdk-0.44.1` sources (`nostr_sdk` does `pub use nostr::{self, *}`; the module lives at `nostr::nips::nip65`). The closure type error resolved once the import was correct.
- **Files modified:** src/ingest/relay_list.rs, tests/common/mod.rs (fixture import)
- **Verification:** `SQLX_OFFLINE=true cargo build` green; grep gate confirms `nip65::extract_relay_list` used and 0 hand-rolled `as_vec`/`TagKind::single_letter`.
- **Committed in:** 70a320e (Task 2 commit)

**2. [Rule 1 - Lint] Factored a complex test type behind an alias**
- **Found during:** Task 4 (ScriptedGraph extension)
- **Issue:** The new `by_relay: Arc<HashMap<(String, Vec<u8>), Vec<Event>>>` field tripped clippy `type_complexity` (a warning my change introduced).
- **Fix:** Introduced a `type RelayPlacements = HashMap<(String, Vec<u8>), Vec<Event>>` alias and used it for the field + `with_relay` local.
- **Files modified:** tests/common/mod.rs
- **Verification:** `cargo clippy --tests` no longer warns on common/mod.rs; the two test binaries remain green.
- **Committed in:** 60de0a7 (amended into the Task 4 commit)

---

**Total deviations:** 2 auto-fixed (1 blocking import correction, 1 self-introduced lint cleanup)
**Impact on plan:** Both were necessary to land a clean offline build / lint-clean test seam. No scope creep — the API surface and behaviors are exactly as planned. The corrected import path is recorded as a key decision so 05-02/03/04 use `nostr_sdk::nips::nip65`.

## Issues Encountered
- testcontainers DB tests ran clean on the first attempt at `--test-threads=2` (no container/port flake re-run needed).
- `.sqlx` regeneration required a live Postgres: spun up an ephemeral `postgres:17` container on port 55433, ran `cargo sqlx migrate run` + `cargo sqlx prepare -- --all-targets`, then removed the container. The integration test file uses runtime `sqlx::query*(...)`/`sqlx::migrate!`, so it added no offline metadata; only the 3 lib queries in `store::relays` produced `.sqlx` entries.

## TDD Gate Compliance
Task 4 was marked `tdd="true"`, but the production seam it tests (extraction, store fns, fixture) was deliberately landed in the preceding Tasks 2–3 commits per the plan's task split (file-ownership disjointness within the wave). Consequently the Task 4 tests went green on first run rather than via a separate RED→GREEN commit pair within Task 4. The behaviors are nonetheless test-verified (3 offline extraction asserts + 3 testcontainers store/migration tests all green), and the Wave 0 fallback bodies are intentionally `#[ignore]`d for 05-03/05-04. No standalone failing-test (RED) commit exists for Task 4.

## User Setup Required
None - no external service configuration required. New `pubkey_relays` table is applied automatically by `run_migrations`; it starts empty and is populated by live observation going forward.

## Next Phase Readiness
- 05-02 (RelayHealthRegistry) can build the parallel registry; the `RelayFailure::Timeout`/`NotFound` injection seam is ready to exercise the health capture sites.
- 05-03 (fallback) has `lookup_write_relays` + `ScriptedGraph::with_relay`/`relay_fetch_fn` + the `relay_list_event` fixture + the named-ignored `nip65_fallback.rs` bodies to fill in.
- 05-04 (health routing/concurrency) has the `no_deadlock_single_permit` scaffold and the error-injecting relay seam.
- No blockers.

---
*Phase: 05-nip-65-outbox-routing-relay-health*
*Completed: 2026-06-15*

## Self-Check: PASSED
- All 5 created files + SUMMARY.md present on disk.
- All 4 task commits (a564f20, 70a320e, 812bde2, 60de0a7) present in git history.
