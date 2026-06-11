# Pitfalls Research

**Domain:** Nostr social-graph crawler / relay-scraping / large-graph ingestion (Rust daemon)
**Researched:** 2026-06-11
**Confidence:** MEDIUM-HIGH (protocol semantics from NIP-01/02/11/65 are HIGH; operational lessons synthesized from one comparable open crawler + community post-mortems are MEDIUM)

This file catalogs what nostr crawler / large-graph-ingestion projects get wrong in practice, mapped to the specific shape of this project: a Rust daemon doing a full reachable crawl of kind-3 follow lists (~millions of pubkeys, hundreds of millions of edges), freshness-driven refresh, feeding a shared DB read by a separate spam layer, single operator, unattended.

## Critical Pitfalls

### Pitfall 1: Trusting `created_at` as ground truth for "newest" replaceable event

**What goes wrong:**
Kind 3 is replaceable: only the newest event per pubkey counts, and "newest" is defined by `created_at`. But `created_at` is a client-supplied integer with no enforcement. You will encounter events dated in the far future (2030, 2099, year-9999 garbage), events with `created_at = 0`, and clients with wrong system clocks. If you naively keep "the event with the max `created_at`," a single future-dated junk event permanently pins a pubkey's follow list to garbage and your crawler will never accept the real, current list — because nothing will ever have a higher timestamp.

**Why it happens:**
NIP-01 says relays keep the highest-`created_at` event per (pubkey, kind), and developers mirror that rule verbatim without realizing the timestamp is adversary-controlled. The protocol's own tie-break (same `created_at` → lowest event id lexically wins) lulls you into treating the rule as authoritative.

**How to avoid:**
- Reject or clamp events whose `created_at` is more than a small window (e.g. 1 hour, at most 1 day) in the future relative to the crawler's own clock. This is established community practice (some validators reject >60s in the future; relays often refuse future-dated events).
- Reject implausibly old timestamps for "newest" selection (e.g. `created_at < nostr_genesis`).
- Store the timestamp you *received* the event alongside `created_at`, so freshness/staleness logic can fall back to receipt time, not attacker time.
- Implement the full replaceable rule including the tie-break: same `created_at` → keep lowest event id in lexical (hex) order. Without this, two relays handing you same-timestamp variants cause nondeterministic flapping.

**Warning signs:**
A pubkey's follow list never updates despite the user clearly being active; `MAX(created_at)` in the DB shows timestamps years in the future; the same pubkey oscillates between two follow sets on consecutive crawls.

**Phase to address:**
Event-ingestion / validation phase (the phase that defines how a raw kind-3 event becomes a stored follow list). Must exist before the first real crawl.

---

### Pitfall 2: Older event arriving after newer one overwrites good data (no monotonic guard)

**What goes wrong:**
Events arrive out of order and in duplicate from many relays. Relay A gives you the current list; later relay B (a stale mirror that never got the update) gives you last month's version of the same pubkey. If your write path is "last write wins" you clobber fresh data with stale data. The downstream spam layer then computes trust over an out-of-date edge set.

