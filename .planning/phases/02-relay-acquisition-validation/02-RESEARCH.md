# Phase 2: Relay Acquisition & Validation - Research

**Researched:** 2026-06-12
**Domain:** Nostr relay acquisition (nostr-sdk 0.44 relay pool, NIP-11, pagination), event validation (secp256k1 signature verification, NIP-01 replaceable-event semantics, NIP-02 kind-3 / NIP-65 kind:10002), per-relay politeness/rate-limiting
**Confidence:** HIGH (stack + event/filter API verified against docs.rs; relay reconnect options verified; NIP semantics HIGH from prior project research)

## Summary

Phase 2 is the "acquisition half" of the crawler: it must pull kind-3 follow lists and kind:10002 relay-list events from a configurable curated relay set **politely and completely**, then run every fetched event through a validation gate so that **only correct, deduplicated, newest-wins** follow lists emerge. It does NOT write the graph (that is Phase 3's `apply_follow_list`, already built in Phase 1) and does NOT do BFS/frontier or NIP-65 fallback routing (Phase 3/5). The output contract of this phase is a *validated, resolved follow-list value* (follower pubkey + applied event id + created_at + the deduped set of followee pubkeys) ready to hand to the Phase 1 store writer.

The entire stack is locked by CLAUDE.md and confirmed current: **nostr-sdk 0.44.1** (umbrella crate bundling `nostr` 0.44.3, `nostr-relay-pool` 0.44.1), **tokio 1.52**, **governor 0.10.4** for per-relay rate limiting. nostr-sdk gives you four of the five requirement areas nearly for free: secp256k1 signature verification (`Event::verify`), relay-pool connection management with **auto-reconnect on by default** (`RelayOptions::reconnect=true`, `retry_interval=10s`, `adjust_retry_interval=true`), cross-relay deduplication (`fetch_events` returns a deduping `Events` set), and p-tag extraction (`Tags::public_keys()`). The genuinely custom logic Phase 2 must own is: (a) the **pagination loop** that defeats the EOSE-completeness trap, (b) the **replaceable-event resolution** (future-dated clamp + newest-wins + same-timestamp lowest-id tie-break), and (c) the **bounds/validation guards** (p-tag malformity, oversized-list cap, request-rate politeness layered on top of nostr-sdk).

**Primary recommendation:** Build a `relay` module (pool wiring, NIP-11 limit cache, per-relay governor rate limiter, paginated author-chunked fetch) and a `validation`/`ingest` module (signature verification gate → kind/author match → replaceable-event resolution → p-tag extraction & bounds → emit a `ValidatedFollowList` value). Use nostr-sdk's `Client` with explicit `RelayOptions` for reconnect; do NOT rely on `fetch_events`'s auto-close-on-EOSE as proof of completeness — wrap it in an explicit `until`-windowed, author-chunked pagination loop keyed off each relay's NIP-11 `max_limit`.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Relay websocket lifecycle / reconnect / backoff | nostr-sdk relay pool (transport) | App config (RelayOptions) | nostr-sdk owns sockets; the app only sets policy (reconnect, retry interval) and supplies the relay set. |
| NIP-11 limit discovery + caching | App (relay module) | nostr-sdk (RelayInformationDocument) | nostr-sdk parses the doc; the app caches per-relay limits and feeds them to the pagination planner. |
| Pagination / completeness (until-windows, author chunking) | App (relay module) | nostr-sdk (`fetch_events`/`stream_events`) | EOSE-completeness is a protocol pitfall nostr-sdk does not solve; the app must loop. |
| Per-relay rate limiting / politeness | App (governor) | nostr-sdk (CLOSED/NOTICE handling) | governor enforces the token bucket; the app reacts to rate-limited notices with backoff. |
| Signature / id verification | nostr-sdk (`Event::verify`) | App (gate enforcement) | Crypto is nostr-sdk's job (never hand-roll secp256k1); the app enforces "verify before accept." |
| Cross-relay duplicate suppression | nostr-sdk (`Events` set) + App | App (event-id seen-set for streamed path) | `fetch_events` dedupes; a streamed/multi-call path needs an app-side seen-set. |
| Replaceable-event resolution (newest-wins, clamp, tie-break) | App (validation module) | — | Adversary-controlled `created_at`; bespoke policy the protocol leaves to the client. |
| p-tag extraction + malformity/size bounds | App (validation module) | nostr-sdk (`Tags::public_keys()`) | nostr-sdk extracts valid p-tags; the app applies the configurable cap and skip-malformed rule. |
| Persisting validated lists | Phase 1 store layer (`apply_follow_list`) | — | Out of Phase 2 scope; Phase 2 emits values, Phase 3 wires the writer. |

## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| RELAY-01 | Maintain connections to a configurable curated relay set; auto-reconnect with exponential backoff + jitter | `RelayOptions::reconnect(true)` (default), `retry_interval` (default 10s), `adjust_retry_interval(true)` (default). nostr-sdk reconnects automatically; **jitter/exponential is NOT confirmed in docs** — see Pitfall 8 + Open Question 1. Relay set from `config`. |
| RELAY-02 | Read each relay's NIP-11 document; respect advertised limits (max_subscriptions, max_limit, etc.) | `RelayInformationDocument` (nostr crate) with NIP-11 `limitation` fields; fetch + cache per relay; feed `max_limit` into the pagination planner. |
| RELAY-03 | Paginate (`until` windows, author chunking); never treat EOSE as completeness | `Filter::until()/authors()/limit()`; explicit loop (Pattern 2). `fetch_events` auto-closes on EOSE → must NOT be the completeness oracle (Pitfall 1). |
| RELAY-04 | Per-relay rate limiting; rate-limited notices trigger backoff | `governor` GCRA per relay (Pattern 3); branch on CLOSED/NOTICE `rate-limited` prefix → backoff. |
| INGEST-01 | Verify every event's signature before acceptance; discard + count invalid | `Event::verify()` (id + sig) at the gate (Pattern 4); `metrics` counter on rejects. |
| INGEST-02 | Process duplicate ids at most once | `fetch_events` returns deduping `Events`; cross-call/stream path needs an app `HashSet<EventId>` seen-set. |
| INGEST-03 | Newest valid kind-3 per pubkey; reject future-dated beyond clamp; same-ts tie → lowest id | Replaceable-event resolver (Pattern 5); configurable future clamp; compare `created_at` then `EventId` ordering. |
| INGEST-04 | Skip malformed p-tags; bound oversized lists by configurable cap without crashing | `Tags::public_keys()` (skips non-standard p-tags); apply configurable cap before/after extraction (Pattern 6). |
| INGEST-05 | Ingest + validate kind:10002 (NIP-65) under same replaceable-event rules | Same validation gate + resolver applied to `Kind::RelayList` (10002); newest-wins per pubkey. Storage of 10002 data is deferred (see Open Question 3). |

## Standard Stack

> Stack is locked by CLAUDE.md. All versions verified against the crates.io sparse index (`https://index.crates.io`) on 2026-06-12.

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| nostr-sdk | 0.44.1 | Umbrella crate: events, secp256k1 sig verify, relay pool, websocket lifecycle, NIP-11/NIP-02/NIP-65 types, filters, fetch/stream | Locked in CLAUDE.md; latest stable (0.45 is alpha only). Bundles `nostr` + `nostr-relay-pool`. Never hand-roll websocket/secp256k1/event parsing. `[VERIFIED: crates.io sparse index]` |
| nostr (transitive) | 0.44.3 | Core types: `Event`, `Filter`, `Tags`, `Kind`, `Timestamp`, `RelayInformationDocument` | Pulled by nostr-sdk in lockstep (0.44.x family). `[VERIFIED: crates.io sparse index]` |
| nostr-relay-pool (transitive) | 0.44.1 | `RelayOptions`, `RelayPool`, reconnect/retry policy | Managed by `Client`; `RelayOptions` exposes reconnect knobs. `[VERIFIED: crates.io sparse index]` |
| tokio | 1.52 | Async runtime (already a dep) | Required by nostr-sdk + sqlx. Already pinned in Cargo.toml. `[CITED: Cargo.toml]` |
| governor | 0.10.4 | Per-relay GCRA / token-bucket rate limiting | Locked in CLAUDE.md for politeness (RELAY-04). `[VERIFIED: crates.io sparse index + package-legitimacy OK]` |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| metrics | 0.24.6 | Counters for validation-failure / dedup / rate-limit-hit | INGEST-01 requires counting discarded events; lightweight facade so Phase 4 can add the Prometheus exporter without rework. `[VERIFIED: crates.io sparse index + legitimacy OK]` |
| tracing | 0.1.44 | Structured spans per relay / per fetch batch | Already implied by stack; useful for diagnosing silent stalls (Pitfall 8). Optional in Phase 2 if metrics counters suffice; full observability is Phase 4. `[VERIFIED]` |
| futures / futures-util | (transitive via nostr-sdk) | `StreamExt` for `stream_events` consumption | Only if the streamed pagination path is chosen over `fetch_events`. |

> **Note on `serde`/`reqwest`:** NIP-11 fetching uses HTTP(S) with `Accept: application/nostr+json`. nostr-sdk/nostr provides `RelayInformationDocument` and (per the relay pool) a way to obtain it; prefer the SDK's own fetch over adding a raw `reqwest` dependency. Confirm the exact accessor during planning (Open Question 2).

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| `Client::fetch_events` (managed) | `nostr-relay-pool` directly | Direct pool gives finer per-relay scheduling/backpressure but more code; per CLAUDE.md "profile first; default to `Client`." Default to `Client` in Phase 2; the pagination/rate-limit logic sits above it either way. |
| `governor` per-relay limiter | Hand-rolled token bucket / `tokio::time` sleeps | governor is GCRA, async-aware, battle-tested; hand-rolling rate limiting is a documented "don't hand-roll." |
| `fetch_events` (collect-then-return) | `stream_events` (async stream) | Stream lets you bound memory and apply the seen-set incrementally; `fetch_events` is simpler but buffers a whole window. Choose stream if window sizes risk memory pressure; otherwise `fetch_events` per window is fine at author-chunk granularity. |

**Installation (add to existing Cargo.toml):**
```toml
[dependencies]
# existing: sqlx, tokio, thiserror, anyhow, config, chrono
nostr-sdk = "0.44"
governor = "0.10"
metrics = "0.24"
# tracing = "0.1"          # optional in Phase 2; required Phase 4
```

**Version verification (2026-06-12, crates.io sparse index):**
- `nostr-sdk` latest stable **0.44.1** (0.45.0-alpha.1 exists but is prerelease — do NOT use). `[VERIFIED]`
- `nostr` 0.44.3, `nostr-relay-pool` 0.44.1 — the 0.44.x family ships in lockstep; pin `nostr-sdk = "0.44"` and let it resolve the family. `[VERIFIED]`
- `governor` 0.10.4, `metrics` 0.24.6, `tracing` 0.1.44 — all current. `[VERIFIED]`
- ⚠️ **Toolchain:** repo pins Rust **1.94.0** (`rust-toolchain.toml`) for sqlx 0.9. Confirm nostr-sdk 0.44.1 MSRV ≤ 1.94 during the first build (rust-nostr historically targets a much lower MSRV; low risk). `[ASSUMED — verify on first cargo build]`

## Package Legitimacy Audit

| Package | Registry | Age | Downloads | Source Repo | Verdict | Disposition |
|---------|----------|-----|-----------|-------------|---------|-------------|
| nostr-sdk | crates.io | since 2022-11 | ~28k/wk | github.com/rust-nostr/nostr | OK | Approved |
| governor | crates.io | since 2019-11 | ~937k/wk | github.com/boinkor-net/governor | OK | Approved |
| metrics | crates.io | since 2015-09 | ~1.1M/wk | github.com/metrics-rs/metrics | OK | Approved |
| metrics-exporter-prometheus | crates.io | (Phase 4) | — | github.com/metrics-rs/metrics | OK | Approved (deferred to Phase 4) |
| tracing-subscriber | crates.io | since 2019-06 | ~8.7M/wk | github.com/tokio-rs/tracing | OK | Approved (Phase 4) |

**Packages removed due to [SLOP] verdict:** none
**Packages flagged as suspicious [SUS]:** none
All packages discovered from CLAUDE.md's locked stack (an authoritative project source) and confirmed `OK` by `gsd-tools query package-legitimacy check --ecosystem crates`. Crates have no npm-style postinstall vector. `[VERIFIED: package-legitimacy seam]`

## Architecture Patterns

### System Architecture Diagram

```
        config (curated relay set, future-clamp, follow cap,            metrics
         per-relay rate, max-authors-per-req)                          counters
                 │                                                         ▲
                 ▼                                                         │
   ┌──────────────────────────── RELAY MODULE (acquisition) ──────────────┴────────────┐
   │                                                                                    │
   │  Client (nostr-sdk) ── add_relay(curated…) + RelayOptions{reconnect, retry}        │
   │       │  auto-reconnect / websocket lifecycle (nostr-sdk owns this)                │
   │       │                                                                            │
   │  NIP-11 limit cache:  per relay → {max_limit, max_subscriptions, max_filters}      │
   │       │                                                                            │
   │  per-relay governor (GCRA token bucket)  ── gate every REQ                         │
   │       │                                                                            │
   │  PAGINATION PLANNER (RELAY-03):                                                    │
   │    chunk author set into batches ≤ max_authors_per_req                             │
   │    for each batch:                                                                 │
   │      until = now                                                                   │
   │      loop:                                                                         │
   │        filter = authors(batch).kind(3 or 10002).limit(max_limit).until(until)      │
   │        events = client.fetch_events(filter, timeout)   // auto-closes on EOSE      │
   │        if events.len() < max_limit  → window complete, break                       │
   │        until = oldest received created_at − 1          // page back                │
   │      ── EOSE is NEVER the completeness signal; the count-vs-cap heuristic is       │
   │                                                                                    │
   │  on CLOSED/NOTICE "rate-limited" → backoff this relay (RELAY-04)                   │
   └───────────────────────────────────────┬────────────────────────────────────────────┘
                                            │ raw Events (one or many per author)
                                            ▼
   ┌────────────────────────── VALIDATION / INGEST MODULE ──────────────────────────────┐
   │  for each raw event:                                                                │
   │   1. event.verify()         ── id + secp256k1 sig   (INGEST-01) fail→discard+count  │
   │   2. kind == requested && author ∈ requested authors  (drop unsolicited)            │
   │   3. dedup by event id       (seen-set / Events)      (INGEST-02)                   │
   │   4. created_at clamp        future > clamp → reject  (INGEST-03)                   │
   │   5. replaceable resolve     keep newest per (pubkey,kind); tie → lowest id         │
   │   6. kind-3: Tags::public_keys() → dedup → drop self → cap at follow_cap (INGEST-04)│
   │      kind-10002: parse relay-list (NIP-65) under same newest-wins rule (INGEST-05)  │
   └───────────────────────────────────────┬────────────────────────────────────────────┘
                                            │ ValidatedFollowList { follower_pubkey,
                                            │   event_id, created_at, followee_pubkeys }
                                            ▼
                       Phase 3 wires this into store::apply_follow_list (Phase 1 writer)
```

File-to-implementation mapping is in Recommended Project Structure, not the diagram.

### Recommended Project Structure
```
src/
├── lib.rs                  # add `pub mod relay;` `pub mod ingest;`
├── error.rs                # extend StoreError or add RelayError / IngestError (thiserror)
├── store/                  # Phase 1 — unchanged
├── relay/
│   ├── mod.rs              # Client/pool wiring, RelayOptions, connect curated set (RELAY-01)
│   ├── nip11.rs            # fetch + cache RelayInformationDocument limits (RELAY-02)
│   ├── rate_limit.rs       # per-relay governor; rate-limited-notice backoff (RELAY-04)
│   └── fetch.rs            # author-chunked until-window pagination loop (RELAY-03)
└── ingest/
    ├── mod.rs              # the validation gate orchestrator
    ├── verify.rs           # Event::verify + kind/author match (INGEST-01)
    ├── replaceable.rs      # clamp + newest-wins + lowest-id tie-break (INGEST-03, -05)
    └── follow_list.rs      # p-tag extraction, dedup, self-drop, cap (INGEST-04)
tests/
├── verify_gate.rs          # forged/invalid event rejected + counted (INGEST-01)
├── dedup.rs                # same id from N relays processed once (INGEST-02)
├── replaceable.rs          # future clamp, newest-wins, tie-break (INGEST-03)
├── follow_list_bounds.rs   # malformed p-tags skipped; oversized list capped (INGEST-04)
├── relay_list.rs           # kind:10002 newest-wins (INGEST-05)
└── pagination.rs           # capped response triggers another page; EOSE not trusted (RELAY-03)
```

### Pattern 1: Connect a curated relay set with explicit reconnect policy (RELAY-01)
**What:** Build a `Client`, add each curated relay with `RelayOptions` enabling reconnect, connect.
**When:** Relay-module init.
**Key facts:**
- `RelayOptions::reconnect(bool)` default **true**; `retry_interval(Duration)` default **10s**; `adjust_retry_interval(bool)` default **true** ("adjust based on success/attempts"). `[VERIFIED: docs.rs nostr-relay-pool RelayOptions]`
- `Client::add_relay(url) -> Result<bool>`; `Client::connect()` (non-blocking, returns `()`); relay set comes from `config` (OPS-01 supplies the file later). `[VERIFIED: docs.rs Client]`
- **Gap:** docs confirm auto-reconnect + adjustable retry interval but do **NOT** confirm *exponential* growth or *jitter*. RELAY-01 explicitly requires "exponential backoff with jitter." See Pitfall 8 + Open Question 1 — the planner must either confirm nostr-sdk's `adjust_retry_interval` provides this, or layer an app-side backoff/jitter wrapper.

### Pattern 2: Paginated, author-chunked fetch that never trusts EOSE (RELAY-03)
**What:** For each author chunk, page backwards with `until` until a window returns fewer than the cap.
**When:** Every fetch of kind-3 / kind:10002 from a relay.
**Key facts:**
- `Filter` builder: `.authors(I: IntoIterator<Item=PublicKey>)`, `.kind(Kind)` / `.kinds(I)`, `.limit(usize)`, `.until(Timestamp)`, `.since(Timestamp)`. `[VERIFIED: docs.rs Filter]`
- `Client::fetch_events(filter, timeout: Duration) -> Result<Events>` performs an **auto-closing subscription closed on EOSE**. The returned `Events` is a deduping collection. `[VERIFIED: docs.rs Client]`
- **The trap:** auto-close-on-EOSE is exactly why EOSE must not be the completeness oracle. Compare `events.len()` against the effective cap (`min(requested limit, relay max_limit)`); if equal, there may be more — set `until = oldest_created_at - 1` and fetch again (Pitfall 1).
- For kind-3 you want one (newest) event per author, so chunk authors under `max_limit` rather than one giant filter. NIP-01 is also trending toward one filter per REQ — design batching not to assume many filters per REQ. `[CITED: prior PITFALLS.md / NIP-01]`

### Pattern 3: Per-relay rate limiting with governor + notice-driven backoff (RELAY-04)
**What:** One governor `RateLimiter` per relay URL gates outbound REQs; on a `rate-limited` CLOSED/NOTICE, back off that relay.
**When:** Around every fetch call.
**Key facts:**
- governor 0.10 GCRA limiter: `RateLimiter::direct(Quota::per_second(...))`; `.until_ready().await` to throttle. `[CITED: governor docs]`
- Branch on machine-readable relay message prefixes: `rate-limited` → exponential backoff w/ jitter for that relay; `blocked`/`restricted` → stop hitting it and surface a metric. nostr-sdk surfaces relay messages via notifications/`handle_notifications`. `[CITED: prior PITFALLS.md / NIP-01 OK/CLOSED prefixes]`
- Politeness target from PROJECT.md: "each list fetched roughly once" — the rate limiter protects relay goodwill, not throughput.

### Pattern 4: Signature-verification gate before any acceptance (INGEST-01)
**What:** First step of ingest: `event.verify()`; on error, discard + increment a counter; never store unverified events.
**When:** Every event from any relay, before dedup/resolution.
**Key facts:**
- `Event::verify()` = "Verify both EventId and Signature"; `Event::verify_signature()` = sig only; `Event::verify_id()` = id composition only. Use **`verify()`** (id + sig) — recomputing the id defends against a relay returning an event whose claimed id ≠ content. `[VERIFIED: docs.rs nostr::event::Event]`
- Also assert `event.kind == requested kind` and `event.pubkey ∈ requested authors` to drop unsolicited events a relay may inject (don't assume a relay returns only what you asked). `[CITED: prior PITFALLS.md — relays are dumb / adversarial]`
- secp256k1 is bundled in nostr-sdk — NEVER hand-roll (CLAUDE.md "What NOT to Use").

### Pattern 5: Replaceable-event resolution (INGEST-03, INGEST-05)
**What:** Across all valid events for a `(pubkey, kind)`, pick the winner: highest `created_at`, ties broken by lowest event id; reject future-dated beyond a configurable clamp.
**When:** After verification + dedup, before emitting the follow list.
**Key facts:**
- `created_at` is adversary-controlled. Reject any event whose `created_at > now + clamp` (configurable, e.g. 1h). Without this, one future-dated junk event permanently pins a pubkey's list. `[CITED: prior PITFALLS.md Pitfall 1]`
- Newest-wins: `max by created_at`; on equal `created_at`, keep the **lowest `EventId`** (lexical/byte order) — NIP-01's deterministic tie-break, prevents flapping between same-timestamp variants from different relays. `[CITED: NIP-01]`
- The Phase-1 store already records `applied_event_id` + `applied_created_at` and short-circuits on an unchanged id (GRAPH-02) — Phase 2's resolver must feed it the *winning* event id/created_at so that downstream comparison is meaningful.
- Apply the **identical** rule to kind:10002 (NIP-65) — it is also a replaceable event (INGEST-05).

### Pattern 6: p-tag extraction, dedup, self-drop, and bounded cap (INGEST-04)
**What:** Extract followee pubkeys from the winning kind-3, skip malformed tags, dedup, drop self-follows, cap the list size configurably.
**When:** After resolution, producing the `ValidatedFollowList`.
**Key facts:**
- `Tags::public_keys() -> impl Iterator<Item=&PublicKey>` extracts **only** valid standard p-tag variants — malformed/non-standard p-tags are skipped automatically (satisfies "malformed p-tags are skipped"). `[VERIFIED: docs.rs nostr Tags]`
- Apply a configurable `follow_cap` (e.g. reject or truncate lists beyond N p-tags) so a 50k-tag follow-bomb cannot stall the pipeline or bloat a row. Decide reject-vs-truncate and document it (Open Question 4). `[CITED: prior PITFALLS.md Pitfall 5]`
- Drop self-follows here too (defense in depth — the store also drops them, D-08), and dedup the followee set (a kind-3 can legally repeat a p-tag).
- Relay hints + petnames on p-tags are **discarded** (D-06) — only the pubkey set crosses into the store.

### Anti-Patterns to Avoid
- **Treating EOSE / `fetch_events` return as "complete."** It is not; relays silently cap at `max_limit`. Always page on count-vs-cap (Pitfall 1).
- **Storing or accepting an event before `verify()`.** Relays are adversarial; an unverified edge poisons the graph (Pitfall 5).
- **Naive `max(created_at)` for newest.** Future-dated junk pins the list forever; clamp + tie-break required (Pitfall 2).
- **One giant filter with thousands of authors / huge limit.** Triggers relay caps + truncation; chunk authors under `max_limit`.
- **Tight-loop reconnect.** Hammers relays into banning you; rely on nostr-sdk reconnect + per-relay backoff, never a bare retry loop (Pitfall 8).
- **Hand-rolling secp256k1, websocket framing, or a token bucket.** All provided (nostr-sdk, governor).
- **Trusting that a relay returns only requested kinds/authors.** Filter responses against your own request.
- **Unbounded p-tag processing.** Cap follow-list size (INGEST-04).

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| secp256k1 signature / event-id verification | Custom crypto | `Event::verify()` (nostr-sdk) | Security-critical; bundled & audited. CLAUDE.md forbids hand-rolling. |
| WebSocket lifecycle / relay connect / reconnect | Custom ws client | nostr-sdk `Client` + `RelayOptions` | Reconnect, framing, relay messages handled. |
| Cross-relay event dedup (single fetch) | Manual id set | `fetch_events` → deduping `Events` | Built in; only a streamed/multi-window path needs an app seen-set. |
| p-tag parsing / malformed-tag skipping | Manual tag walking | `Tags::public_keys()` | Extracts only valid standard p-tag variants; skips malformed automatically. |
| Rate limiting | Custom token bucket / sleeps | `governor` GCRA | Async, correct, battle-tested. |
| NIP-11 doc parsing | Manual JSON | `RelayInformationDocument` (nostr) | Typed limitation fields. |
| Filter / REQ JSON construction | Manual JSON | `Filter` builder | Compile-checked, correct wire format. |

**Key insight:** nostr-sdk + governor cover transport, crypto, parsing, dedup, and throttling. Phase 2's *only* genuinely custom logic is the **pagination loop**, the **replaceable-event policy** (clamp + newest-wins + tie-break), and the **bounds guards** (follow cap, self-drop). Those three are where all the correctness risk lives — concentrate test effort there.

## Common Pitfalls

### Pitfall 1: Treating EOSE / `fetch_events` return as proof of completeness
**What goes wrong:** `fetch_events` auto-closes on EOSE and returns. A relay silently caps results at `max_limit` (often 500) below your requested limit, so you get a truncated set, see "done," and permanently miss pubkeys — a hole in the reachable graph.
**Why it happens:** EOSE only separates stored from live events; it is not a completeness signal, and `fetch_events`'s convenience hides the cap.
**How to avoid:** Compare `events.len()` to `min(requested_limit, relay_max_limit)`; if equal, page back with `until = oldest_created_at - 1` until a window returns strictly fewer. Chunk authors so per-author hit-rate is measurable.
**Warning signs:** Author batches returning exactly the cap every time; coverage plateau; known-active pubkeys never appearing.

### Pitfall 2: Future-dated `created_at` pins a pubkey's list to garbage (INGEST-03)
**What goes wrong:** A single event dated in 2099 wins "newest" forever; the real current list never gets applied.
**Why it happens:** `created_at` is unauthenticated client input; naive `max(created_at)` trusts it.
**How to avoid:** Reject `created_at > now + clamp` (configurable, default ~1h) before the newest-wins comparison. Store receipt time separately for freshness if needed.
**Warning signs:** `MAX(applied_created_at)` years in the future; a pubkey's list never updates despite activity.

### Pitfall 3: Same-timestamp flapping without the lowest-id tie-break (INGEST-03)
**What goes wrong:** Two relays hand you two valid events with identical `created_at`; without a deterministic tie-break, the applied list oscillates between crawls.
**Why it happens:** The NIP-01 tie-break (same `created_at` → lowest event id) is easy to omit.
**How to avoid:** Implement the full rule: max `created_at`, ties → lowest `EventId` byte order.
**Warning signs:** A pubkey's edge set oscillates on consecutive crawls with no real change.

### Pitfall 4: Accepting events the relay was not asked for (INGEST-01/02)
**What goes wrong:** A malicious/buggy relay returns events of other kinds or other authors; if you trust the response shape, you ingest fabricated edges.
**Why it happens:** Assuming relays only return matching events.
**How to avoid:** After `verify()`, assert `kind == requested` and `pubkey ∈ requested authors`; drop and count mismatches.
**Warning signs:** Edges from one relay no other corroborates; kinds/authors outside the request.

### Pitfall 5: Skipping or deferring signature verification "for speed" (INGEST-01)
**What goes wrong:** Forged events from a dumb/malicious relay enter the graph; the spam layer is defeated at its foundation.
**Why it happens:** Per-event verification over hundreds of millions of events is costly, tempting an "optimization."
**How to avoid:** Verify every event with `Event::verify()` before acceptance — non-negotiable. Parallelize/batch verification in Rust if it's a throughput bottleneck, but never skip it.
**Warning signs:** Unverified-event count is zero because the gate was never wired; suspiciously manipulated trust results.

### Pitfall 6: Oversized follow lists / malformed p-tags crash or stall the pipeline (INGEST-04)
**What goes wrong:** A kind-3 with 50k+ p-tags (legal but adversarial) blows a row/memory; a malformed p-tag panics a naive parser.
**Why it happens:** No cap; manual tag parsing assuming well-formed input.
**How to avoid:** `Tags::public_keys()` skips malformed tags; apply a configurable `follow_cap` (reject or truncate) before emitting. Never `unwrap()` on tag contents.
**Warning signs:** Memory spike on a single event; pipeline stall on one pubkey.

### Pitfall 7: Duplicate events from multiple relays processed more than once (INGEST-02)
**What goes wrong:** The same event id from N relays is validated/applied N times, wasting work and risking non-idempotent side effects in metrics.
**Why it happens:** Multi-relay fan-out without a seen-set when not using `fetch_events`'s built-in dedup.
**How to avoid:** Within a single `fetch_events` call, `Events` dedupes for you. Across windows/streams/relays, keep an app `HashSet<EventId>` and skip seen ids. The store's id-equal short-circuit (GRAPH-02) is the final backstop.
**Warning signs:** Validation counters far exceeding distinct-event counts.

### Pitfall 8: nostr-sdk reconnect may not be exponential-with-jitter (RELAY-01)
**What goes wrong:** RELAY-01 mandates exponential backoff *with jitter*. nostr-sdk confirms auto-reconnect + an adjustable retry interval, but the docs do not confirm exponential growth or jitter — a fixed 10s retry across many relays after a shared outage can cause synchronized reconnect storms (thundering herd) that look like a DoS.
**Why it happens:** Relying on library defaults without verifying they meet the requirement.
**How to avoid:** During planning, verify whether `adjust_retry_interval=true` yields exponential+jitter; if not, layer an app-side backoff (e.g. capped exponential + random jitter) around reconnect, or set per-relay `retry_interval` with jitter. Treat this as a tracked task, not an assumption.
**Warning signs:** Synchronized reconnect spikes after an outage; relays rate-limiting on reconnect.

### Pitfall 9: Silent websocket disconnects stall the crawl (RELAY-01, supports OPS later)
**What goes wrong:** A socket drops silently; a subscription hangs with no events and no EOSE; in-flight authors never complete.
**Why it happens:** WebSocket libs don't always surface network-level drops; no per-fetch deadline.
**How to avoid:** Always pass a `timeout` to `fetch_events`; treat a timed-out window as "requeue these authors," not "done." nostr-sdk's reconnect handles the socket; the app handles the unfinished work. (Frontier requeue is Phase 3, but the per-fetch timeout discipline starts here.)
**Warning signs:** Throughput drops to zero with no error; a relay's in-flight count stuck.

## Code Examples

> Signatures verified against docs.rs for nostr 0.44.3 / nostr-sdk 0.44.1 on 2026-06-12. Exact NIP-11 accessor and the reconnect-jitter detail are flagged as Open Questions.

### Connect curated relay set with reconnect policy (RELAY-01)
```rust
// Source: docs.rs nostr-sdk Client + nostr-relay-pool RelayOptions (0.44)
use nostr_sdk::prelude::*;
use std::time::Duration;

let client = Client::builder().build();          // no signer needed; crawler is read-only
for url in &config.curated_relays {
    // RelayOptions: reconnect=true (default), retry_interval=10s (default),
    // adjust_retry_interval=true (default). See Open Question 1 re: jitter.
    client.add_relay(url).await?;                 // returns Result<bool>
}
client.connect().await;                          // non-blocking; pool manages sockets + reconnect
```

### Paginated author-chunked fetch that never trusts EOSE (RELAY-03)
```rust
// Source: docs.rs Filter + Client::fetch_events (0.44). Pagination loop is app logic.
use nostr_sdk::prelude::*;
use std::time::Duration;

async fn fetch_kind3_complete(
    client: &Client,
    authors: &[PublicKey],
    max_limit: usize,        // from NIP-11 (RELAY-02), fallback to a sane default
    max_authors: usize,      // chunk size under relay caps
    timeout: Duration,
) -> Result<Vec<Event>> {
    let mut out = Vec::new();
    for chunk in authors.chunks(max_authors) {
        let mut until = Timestamp::now();
        loop {
            let filter = Filter::new()
                .authors(chunk.iter().copied())
                .kind(Kind::ContactList)         // kind 3
                .limit(max_limit)
                .until(until);
            let events = client.fetch_events(filter, timeout).await?; // auto-closes on EOSE
            let n = events.len();
            // page back ONLY by count-vs-cap; EOSE alone is NOT completeness (Pitfall 1)
            let oldest = events.iter().map(|e| e.created_at).min();
            out.extend(events.into_iter());
            match (n >= max_limit, oldest) {
                (true, Some(ts)) => until = Timestamp::from(ts.as_u64().saturating_sub(1)),
                _ => break,                       // fewer than cap → window complete
            }
        }
    }
    Ok(out)
}
```

### Signature-verification gate + kind/author match (INGEST-01)
```rust
// Source: docs.rs nostr::event::Event::verify (0.44)
fn accept(event: &Event, want_kind: Kind, requested: &HashSet<PublicKey>) -> bool {
    if event.verify().is_err() {                 // id + secp256k1 sig (Pattern 4)
        metrics::counter!("ingest_invalid_signature").increment(1);
        return false;
    }
    if event.kind != want_kind || !requested.contains(&event.pubkey) {
        metrics::counter!("ingest_unsolicited").increment(1);
        return false;                            // drop events we didn't ask for (Pitfall 4)
    }
    true
}
```

### Replaceable-event resolution: clamp + newest-wins + tie-break (INGEST-03/-05)
```rust
// Source: NIP-01 replaceable semantics; created_at clamp = community practice
fn pick_winner<'a>(
    events: impl Iterator<Item = &'a Event>,
    now: Timestamp,
    future_clamp_secs: u64,
) -> Option<&'a Event> {
    let max_ok = now.as_u64().saturating_add(future_clamp_secs);
    events
        .filter(|e| e.created_at.as_u64() <= max_ok)        // reject future-dated (Pitfall 2)
        .max_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)                          // newest by created_at
                .then_with(|| b.id.cmp(&a.id))               // tie → LOWEST id wins (Pitfall 3)
        })
}
```

### p-tag extraction, self-drop, dedup, cap (INGEST-04)
```rust
// Source: docs.rs nostr Tags::public_keys (0.44)
fn followee_pubkeys(event: &Event, follow_cap: usize) -> Vec<PublicKey> {
    let mut seen = HashSet::new();
    let mut out: Vec<PublicKey> = event
        .tags
        .public_keys()                          // skips malformed p-tags automatically
        .copied()
        .filter(|pk| *pk != event.pubkey)       // drop self-follow (D-08, defense in depth)
        .filter(|pk| seen.insert(*pk))          // dedup repeated p-tags
        .collect();
    if out.len() > follow_cap {                 // bound oversized lists (Pitfall 6)
        metrics::counter!("ingest_oversized_follow_list").increment(1);
        out.truncate(follow_cap);               // or reject entirely — decide (Open Question 4)
    }
    out
}
```

> ⚠️ The exact spelling of `Kind::ContactList`, `Timestamp::from`/`as_u64`, and `EventId`'s `Ord` are illustrative; confirm against `nostr` 0.44.3 during planning (Assumptions A2–A4). The *patterns* are the load-bearing content.

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Hand-rolled ws + secp256k1 + REQ JSON | nostr-sdk `Client` + `Event::verify` + `Filter` | rust-nostr maturity | Phase 2 is mostly wiring + 3 custom policies, not protocol plumbing. |
| Trust EOSE = complete | Count-vs-`max_limit` pagination heuristic | Long-standing NIP-01 subtlety | Mandatory for a "complete reachable" crawl (RELAY-03). |
| `max(created_at)` newest | Clamp + newest-wins + lowest-id tie-break | NIP-01 + adversarial reality | Prevents future-dated pinning and flapping (INGEST-03). |
| Many filters per REQ | One filter per REQ; author chunking | NIP-01 direction | Don't batch many filters into one REQ. |

**Deprecated/outdated:**
- nostr-sdk 0.45.0-alpha.1 exists but is **prerelease** — stay on 0.44.1 until 0.45 is stable.
- CLAUDE.md's "Rust 1.84+" floor is already superseded by the repo's pinned 1.94 (sqlx 0.9 MSRV).

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | nostr-sdk 0.44.1 MSRV ≤ 1.94 (compatible with pinned toolchain) | Standard Stack | Medium — if MSRV > 1.94, build fails; bump toolchain. Verify on first `cargo build`. |
| A2 | kind-3 maps to `Kind::ContactList` in nostr 0.44 | Code Examples | Low — name may differ (`Kind::Custom(3)` fallback always works); confirm in docs. |
| A3 | `EventId`/`Timestamp` expose `Ord`/`as_u64`/`from` as shown | Code Examples | Low — adjust to actual API; tie-break/clamp logic is unchanged. |
| A4 | `RelayInformationDocument` is reachable via the SDK to read `limitation.max_limit` per relay | RELAY-02 / Open Q2 | Medium — if no SDK accessor, add a small `reqwest` NIP-11 fetch (Accept: application/nostr+json). |
| A5 | nostr-sdk `adjust_retry_interval` does NOT guarantee jitter/exponential | RELAY-01 / Pitfall 8 | Medium — drives whether an app-side backoff wrapper is needed; verify before claiming RELAY-01 satisfied. |
| A6 | kind:10002 storage is NOT required in Phase 2 (validate now, store in Phase 5) | INGEST-05 / Open Q3 | Medium — if 10002 must persist now, Phase 2 needs an additive migration + store fn. |
| A7 | `Client::fetch_events` `Events` dedupes by event id within a call | INGEST-02 | Low — even if not, the app seen-set + store short-circuit cover it. |

## Open Questions

1. **Does nostr-sdk's reconnect provide exponential backoff WITH jitter (RELAY-01)?**
   - Known: reconnect=true, retry_interval=10s, adjust_retry_interval=true (adjusts on success/attempts).
   - Unclear: whether "adjust" means exponential + jitter, or just success-based tuning.
   - Recommendation: Plan a task to verify the actual algorithm (read `nostr-relay-pool` source / changelog). If it lacks jitter, layer an app-side capped-exponential+jitter backoff. Do not mark RELAY-01 satisfied on the default alone.

2. **What is the exact nostr-sdk 0.44 accessor for a relay's NIP-11 document (RELAY-02)?**
   - Known: `RelayInformationDocument` type exists with NIP-11 `limitation` fields (max_limit, max_subscriptions, max_filters per the spec).
   - Unclear: whether it's `relay.document().await`, a `Client` method, or requires a manual HTTP fetch.
   - Recommendation: Plan a spike task to locate the accessor; fallback is a tiny `reqwest` GET with `Accept: application/nostr+json`. Cache per relay; provide sane defaults when a relay omits limits.

3. **Must kind:10002 (NIP-65) data be PERSISTED in Phase 2, or only validated (INGEST-05)?**
   - Known: INGEST-05 says "ingested and validated under the same replaceable-event rules." RELAY-05/06 (NIP-65 fallback consuming 10002) are Phase 5. Phase 1 explicitly deferred kind:10002 storage tables to a later additive migration (D-13).
   - Unclear: whether "ingested" implies storage now or just validation now.
   - Recommendation: Phase 2 validates 10002 with the same gate/resolver and emits a validated value; defer the storage table + write fn to Phase 5 (where it's consumed) unless the planner/operator wants it persisted now. If persisted now, add an additive migration. Flag for discuss-phase.

4. **Oversized follow list: reject vs. truncate (INGEST-04)?**
   - Known: must be bounded by a configurable cap "without crashing."
   - Unclear: whether to drop the whole list or truncate to the cap.
   - Recommendation: Reject-and-count is safer (a 50k-tag event is almost certainly a follow-bomb, not a real list); truncation silently corrupts the graph. Default to reject + metric; make it config-driven. Confirm with operator.

5. **`fetch_events` (buffer-per-window) vs. `stream_events` (incremental)?**
   - Known: both exist; `fetch_events` buffers a window, `stream_events` yields incrementally.
   - Recommendation: `fetch_events` per author-chunk window is simplest and memory is bounded by chunk size × max_limit; default to it. Switch to `stream_events` only if window memory becomes a measured problem.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Rust toolchain (cargo) | entire build | ✗ (not verified this session) | pinned 1.94.0 via rust-toolchain.toml | BLOCKING — must be installed (was a Phase 1 prereq) |
| Network egress to public relays (wss://) | RELAY-01/02/03 live tests | unverified | — | Mock relay / recorded fixtures for unit tests (see Validation) |
| Docker | testcontainers (only if store integration is touched) | ✓ (Phase 1 used it) | — | Phase 2 is mostly DB-free; validation tests need no DB |
| A test relay or fixtures | INGEST/RELAY tests | ✗ | — | Construct `Event`s in-test with known keys; a local ephemeral relay (e.g. an in-process mock) for pagination/EOSE tests |

**Missing dependencies with no fallback:** none that block (validation logic is testable offline with constructed events).
**Missing dependencies with fallback:** Live relay access — substitute constructed/forged events and a mock or recorded relay for deterministic tests; reserve live-relay smoke tests as a manual or opt-in step (network-dependent, not in the fast suite).

## Validation Architecture

> nyquist_validation is enabled (config `workflow.nyquist_validation: true`).

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in harness (`#[test]` / `#[tokio::test]`); follows Phase 1 conventions |
| Config file | none required; `SQLX_OFFLINE=true` in CI (only if store tests run) |
| Quick run command | `cargo test --lib` (pure validation logic — no network, no Docker) |
| Full suite command | `cargo test` (adds integration tests; mock/recorded relay for pagination) |

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| INGEST-01 | Forged/invalid-sig event rejected and counted | unit | `cargo test verify_gate` | ❌ Wave 0 |
| INGEST-01 | Event of wrong kind/author dropped | unit | `cargo test verify_gate::unsolicited` | ❌ Wave 0 |
| INGEST-02 | Same id from N relays processed once | unit/integration | `cargo test dedup` | ❌ Wave 0 |
| INGEST-03 | Future-dated > clamp rejected | unit | `cargo test replaceable::future_clamp` | ❌ Wave 0 |
| INGEST-03 | Newest-wins; same-ts → lowest id | unit | `cargo test replaceable::tie_break` | ❌ Wave 0 |
| INGEST-04 | Malformed p-tags skipped | unit | `cargo test follow_list_bounds::malformed` | ❌ Wave 0 |
| INGEST-04 | Oversized list bounded without panic | unit | `cargo test follow_list_bounds::cap` | ❌ Wave 0 |
| INGEST-05 | kind:10002 newest-wins resolution | unit | `cargo test relay_list` | ❌ Wave 0 |
| RELAY-03 | Capped response triggers another page; EOSE not trusted | integration | `cargo test pagination` (mock relay) | ❌ Wave 0 |
| RELAY-02 | NIP-11 limits parsed + capped into filter | unit/integration | `cargo test nip11_limits` | ❌ Wave 0 |
| RELAY-04 | rate-limited notice triggers backoff | unit | `cargo test rate_limit_backoff` | ❌ Wave 0 |
| RELAY-01 | Reconnect policy applied (+ jitter if app-layer added) | unit | `cargo test reconnect_policy` | ❌ Wave 0 |

### Sampling Rate
- **Per task commit:** `cargo test --lib` (validation logic — fast, offline).
- **Per wave merge:** `cargo test` (incl. mock-relay pagination/NIP-11 integration tests).
- **Phase gate:** Full suite green before `/gsd-verify-work`.

### Wave 0 Gaps
- [ ] Test fixtures: helpers to build signed `Event`s with known keys, plus a forged/invalid-sig event, plus same-ts variants for tie-break.
- [ ] A mock or in-process relay (or recorded responses) that can return capped result sets + EOSE for the pagination test (RELAY-03) — the single hardest fixture; pin it down in Wave 0.
- [ ] `tests/` files per the table above.
- [ ] First `cargo build` confirms nostr-sdk 0.44.1 compiles on toolchain 1.94 (A1).

## Security Domain

> security_enforcement enabled (config `workflow.security_enforcement: true`, ASVS level 1). Phase 2 is the project's primary adversarial-input boundary.

### Applicable ASVS Categories
| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | Crawler is an anonymous read-only relay client; no auth surface. |
| V3 Session Management | no | No sessions. |
| V4 Access Control | no | No multi-user access in this phase. |
| V5 Input Validation | **yes** | Every relay-supplied event is untrusted input: `Event::verify()` (sig+id), kind/author match, `created_at` clamp, p-tag malformity skip, follow-list size cap. This is the core of Phase 2. |
| V6 Cryptography | **yes** | secp256k1 sig verification via nostr-sdk — NEVER hand-roll (CLAUDE.md). |
| V7 Error Handling / Logging | partial | Count discarded/invalid events (INGEST-01); never `unwrap()` on relay input (no panics from adversarial data). |

### Known Threat Patterns for nostr relay acquisition (adversarial by design)
| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Forged/impersonated event from a dumb/malicious relay | Spoofing / Tampering | `Event::verify()` (recompute id + verify sig) before acceptance; drop+count failures. |
| Unsolicited events injected in a response | Tampering | Assert kind == requested && author ∈ requested; drop mismatches. |
| Future-dated `created_at` pinning a list | Tampering / DoS-of-updates | Reject `created_at > now + clamp`. |
| Follow-bomb (50k+ p-tags) | DoS | Configurable follow-list cap (reject/truncate); no unbounded allocation. |
| Malformed p-tag crashing the parser | DoS | `Tags::public_keys()` skips malformed; never `unwrap()` tag fields. |
| Duplicate flooding from many relays | DoS / wasted work | Dedup by event id (`Events` + app seen-set). |
| Aggressive crawl → IP ban | (availability) | Per-relay governor rate limit; NIP-11 limits respected; backoff on rate-limited. |
| Reconnect storm (thundering herd) | (availability) | Backoff + jitter on reconnect (verify nostr-sdk provides, else app-layer). |

## Sources

### Primary (HIGH confidence)
- docs.rs `nostr` 0.44.3 — `Event::verify/verify_signature/verify_id`, `Filter::authors/kinds/kind/limit/until/since`, `Tags::public_keys()` — exact signatures quoted. (2026-06-12)
- docs.rs `nostr-sdk` latest (0.44.x) `Client` — `fetch_events(filter, timeout) -> Result<Events>` (auto-closes on EOSE), `fetch_events_from`, `subscribe`, `add_relay`, `connect`, `stream_events*`, notifications/`handle_notifications`. (2026-06-12)
- docs.rs `nostr-relay-pool` `RelayOptions` — `reconnect` (default true), `retry_interval` (default 10s), `adjust_retry_interval` (default true). (2026-06-12)
- crates.io sparse index (`https://index.crates.io`) — nostr-sdk 0.44.1 (latest stable; 0.45 alpha only), nostr 0.44.3, nostr-relay-pool 0.44.1, governor 0.10.4, metrics 0.24.6, tracing 0.1.44. (2026-06-12)
- `gsd-tools query package-legitimacy check --ecosystem crates` — nostr-sdk, governor, metrics, tracing-subscriber all `OK`. (2026-06-12)
- CLAUDE.md — locked stack, "What NOT to Use", version-compatibility table.
- `.planning/REQUIREMENTS.md`, `.planning/ROADMAP.md`, `.planning/STATE.md` — Phase 2 scope, requirements, Phase 1 outcomes.
- `src/store/*.rs`, `migrations/0001_graph_schema.sql` — Phase 1 store API and schema the Phase 2 output feeds (`apply_follow_list`, `applied_event_id`/`applied_created_at`, status lifecycle).

### Secondary (MEDIUM confidence)
- `.planning/research/PITFALLS.md` — nostr crawler pitfalls (created_at trust, EOSE≠complete, rate-limit/ban, signature verification, follow-bomb), cross-checked against NIP-01/02/11/65.
- NIP-01 (replaceable events, EOSE, OK/CLOSED prefixes, tie-break), NIP-02 (kind-3 p-tags), NIP-11 (limitation fields incl. max_limit), NIP-65 (kind:10002) — protocol semantics.

### Tertiary (LOW confidence)
- General WebSearch on nostr-sdk reconnect/backoff internals — used only to flag the jitter uncertainty (Open Question 1), not as load-bearing fact.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — versions verified live; nostr-sdk is the locked, canonical crate; event/filter/tags API signatures quoted from docs.rs.
- Architecture: HIGH — responsibilities clearly split between nostr-sdk (transport/crypto/parse/dedup), governor (rate limit), and three custom policies (pagination, replaceable resolution, bounds).
- Pitfalls: HIGH — drawn from prior project pitfall research mapped to verified NIP semantics; this is the project's known-risk domain.
- Two MEDIUM-risk gaps tracked as Open Questions: reconnect jitter (RELAY-01) and the NIP-11 accessor (RELAY-02). Neither blocks planning; both are first-task spikes.

**Research date:** 2026-06-12
**Valid until:** 2026-07-12 (stable stack; re-verify if nostr-sdk 0.45 goes stable before planning completes).
