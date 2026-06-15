---
phase: 05-nip-65-outbox-routing-relay-health
reviewed: 2026-06-15T00:00:00Z
depth: standard
files_reviewed: 16
files_reviewed_list:
  - migrations/0004_pubkey_relays.sql
  - src/ingest/relay_list.rs
  - src/ingest/mod.rs
  - src/store/relays.rs
  - src/store/mod.rs
  - src/relay/health.rs
  - src/relay/fetch.rs
  - src/relay/mod.rs
  - src/crawl/apply.rs
  - src/crawl/mod.rs
  - src/daemon/mod.rs
  - src/daemon/loop_.rs
  - src/daemon/config.rs
  - src/daemon/observe.rs
  - src/daemon/sampler.rs
  - config.example.toml
  - ops/grafana-dashboard.json
findings:
  critical: 3
  warning: 4
  info: 2
  total: 9
status: issues_found
---

# Phase 5: Code Review Report

**Reviewed:** 2026-06-15T00:00:00Z
**Depth:** standard
**Files Reviewed:** 16
**Status:** issues_found

## Summary

Phase 5 adds NIP-65 write-relay fallback, per-relay EWMA health scoring, and a
per-relay concurrency admission gate on top of the existing crawler. The
architecture is sound and the security-critical path (verify-before-dedup
ordering, single ingest pass over the cross-relay union, full gate reuse for
adversarial write relays) is correctly implemented. However, three blockers and
four warnings are present that require attention before the daemon runs
unattended for multi-day periods.

The most serious issues are: (1) a busy-spin in `admit_per_relay` that holds
the per-relay semaphore permit while yielding in a tight loop, which can
deadlock the tokio runtime under saturation; (2) `route_allowed` reading and
then `mark_attempt` writing without atomicity, producing a race where every
concurrent task for the same degraded relay sees "probe due" at the same moment
and all probe simultaneously; and (3) `fallback_fetch` in `daemon/mod.rs`
bypasses the per-relay semaphore and health routing that the main fan-out uses,
meaning write-relay fetches are neither health-gated nor concurrency-admitted.

---

## Critical Issues

### CR-01: Busy-spin in `admit_per_relay` holds semaphore permit across `yield_now` calls — potential starvation / livelock

**File:** `src/relay/health.rs:269-273`

**Issue:** After acquiring the hard semaphore permit (line 261), the function
enters a `loop { if in_use < permits { break; } tokio::task::yield_now().await;
}` spin. The semaphore permit is held for the entire duration of this spin. If
`per_relay_concurrency` concurrent tasks all hold their semaphore permits and
are waiting for `in_use` to drop, none of them can make progress: the
`in_use` counter is only decremented in `decr_in_use` which is called from the
`InUseGuard` drop — which only fires after a fetch completes — but the semaphore
is already exhausted so no new permits are available to let *other* fetches
complete. Under full saturation this is a live-lock; the semaphore size equals
`per_relay_concurrency` and the `in_use` ceiling also equals
`per_relay_concurrency`, so the admission gate above the semaphore never fires
because the semaphore below it is always fully occupied by spinners.

Additionally, `tokio::task::yield_now()` yields only once and re-polls
immediately. On a CPU-bound tokio runtime with many relays, this tight yield
loop gives other tasks very little opportunity to run, starving the tasks whose
`InUseGuard` drops would unblock the spinners.

**Fix:** Acquire the semaphore permit only after the `in_use` check passes, not
before it. One clean approach:

```rust
pub async fn admit_per_relay<F, Fut, T, E>(
    health: &RelayHealthRegistry,
    sem: &Arc<Semaphore>,
    relay: &str,
    per_relay_concurrency: usize,
    fetch: F,
) -> Result<T, E>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    // Wait until health-scaled admission allows entry, then acquire the
    // hard semaphore. No permit is held across the poll loop.
    loop {
        if health.in_use(relay) < health.permits(relay, per_relay_concurrency) as u32 {
            break;
        }
        tokio::task::yield_now().await;
    }

    // Now acquire the hard cap. Held only for the fetch, not the poll loop.
    let _permit = sem
        .acquire()
        .await
        .expect("per-relay semaphore is never closed");

    health.incr_in_use(relay);
    struct InUseGuard<'a> { health: &'a RelayHealthRegistry, relay: &'a str }
    impl Drop for InUseGuard<'_> {
        fn drop(&mut self) { self.health.decr_in_use(self.relay); }
    }
    let _guard = InUseGuard { health, relay };

    fetch().await
}
```

Alternatively, replace `yield_now` with a small `tokio::time::sleep` (e.g.
1 ms) to reduce CPU spin pressure during saturation:
```rust
tokio::time::sleep(Duration::from_millis(1)).await;
```

---