**Why it happens:**
The crawl is concurrent across thousands of connections; events for the same pubkey land at unpredictable times. A simple `UPSERT ... SET follows = new` ignores ordering. Relays themselves "clobber" (they can't run a merge function), so each relay's copy is just whatever it last accepted — they routinely disagree.

**How to avoid:**
- Make the write conditional: only replace a pubkey's stored list when the incoming event is strictly newer per the replaceable rule (`created_at` greater, or equal-with-lower-id). Enforce this at the DB layer (conditional UPDATE / `WHERE incoming_created_at > stored_created_at`) so concurrent writers can't race past the guard.
- Treat the stored event's `id` and `created_at` as the version key; compare every incoming event against it before writing.
- Query *multiple* relays per pubkey and merge by selecting the winner — do not assume one relay is canonical. The strategy doc already plans curated-set + NIP-65 fallback; make sure the selection step picks the newest across all responses, not the first responder.

**Warning signs:**
Edge counts for a pubkey fluctuate down then up across crawls; follow lists "go backwards" (users you saw following 200 now show 150 then 200 again); spam-layer complains the graph is non-monotonic.

**Phase to address:**
Storage / write-path phase (schema + conditional-write semantics). Tie this to the schema design since the spam layer reads the result.

---

### Pitfall 3: Aggressive crawling → rate-limited, throttled, or IP-banned by relays

**What goes wrong:**
A naive full crawl opens many subscriptions, blasts large `REQ` batches, and reconnects in a tight loop on error. Relays respond with `CLOSED`/`OK` messages tagged `rate-limited` or `blocked`, silently drop your subscriptions, slow you to a crawl, or ban your IP. Because the project explicitly relies on "relay goodwill," getting banned from the curated workhorse relays is close to a project-ending failure for unattended operation.

**Why it happens:**
Developers ignore NIP-11 relay limitation fields (`max_subscriptions`, `max_filters`, `max_limit`, `max_subid_length`) and the per-connection nature of subscriptions. They send more filters per `REQ` or more concurrent subs than the relay allows; the relay rejects requests that exceed its limits, and excess traffic looks like a DoS.

**How to avoid:**
- Fetch and cache each relay's NIP-11 document at connect time; respect `max_subscriptions`, `max_filters`, `max_limit`. Note that NIP-01 has been moving toward a single filter per `REQ` — design batching to not assume many filters per REQ.
- Honor machine-readable `CLOSED`/`OK` prefixes: on `rate-limited`, back off exponentially with jitter; on `blocked`/`restricted`, stop hitting that relay and alert.
- Cap concurrent subscriptions *per relay* well under the advertised max; use one persistent websocket per relay (NIP-01 expects a single connection per relay) and multiplex subscriptions over it.
- Add global politeness: per-relay request rate limits, jittered scheduling, and a configurable concurrency ceiling. Identify yourself honestly where supported.

**Warning signs:**
Rising rate of `rate-limited`/`blocked` `CLOSED` messages; subscriptions silently never returning EOSE; sudden 100% connection failures to a relay that worked yesterday (IP ban); falling crawl throughput with no error logged.

**Phase to address:**
Relay-connection / transport phase. This is foundational; build politeness and NIP-11 awareness before scaling concurrency.

---

### Pitfall 4: Mistaking EOSE for "you now have every event" → silent partial data

**What goes wrong:**
EOSE ("End Of Stored Events") only marks the boundary between stored and live events. It does NOT mean the relay sent everything matching your filter. Relays enforce an internal cap (commonly 500, sometimes lower than your requested `limit`) independent of your `limit`. If the cap is below your `limit`, you receive a truncated set, see EOSE, and incorrectly conclude the result is complete — silently missing pubkeys. For a *complete reachable* crawl, silently dropped events mean permanently missing branches of the graph.

**Why it happens:**
NIP-01 EOSE semantics are subtle and widely misunderstood. The "is it complete?" signal (NIP-67 `finish`/`more` hints) is new and not widely implemented, so most relays give you no completeness signal at all.

**How to avoid:**
- Never treat EOSE as completeness. Use the standard pagination heuristic: if events received ≈ requested `limit` (or ≈ the relay's `max_limit`), assume there may be more and paginate with `until` set to the oldest received `created_at`, repeating until you get strictly fewer than the cap.
- For kind 3 specifically, you usually want one event per author, so paginate by authors (chunk author lists across multiple `REQ`s under `max_limit`) rather than relying on a huge single filter.
- Where a relay advertises NIP-67 EOSE hints, honor `more`/`finish`. Otherwise assume `more` whenever you hit the cap.
- Cross-check coverage: track how many requested authors actually returned an event per relay; a low hit-rate is a sign of truncation or a relay that doesn't hold those authors.

**Warning signs:**
Author-batch hit rate suspiciously round (exactly 500 events back every time); coverage metric plateaus below expected; pubkeys known to be active never appear despite being requested.

**Phase to address:**
Fetch / pagination phase (how a `REQ` becomes a complete per-author result). Build the pagination loop and per-relay cap detection here.

---

### Pitfall 5: Frontier explosion and memory blowup at full-reachable scale

**What goes wrong:**
A full reachable crawl from a well-connected anchor expands fast. Naive BFS keeps the frontier (to-visit set) and a "seen" set in memory; at millions of pubkeys and hundreds of millions of edges, an in-RAM frontier/visited structure plus in-flight events OOM-kills the daemon. Worse, spam follow-list bombs (accounts with tens of thousands of p-tags, follow farms pointing at huge fabricated cohorts) inflate the frontier with pubkeys nobody legitimately reaches.

**Why it happens:**
Graph crawlers are prototyped at thousands of nodes where everything fits in memory. The jump to millions is not linear because edges (the hundreds-of-millions number) dominate, and adversarial follow lists are deliberately huge.

**How to avoid:**
- Keep the frontier and visited/seen sets on disk (in the DB or an embedded KV store), not solely in RAM. Stream the frontier; don't materialize the whole graph in memory.
- Use a compact "seen" representation (the DB's own pubkey table as the dedup oracle, or a bloom/sorted-key structure) rather than a giant in-memory `HashSet<[u8;32]>`.
- Enforce the project's own scope rule mechanically: only enqueue a pubkey when something in the *reachable* set points to it (PROJECT.md: "never spend effort on pubkeys nobody points to"). This naturally starves spam islands — but verify the rule is applied at enqueue time, not after fetching.
- Cap per-event p-tag processing: a kind-3 event with 50k+ tags is legal but should be bounded/validated; reject or truncate absurd lists and log them rather than letting one event explode the frontier and a single DB row.
- Bound in-flight work: backpressure between the fetch stage and the parse/write stage so a fast relay can't fill memory with unprocessed events.

**Warning signs:**
RSS growing without bound during initial crawl; frontier size growing faster than completion; a handful of pubkeys with enormous follow counts; OOM kill on the initial crawl that "worked in testing."

**Phase to address:**
Frontier / scheduling phase and storage phase. The on-disk frontier decision is architectural and must be made before the initial full crawl.

---

### Pitfall 6: Crawl restart loses state / no resumable checkpoint

**What goes wrong:**
The initial full crawl runs for hours or days. The daemon crashes, the operator restarts it, or it gets OOM-killed (see Pitfall 5) — and it starts over from the anchor because the frontier and progress lived in memory. For an unattended single-operator daemon, a non-resumable crawl is a recurring catastrophe: every transient failure costs the whole crawl and burns relay goodwill re-fetching everything.

**Why it happens:**
"Crawl then persist results" is the obvious design; the crawl *state* (what's queued, what's in-progress, what's done, freshness clocks) is treated as ephemeral rather than as durable as the graph itself.

**How to avoid:**
- Persist crawl state durably and continuously: frontier, in-progress set, per-pubkey freshness/last-fetched metadata all live in the shared DB (or alongside it), not just in memory.
- Make the crawl idempotent and resumable: on startup, reconstruct the work queue from DB state (stale + never-fetched pubkeys) rather than from the anchor.
- Checkpoint frequently enough that a crash loses minutes, not the whole crawl.
- The freshness metadata required by the project doubles as resume state — design them together so "what's stale" and "what's unfetched" come from the same durable source.

**Warning signs:**
Restart triggers a full re-crawl from the anchor; relay request volume spikes after every restart; no DB table answers "what is left to do"; operator afraid to restart the daemon.

**Phase to address:**
State-persistence / scheduling phase, co-designed with the freshness model. Should be in place before the first long crawl, not bolted on after.

---

### Pitfall 7: DB write amplification when re-fetched lists barely change

**What goes wrong:**
Freshness-driven refresh re-fetches lists that have mostly not changed. If each refresh rewrites the entire follow list (delete all edges for the pubkey, re-insert all of them), you generate enormous write churn: hundreds of millions of edge rows rewritten per refresh cycle, index bloat, vacuum/compaction pressure, and a DB that the spam layer is trying to read concurrently. At this scale, full-rewrite-on-refresh can make the DB the bottleneck and degrade read latency for the consumer.

**Why it happens:**
Replaceable-event semantics ("the new event replaces the old list") map naturally to "replace all the edges," and that's correct semantically but disastrous as a write pattern when 99% of refreshes are no-ops or tiny diffs.

**How to avoid:**
- Short-circuit no-op refreshes: if the incoming event `id` equals the stored one, update only the freshness/last-checked timestamp — touch zero edge rows.
- When the list did change, diff against the stored edge set and apply only adds/removes, not a full delete+reinsert.
- Separate the cheap freshness metadata (touched every refresh) from the expensive edge data (touched rarely) so the hot-write path is a tiny row, not the edge table.
- Choose a DB and schema that tolerate this access pattern; this is exactly the kind of edge-volume vs. cross-project-read tradeoff PROJECT.md flags as needing research.

**Warning signs:**
DB size grows on every refresh cycle even when the graph is stable; write IOPS dominated by refresh; autovacuum/compaction can't keep up; spam-layer read latency climbs during refresh windows.

**Phase to address:**
Storage / refresh phase. The "diff don't replace" and "id-equal short-circuit" rules should be part of the write-path design.

---

### Pitfall 8: Websocket connection churn and silent relay disconnects

**What goes wrong:**
Relay websockets drop silently — the OS/runtime may not surface a network-level disconnect, so the daemon thinks a subscription is alive while no events flow and no EOSE ever comes. Conversely, churn (reconnect storms after a relay blip) hammers relays and trips their rate limits (Pitfall 3). Either way, parts of the crawl silently stall: pubkeys "in flight" on a dead connection never complete and, without timeouts, never get retried.

**Why it happens:**
WebSocket libraries don't reliably report disconnects on network failure (a documented nostr crawler issue); developers assume the socket will error if the peer goes away. There's no per-subscription deadline, so a hung sub is indistinguishable from a slow one.

**How to avoid:**
- Application-level heartbeat/keepalive: send pings (or use a quiet-period detector) and close+reconnect a socket that goes silent past a threshold.
- Per-subscription timeout/deadline: if a `REQ` hasn't produced EOSE or events within N seconds, cancel and requeue those pubkeys.
- Reconnect with exponential backoff + jitter and a per-relay circuit breaker; never tight-loop reconnect.
- Treat in-flight pubkeys as "pending with a deadline," not "done," so a dead connection's work returns to the frontier instead of being lost.

**Warning signs:**
Crawl throughput drops to near-zero but no errors logged; a relay shows an open connection but zero events for minutes; "in-flight" count stuck and never draining; reconnect log spam after a relay hiccup.

**Phase to address:**
Relay-connection / transport phase, alongside the politeness work (Pitfall 3).

---

### Pitfall 9: Trusting relay output without verifying signatures (poisoned graph)

**What goes wrong:**
Relays are "dumb" — they do not verify signatures and will store/return whatever they were given, including impersonated or forged events, events you didn't ask for, or events attributed to the wrong author. If the crawler stores edges without verifying the event signature and that the pubkey matches the author, a malicious relay can inject fabricated follow lists, corrupting the very graph the spam layer relies on to defeat spam farms. This is an adversarial setting by design.

**Why it happens:**
The crawler trusts that a relay returning an event for author X means X authored it. The nostr trust model is "don't trust relays, trust cryptography" — verification is the *client's* job, and a crawler is a client.

**How to avoid:**
- Verify every event before storing: recompute the event `id` from the serialized event, verify the secp256k1 signature against the event's pubkey, and confirm the event matches what was requested (kind 3, expected author). Drop anything that fails.
- Don't assume a relay only returns events you asked for; filter responses against your own request.
- Because verification is per-event over hundreds of millions of events, budget for it (batched/parallel verification in Rust) — but never skip it as an "optimization."

**Warning signs:**
Edges from a single relay that no other relay corroborates; events whose recomputed id ≠ claimed id; spam-layer trust results that look manipulated; a relay returning kinds or authors outside the request.

**Phase to address:**
Event-ingestion / validation phase (same gate as Pitfall 1). Signature verification is a hard precondition for storing any edge.

---

## Technical Debt Patterns

| Shortcut | Immediate Benefit | Long-term Cost | When Acceptable |
|----------|-------------------|----------------|-----------------|
| In-memory frontier/visited set | Fast to build, simple BFS | OOM at millions of pubkeys; non-resumable crawl | Only for a hop-limited prototype/spike, never for the real full crawl |
| Full delete+reinsert edges on every refresh | Trivially correct replaceable semantics | Massive write amplification, index bloat, read-latency hit for spam layer | MVP only if refresh volume is tiny; replace with diff before scaling refresh |
| Skip signature verification to go faster | Higher ingest throughput | Poisoned, attacker-controlled graph; undermines the whole anti-spam purpose | Never — this is the adversarial threat the project exists to survive |
| Treat one relay as canonical per pubkey | Simpler fetch/merge logic | Misses newer lists held only on other relays; stale graph | Acceptable transiently if curated relays are known-comprehensive; revisit with NIP-65 fallback |
| Trust EOSE = complete | Simpler fetch loop, no pagination | Silent missing pubkeys; incomplete reachable set | Never for a "complete reachable" requirement |
| No per-sub timeout | Less code | Hung subs silently stall the crawl | Never for an unattended daemon |

## Integration Gotchas

| Integration | Common Mistake | Correct Approach |
|-------------|----------------|------------------|
| Relay `REQ`/filters | Sending many filters per REQ / huge single filter; ignoring `max_limit` | Read NIP-11 limits; chunk authors; one filter per REQ where required; paginate under `max_limit` |
| Relay `CLOSED`/`OK` messages | Ignoring machine-readable prefix, retrying blindly | Branch on `rate-limited`/`blocked`/`restricted`/`duplicate`/`invalid`; back off or drop relay accordingly |
| NIP-11 relay info doc | Not fetching it; assuming uniform limits | Fetch + cache per relay; adapt concurrency/limit/subscription count per relay |
| NIP-65 (kind 10002) fallback | Treating hints as authoritative or always-present; following them blindly | Use as fallback when curated set misses a pubkey; cap fan-out to hinted relays; still verify + paginate |
| WebSocket transport | Assuming socket errors on network loss | App-level heartbeat + silence timeout + reconnect with backoff |
| Shared DB with spam layer | Schema as afterthought; long write locks during refresh | Schema is the public API — version it; keep writes short; avoid blocking concurrent reads |

## Performance Traps

| Trap | Symptoms | Prevention | When It Breaks |
|------|----------|------------|----------------|
| In-RAM visited/frontier | RSS grows unbounded | On-disk dedup + streamed frontier | Hundreds of thousands → millions of pubkeys |
| Full edge rewrite per refresh | Write IOPS spike, DB bloat | id-equal short-circuit; edge diffing | Once refresh cycles cover the full graph |
| Unbounded p-tag lists | One huge row/event stalls pipeline | Cap/validate tag count; backpressure | First follow-list bomb (10k+ p-tags) hit |
| Single giant filter per relay | Relay caps response; truncation | Author-chunked, paginated REQs | When author count per request exceeds relay `max_limit`/cap |
| Synchronous per-event signature verify on hot path | Ingest throughput collapses | Batched/parallel verification, backpressure | Hundreds of millions of events |
| Unbounded reconnect loop | Relay bans, CPU spin | Backoff + jitter + circuit breaker | First relay blip under high concurrency |

## Security Mistakes

| Mistake | Risk | Prevention |
|---------|------|------------|
| Storing edges without verifying event signature/author | Attacker injects fake follow graph via a malicious relay; spam layer defeated | Verify id recompute + secp256k1 sig + author match before any write |
| Accepting future-dated `created_at` | Junk event permanently pins a pubkey's list; denial of updates | Clamp/reject `created_at` beyond a small future window |
| No bound on p-tag count or event size | Memory/storage DoS from a single crafted kind-3 event | Validate and cap tag count and event size on ingest |
| Following NIP-65 hints without limits | Adversary directs crawler to arbitrary/abusive relays | Cap relays-per-pubkey from hints; allowlist/denylist; verify results |
| Enqueuing every discovered pubkey | Spam islands inflate the crawl, waste goodwill | Only enqueue pubkeys reachable from the trusted set (scope rule) |

## UX Pitfalls

(Operator-facing — this is unattended infrastructure, the "user" is the single operator.)

| Pitfall | User Impact | Better Approach |
|---------|-------------|-----------------|
| No coverage/staleness/relay-health metrics | Operator can't tell if the graph is current or the daemon is silently stalled; can't trust it unattended | Ship observability in v1 (PROJECT.md requires it): coverage %, staleness distribution, per-relay health, frontier/in-flight sizes |
| Silent failures (stalled subs, banned relays) logged at debug or not at all | Operator discovers a dead crawl days later | Alert on rate-limited/blocked spikes, throughput cliffs, stuck in-flight counts |
| No "what's left to do" visibility | Operator can't estimate crawl completion or diagnose | Expose frontier size, unfetched count, stale count from durable state |

## "Looks Done But Isn't" Checklist

- [ ] **Replaceable-event handling:** Often missing the future-`created_at` clamp and the same-timestamp lowest-id tie-break — verify both with crafted junk events.
- [ ] **Completeness:** Often treats EOSE as done — verify pagination actually triggers when a relay caps the response, and coverage metric reflects real per-author hit rate.
- [ ] **Signature verification:** Often skipped "for speed" — verify a forged event from a test relay is rejected, not stored.
- [ ] **Resumability:** Often non-resumable — verify a kill -9 mid-crawl resumes from durable state, not from the anchor.
- [ ] **Refresh write path:** Often full-rewrites — verify a no-op refresh (same event id) touches zero edge rows.
- [ ] **Disconnect handling:** Often assumes sockets error on network loss — verify a silently dropped connection's in-flight pubkeys get requeued, not lost.
- [ ] **Frontier scope:** Often enqueues everything — verify spam-island pubkeys (reachable from no one in the trusted set) are never fetched.
- [ ] **Relay politeness:** Often ignores NIP-11 — verify the daemon reads and respects `max_limit`/`max_subscriptions` and backs off on `rate-limited`.

## Recovery Strategies

| Pitfall | Recovery Cost | Recovery Steps |
|---------|---------------|----------------|
| Poisoned graph (no sig verify) | HIGH | Add verification, then re-crawl / re-verify all stored events; quarantine edges from suspect relays |
| Future-dated junk pinning lists | MEDIUM | Add clamp; re-select newest *valid* event per pinned pubkey; re-fetch affected pubkeys |
| IP banned by key relays | MEDIUM-HIGH | Stop traffic, contact/rotate, add politeness + backoff before resuming; lean on NIP-65 fallback meanwhile |
| Non-resumable crawl discovered late | MEDIUM | Add durable frontier/freshness tables; derive work queue from DB; one more full crawl to seed state |
| Write amplification / DB bloat | MEDIUM | Add id-equal short-circuit + edge diffing; vacuum/compact/rebuild indexes; possibly reschema |
| Frontier OOM | LOW-MEDIUM | Move frontier/visited to disk; add backpressure; restart from durable state |

## Pitfall-to-Phase Mapping

| Pitfall | Prevention Phase | Verification |
|---------|------------------|--------------|
| `created_at` trust / future timestamps | Event ingestion & validation | Crafted future-dated event is rejected/clamped |
| Older-overwrites-newer | Storage / write-path | Conditional write rejects stale event; edges never go backwards |
| Relay rate-limit / ban | Relay connection / transport | NIP-11 respected; backoff on `rate-limited`; concurrency capped |
| EOSE ≠ complete | Fetch / pagination | Pagination triggers on capped responses; coverage metric accurate |
| Frontier explosion / OOM | Frontier scheduling + storage | Full crawl runs within bounded memory; spam islands unvisited |
| Restart loses state | State persistence (with freshness model) | kill -9 resumes from DB, not anchor |
| Write amplification | Storage / refresh | No-op refresh touches zero edge rows; changes applied as diffs |
| Connection churn / silent disconnect | Relay connection / transport | Silent socket requeues in-flight work; heartbeat closes dead sockets |
| Unverified relay output | Event ingestion & validation | Forged event rejected, not stored |
| No observability | Observability (v1, per PROJECT.md) | Operator can read coverage, staleness, relay health, frontier size |

## Sources

- NIP-01 (basic protocol, replaceable events, EOSE, CLOSED/OK prefixes, tie-break rule): https://nips.nostr.com/1 and https://github.com/nostr-protocol/nips/blob/master/01.md — HIGH
- NIP-01 single-filter-per-REQ direction: https://github.com/nostr-protocol/nips/pull/1645 — MEDIUM
- NIP-02 follow list (kind 3 structure, p-tags, replace-whole-list, lost-follows data loss): https://nips.nostr.com/2 and https://nostrbook.dev/kinds/3 — HIGH
- NIP-11 relay information document (max_subscriptions, max_filters, max_limit): https://nips.nostr.com/11 and https://nostr.co.uk/nips/nip-11/ — HIGH
- LIMITS command proposal (per-connection limits): https://github.com/nostr-protocol/nips/pull/1434 — LOW (proposal)
- NIP-67 EOSE completeness hints (finish/more): https://www.e2encrypted.com/nostr/nips/67/ — MEDIUM
- EOSE limit semantics / how limit works: https://github.com/nostr-protocol/nips/issues/233 — MEDIUM
- Real-world lost-follows / kind-3 race conditions: https://stacker.news/items/182519 and https://github.com/nostr-protocol/nips/pull/349 — MEDIUM
- Relays don't verify signatures / trust model: https://github.com/nostr-protocol/nips/issues/554 and https://nessy.info/post/2023-04-18-can-nostr-events-be-manipulated/ — HIGH
- Comparable open crawler (retries/backoff, NIP-10002 discovery, dual store, rank-gated ingest): https://github.com/vertex-lab/crawler_v2 — MEDIUM
- Large-scale nostr empirical study (1.5M pubkeys, 712 relays, replication overhead, availability): https://arxiv.org/html/2402.05709v2 — MEDIUM
- WebSocket silent-disconnect behavior (heartbeat needed): community/tooling notes per search — MEDIUM
- created_at future-timestamp validation practice (reject >60s / >1 day future): community security guidance per search — MEDIUM

---
*Pitfalls research for: nostr social-graph crawler / large-graph ingestion*
*Researched: 2026-06-11*