### CR-02: `route_allowed` / `mark_attempt` check-then-act race — multiple concurrent tasks all probe a degraded relay simultaneously

**File:** `src/relay/health.rs:193-218`, `src/daemon/mod.rs:256-259`

**Issue:** The call site in `daemon/mod.rs` is:
```rust
if !health.route_allowed(relay_url, relay_health_threshold) {
    continue;
}
health.mark_attempt(relay_url);
```

`route_allowed` reads `last_probe` under one `Mutex` lock; `mark_attempt`
writes `last_probe` under a second, separate `Mutex` lock (a different field).
Between the two calls the locks are both released. If `concurrency` tasks are
all iterating the curated relay list simultaneously (which is the normal
operating mode — the semaphore allows up to `concurrency` batches in flight),
every task that evaluates `route_allowed` for the same degraded relay sees the
probe as "due" (because `mark_attempt` has not fired yet), and every task calls
`mark_attempt`. The result is that all concurrent tasks probe the degraded relay
simultaneously, defeating the purpose of the probe mechanism and potentially
flooding the relay with requests at exactly the moment it is degraded.

In the worst case, with `concurrency = 8` and a single degraded relay, all 8
concurrent batches probe it within the same iteration. If the relay is
degraded, all 8 probes fail, sampling `0.0` eight times, driving the score
further toward zero faster than intended.

**Fix:** Combine the check and the mark into a single atomic operation inside
`RelayHealthRegistry`:

```rust
/// Returns true and marks the attempt atomically if routing is allowed.
/// Uses a single lock scope to avoid the check-then-act race.
pub fn try_mark_attempt(&self, relay: &str, threshold: f64) -> bool {
    let score = self.score(relay); // separate read (already lock-free enough)
    if score >= threshold {
        self.mark_attempt(relay);
        return true;
    }
    // Below threshold: check and mark atomically under the probe lock.
    let mut last = self.last_probe.lock().expect("health probe map not poisoned");
    let due = match last.get(relay) {
        None => true,
        Some(t) => t.elapsed() >= PROBE_INTERVAL,
    };
    if due {
        last.insert(relay.to_string(), Instant::now());
    }
    due
}
```

Replace the two-call pattern at every call site with `try_mark_attempt`.

---

### CR-03: `fallback_fetch` in `daemon/mod.rs` bypasses per-relay semaphore and health routing

**File:** `src/daemon/mod.rs:314-344`

**Issue:** The `fallback_fetch` closure calls
`fetch::fetch_complete_with_timeout` directly on NIP-65 write relay URLs
without going through `admit_per_relay` (the per-relay semaphore + in_use
admission gate) and without any `route_allowed` / health check. The same
omission applies to `relay_list_fetch` (lines 349-381). The comments in
`daemon/mod.rs` acknowledge that these closures "reuse the SAME GCRA-gated
per-relay fetch path" but note they "do NOT duplicate the health
routing/admission of the fan-out (the fan-out is the only routing site)."

The consequence for a multi-day unattended run:
1. Write relays that are degraded or rate-limiting will receive unbounded
   concurrent fallback requests because there is no per-relay semaphore for
   write relays.
2. `in_use` gauges for write relay URLs will stay at zero (never incremented),
   so the health registry never observes their actual concurrency.
3. Since `per_relay_sems` is only populated for curated relays
   (`cfg.relays.iter()`), a write-relay URL not in the curated set has no
   semaphore entry — `per_relay_sems.get(relay_url)` would panic
   (`expect("every curated relay has a per-relay semaphore")`), but since the
   fallback closures do NOT call `admit_per_relay`, the panic path is never
   reached; instead the fallback simply has no concurrency cap at all.

GCRA rate limiting still applies (via `registry`), but unlimited concurrent
fallback fetches to the same write relay URL can still overwhelm it before the
rate limiter fires (the GCRA limiter is per-request, not per-concurrency).

**Fix:** The simplest safe approach is to cap fallback concurrency via a shared
`Semaphore` sized at `nip65_max_write_relays` (already the fan-out cap), then
wrap each write-relay fetch in the GCRA gate that `fetch_complete_with_timeout`
already applies. For write relays not in the curated set, construct or reuse a
per-URL semaphore from a separate `HashMap<String, Arc<Semaphore>>` initialized
in the fallback closure's outer scope, paralleling `per_relay_sems`:

```rust
// In the fallback_fetch closure setup, add:
let fallback_sems: Arc<Mutex<HashMap<String, Arc<Semaphore>>>> =
    Arc::new(Mutex::new(HashMap::new()));

// Inside fallback_fetch, per write relay:
let sem = {
    let mut map = fallback_sems.lock().unwrap();
    Arc::clone(map.entry(relay_url.clone())
        .or_insert_with(|| Arc::new(Semaphore::new(per_relay_concurrency))))
};
// Then admit_per_relay with the write-relay sem + health.
```

Alternatively, a simpler fix is a single shared fallback semaphore capped at
`nip65_max_write_relays` across all concurrent fallbacks, which bounds the
total write-relay concurrency without per-URL granularity.

---

## Warnings

### WR-01: `admits_per_relay` spin loop — unbounded spin with `yield_now` instead of sleep can starve other tasks under load

**File:** `src/relay/health.rs:269-273`

**Issue:** Even if CR-01 is resolved (by moving the semaphore acquisition after
the spin), the remaining `yield_now` spin loop has no sleep. `yield_now` yields
to the tokio scheduler exactly once and immediately re-polls. Under saturation
— when many tasks are spinning simultaneously — this becomes effectively a busy
loop on the tokio worker threads, reducing throughput for all other tasks
(DB writes, timer callbacks, health sampling). This is especially relevant for a
long-running daemon where relay degradation is expected to be transient.

**Fix:** Add a short sleep inside the loop to allow genuine cooperative
multitasking:
```rust
loop {
    if health.in_use(relay) < health.permits(relay, per_relay_concurrency) as u32 {
        break;
    }
    tokio::time::sleep(Duration::from_millis(1)).await;
}
```

Note: this warning is secondary to CR-01; fix CR-01 first.

---

### WR-02: `fallback_recover` calls `lookup_write_relays` with `claimed.id` but the pubkey_id may differ from what was just upserted

**File:** `src/crawl/apply.rs:316`, `src/crawl/apply.rs:331`

**Issue:** In `fallback_recover`, after the on-demand kind:10002 fetch path:

```rust
if write_relays.is_empty() {
    if let Ok(raw) = relay_list_fetch(author).await {
        if let Some(vrl) = resolve_relay_list(raw, author, now, future_clamp_secs) {
            let pubkey_id = upsert_pubkey(pool, &author.to_bytes()).await?;
            apply_relay_list(pool, pubkey_id, &vrl.relays, vrl.created_at).await?;
        }
    }
    write_relays = lookup_write_relays(pool, claimed.id).await?;  // line 331
}
```

`apply_relay_list` stores rows keyed by the `pubkey_id` returned from
`upsert_pubkey`. `lookup_write_relays` queries using `claimed.id` (the surrogate
id already stored in the `ClaimedAuthor`). These should be the same value —
`upsert_pubkey` returns the existing id if the pubkey already exists. However,
if `upsert_pubkey` is ever invoked for the first time at this point (i.e. the
pubkey is known to the crawler only via a `ClaimedAuthor` row that somehow
pre-dates an explicit `upsert_pubkey` call for this exact author), the
`claimed.id` from the row and the freshly upserted `pubkey_id` will match as
long as `upsert_pubkey` returns the existing id on conflict. This is currently
safe because `claimed.id` must already exist (the row was claimed), but the code
obscures this invariant.

The more concrete risk: if `upsert_pubkey` returns a DIFFERENT id than
`claimed.id` due to a schema or upsert-path bug in a future migration,
`apply_relay_list` stores against `pubkey_id` while `lookup_write_relays`
queries against `claimed.id`, and the relay list is silently stored but never
found. There is no assertion that the two ids match.

**Fix:** Use the return value of `upsert_pubkey` for the subsequent
`lookup_write_relays` call, making the id provenance explicit:

```rust
if write_relays.is_empty() {
    if let Ok(raw) = relay_list_fetch(author).await {
        if let Some(vrl) = resolve_relay_list(raw, author, now, future_clamp_secs) {
            let pubkey_id = upsert_pubkey(pool, &author.to_bytes()).await?;
            apply_relay_list(pool, pubkey_id, &vrl.relays, vrl.created_at).await?;
            write_relays = lookup_write_relays(pool, pubkey_id).await?;
            // Skip the second lookup below — we just persisted into pubkey_id.
        }
    }
    if write_relays.is_empty() {
        write_relays = lookup_write_relays(pool, claimed.id).await?;
    }
}
```

---

### WR-03: `handle_relay_message` fires `record_notice` for `Blocked` without health degradation — inconsistency with `RateLimited`

**File:** `src/relay/mod.rs:276-280`

**Issue:** In `handle_relay_message`:

```rust
NoticeKind::Blocked => {
    let _ = registry.record_notice(relay_url, message);
    // NOTE: health is NOT degraded for Blocked
}
```

A `Blocked` notice escalates the rate-limiter's consecutive-failure count
(via `record_notice`) but does NOT degrade the EWMA health score. A `Blocked`
relay is therefore still routed to (score stays at previous level, potentially
healthy), meaning the crawler will keep attempting fetches from it even though
it has explicitly blocked the crawler. The `RateLimited` arm correctly degrades
the health score (sample 0.2), which eventually routes the relay below the
threshold.

A relay that sends `blocked` is signaling it does not want the crawler's
traffic at all — it is a harder stop signal than `rate-limited`. The health
score should be degraded at least as aggressively as for `rate-limited`
(sample 0.2) or more aggressively (sample 0.0).

**Fix:**
```rust
NoticeKind::Blocked => {
    let _ = registry.record_notice(relay_url, message);
    // Blocked is a harder stop signal than rate-limited; degrade health
    // at least as aggressively so routing skips it.
    health.record_connect_failure(relay_url); // sample 0.0
}
```

---

### WR-04: `paginate_chunk_gated` acquires the GCRA token AFTER the fetch future is created — rate limiter may not gate the actual network send

**File:** `src/relay/fetch.rs:220-226`

**Issue:** In `paginate_chunk_gated`:
```rust
paginate_chunk(authors, kind, cap, move |filter| {
    let fut = fetch(filter);      // fetch future created HERE (before rate-limit)
    async move {
        registry.acquire(relay_url).await?;  // rate-limit acquired HERE
        fut.await                             // future polled HERE
    }
})
```

`fetch(filter)` is called before `registry.acquire()`. For the SDK's
`client.fetch_events`, creating the future does not immediately send the
network request — the request is sent when the future is first polled. Since
`fut.await` comes after `registry.acquire()`, the actual poll (and thus the
network REQ) is gated correctly. However, this pattern is subtle and fragile:
if `fetch` is ever replaced with a function that has an eager side effect on
construction (e.g. immediately enqueuing a subscription), the GCRA gate would
no longer protect the actual network send.

With the current `nostr-sdk` `client.fetch_events` implementation this is
functionally correct, but the ordering is non-obvious and could silently regress
if the fetch function changes.

**Fix:** Move the future creation after the rate-limit acquisition:
```rust
paginate_chunk(authors, kind, cap, move |filter| {
    async move {
        registry.acquire(relay_url).await?;
        fetch(filter).await   // future created and polled after rate-limit
    }
})
```

Note: this requires `fetch` to be `FnMut(Filter) -> Fut` with `Fut: Future`
(which it already is), so `fetch(filter)` inside the `async move` block is
valid and borrows `fetch` correctly for each call.

---

## Info

### IN-01: `staleness_ages` histogram is sampled only from the oldest 1000 rows — may systematically underrepresent fresh fetches

**File:** `src/daemon/sampler.rs:186-200`

**Issue:** The staleness-age histogram query uses `ORDER BY last_fetched_at ASC
LIMIT 1000`, meaning it always samples the 1000 *oldest* rows. At multi-million
pubkey scale, this means the histogram only ever shows the stalest tail of the
distribution — the freshly-fetched majority of the population is invisible. The
Grafana staleness panel will look permanently alarming (all samples at the high
end of the age buckets) even when the crawl is healthy.

A random or representative sample (e.g. `ORDER BY random() LIMIT 1000` or a
tablesample) would give the operator a more accurate picture of the actual
staleness distribution.

**Fix:** Replace `ORDER BY last_fetched_at ASC` with a representative sample:
```sql
SELECT EXTRACT(EPOCH FROM (now() - last_fetched_at))::double precision AS age_secs
FROM pubkeys
WHERE status IN ('fetched','not_found','failed')
  AND last_fetched_at IS NOT NULL
ORDER BY random()
LIMIT 1000
```

Note that `ORDER BY random()` performs a sequential scan on large tables;
`TABLESAMPLE SYSTEM (0.1)` with a `WHERE` guard may be preferable at scale.

---

### IN-02: `config.example.toml` contains a real-looking anchor pubkey with default credentials

**File:** `config.example.toml:12`, `config.example.toml:24`

**Issue:** The example config ships with:
- `anchor_pubkey = "82341f882b6eabcd2ba7f1ef90aad961cf074af15b9ef44a09f9d2a8fbfbe6a2"` — a real nostr pubkey (Jack Dorsey's well-known key). While not a secret, using a real person's pubkey as the default may cause unintended crawl behavior if an operator forgets to change it.
- `database_url = "postgres://crawler:changeme@localhost:5432/web_of_trust"` — a hardcoded default password. While `changeme` is an obvious placeholder, the comment says "NEVER logged" but does not flag that this specific value must be changed before use. The comment says "prefer supplying it via `WOT__DATABASE_URL` in production", which is correct but easy to overlook.

**Fix:** Replace the anchor pubkey with an obviously fake placeholder:
```toml
anchor_pubkey = "REPLACE_WITH_YOUR_ANCHOR_PUBKEY_HEX_OR_NPUB"
```

And add an explicit `# CHANGE THIS` marker on the database_url line to make
it harder to deploy accidentally with the default password.

---

_Reviewed: 2026-06-15T00:00:00Z_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
