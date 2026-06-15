//! Daemon configuration: the single source of truth every other daemon task
//! consumes (OPS-01).
//!
//! [`Config`] is deserialized from a TOML file (`--config <path>`) with a
//! `WOT__*` environment-variable overlay (double-underscore nesting). Every
//! optional field defaults to the existing `DEFAULT_*` constant it mirrors —
//! the defaults reference those consts by name so the daemon and the
//! library can never drift (`crawl::DEFAULT_*`, `relay::rate_limit::DEFAULT_*`,
//! `relay::fetch::DEFAULT_FETCH_TIMEOUT`).
//!
//! [`validate`] is fail-fast: a bad anchor pubkey, an empty relay set, an empty
//! or non-URL-looking `database_url`, or a non-positive TTL all return an
//! actionable [`anyhow::Error`] *before* any crawl work begins (OPS-01
//! fail-fast; threat T-04-04). The authoritative `database_url` check is the
//! `PgPool` connect at startup.
//!
//! Security (T-04-03 / T-03-04): `database_url` may carry a password and is
//! NEVER logged. `Config`'s `Debug` is hand-implemented to redact it; use that
//! redacted form for any config-echo logging.

use std::fmt;
use std::net::SocketAddr;
use std::time::Duration;

use serde::Deserialize;

use crate::crawl::{DEFAULT_BATCH_SIZE, DEFAULT_CONCURRENCY, DEFAULT_MAX_ATTEMPTS};
use crate::relay::fetch::DEFAULT_FETCH_TIMEOUT;
use crate::relay::rate_limit::DEFAULT_REQS_PER_SECOND;

/// Log output format selectable via config (OBS-02). Human-readable by default;
/// JSON for log shipping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// Human-readable `tracing_subscriber::fmt` output (default).
    #[default]
    Human,
    /// Structured JSON lines for log shipping.
    Json,
}

/// The full daemon tunable set (OPS-01).
///
/// Required fields (`anchor_pubkey`, `relays`, `database_url`, `ttl`,
/// `metrics_addr`) must be present in the TOML/env; every other field defaults
/// to its `DEFAULT_*` const so a minimal config is enough.
#[derive(Clone, Deserialize)]
pub struct Config {
    /// Anchor pubkey the crawl starts from (CRAWL-01), hex or bech32 `npub`.
    pub anchor_pubkey: String,
    /// Curated relay set the crawler fetches follow lists from. Must be non-empty.
    pub relays: Vec<String>,
    /// Postgres connection URL. NEVER logged (T-04-03); redacted in `Debug`.
    pub database_url: String,
    /// Uniform staleness TTL (FRESH-02): rows past this age are re-enqueued.
    #[serde(with = "humantime_serde")]
    pub ttl: Duration,
    /// In-flight batch-fetch concurrency cap (CRAWL-04).
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    /// Authors claimed per worker batch (D-07).
    #[serde(default = "default_batch_size")]
    pub batch_size: i64,
    /// Transient-error fetch attempts before a pubkey is marked `failed` (D-09).
    #[serde(default = "default_max_attempts")]
    pub max_attempts: i16,
    /// Per-fetch deadline.
    #[serde(with = "humantime_serde", default = "default_fetch_timeout")]
    pub fetch_timeout: Duration,
    /// Sustained per-relay outbound REQ rate (requests/second).
    #[serde(default = "default_reqs_per_second")]
    pub reqs_per_second: u32,
    /// Bind address for the axum `/metrics` + `/health/*` server (OBS-01/OBS-03).
    pub metrics_addr: SocketAddr,
    /// `EnvFilter`/`RUST_LOG`-style log level (OBS-02).
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// Log output format (OBS-02).
    #[serde(default)]
    pub log_format: LogFormat,
    /// Interval between periodic crawl-progress summaries (OBS-04).
    #[serde(with = "humantime_serde", default = "default_progress_interval")]
    pub progress_interval: Duration,
    /// Interval between TTL staleness scans (FRESH-02).
    #[serde(with = "humantime_serde", default = "default_staleness_scan_interval")]
    pub staleness_scan_interval: Duration,
    /// Interval between in-run stale-lease reclaim sweeps (OPS-02).
    #[serde(with = "humantime_serde", default = "default_reclaim_interval")]
    pub reclaim_interval: Duration,
    /// Age threshold for the in-run reclaim sweep: only `in_progress` leases
    /// older than this are reclaimed, so freshly-claimed live leases are left
    /// untouched (OPS-02; never resets in-flight work).
    #[serde(with = "humantime_serde", default = "default_reclaim_age")]
    pub reclaim_age: Duration,
    /// Poll/sleep interval when the frontier is empty (continuous loop idle).
    #[serde(with = "humantime_serde", default = "default_idle_poll_interval")]
    pub idle_poll_interval: Duration,
}

// Default fns reference the existing library consts by name — never re-literal
// the numbers, so the daemon and library can never drift.
fn default_concurrency() -> usize {
    DEFAULT_CONCURRENCY
}
fn default_batch_size() -> i64 {
    DEFAULT_BATCH_SIZE
}
fn default_max_attempts() -> i16 {
    DEFAULT_MAX_ATTEMPTS
}
fn default_fetch_timeout() -> Duration {
    DEFAULT_FETCH_TIMEOUT
}
fn default_reqs_per_second() -> u32 {
    DEFAULT_REQS_PER_SECOND
}
fn default_log_level() -> String {
    "info".to_string()
}
fn default_progress_interval() -> Duration {
    Duration::from_secs(60)
}
fn default_staleness_scan_interval() -> Duration {
    Duration::from_secs(300)
}
fn default_reclaim_interval() -> Duration {
    Duration::from_secs(60)
}
fn default_reclaim_age() -> Duration {
    // Comfortably above the default fetch timeout so a live, freshly-claimed
    // lease is never mistaken for an orphan.
    Duration::from_secs(300)
}
fn default_idle_poll_interval() -> Duration {
    Duration::from_secs(5)
}

/// Hand-implemented `Debug` that REDACTS `database_url` (T-04-03 / T-03-04).
///
/// The DB URL may embed a password; this is the only `Debug` for `Config`, so
/// config-echo logging (`tracing::info!(?config, ...)`) can never leak it.
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("anchor_pubkey", &self.anchor_pubkey)
            .field("relays", &self.relays)
            .field("database_url", &"<redacted>")
            .field("ttl", &self.ttl)
            .field("concurrency", &self.concurrency)
            .field("batch_size", &self.batch_size)
            .field("max_attempts", &self.max_attempts)
            .field("fetch_timeout", &self.fetch_timeout)
            .field("reqs_per_second", &self.reqs_per_second)
            .field("metrics_addr", &self.metrics_addr)
            .field("log_level", &self.log_level)
            .field("log_format", &self.log_format)
            .field("progress_interval", &self.progress_interval)
            .field("staleness_scan_interval", &self.staleness_scan_interval)
            .field("reclaim_interval", &self.reclaim_interval)
            .field("reclaim_age", &self.reclaim_age)
            .field("idle_poll_interval", &self.idle_poll_interval)
            .finish()
    }
}

/// Load the daemon config from `path` (a TOML file) with a `WOT__*` environment
/// overlay (double-underscore nesting). Env vars override file values, which
/// override the `DEFAULT_*`-backed defaults.
///
/// Returns a deserialization error if a required field is missing or a value is
/// malformed; call [`validate`] afterward for the fail-fast semantic checks.
pub fn load_config(path: &str) -> anyhow::Result<Config> {
    let cfg: Config = config::Config::builder()
        .add_source(config::File::with_name(path))
        .add_source(config::Environment::default().prefix("WOT").separator("__"))
        .build()?
        .try_deserialize()?;
    Ok(cfg)
}

/// Fail-fast semantic validation (OPS-01; threat T-04-04). Returns an
/// actionable error on a bad anchor pubkey, an empty relay set, an empty /
/// non-URL-looking `database_url`, a non-positive TTL, or a non-positive
/// `concurrency` / `batch_size` / `reqs_per_second` — before any crawl work
/// begins. The authoritative `database_url` check is the `PgPool` connect.
///
/// The numeric guards matter for fail-fast (OPS-01): `concurrency == 0` makes
/// `Semaphore::new(0)` deadlock the loop forever (it never closes); a negative
/// or zero `batch_size` makes the first `claim_batch` `LIMIT` a Postgres error;
/// and `reqs_per_second == 0` makes the rate-limiter `NonZeroU32` build fail.
/// All three are checked here so a misconfigured daemon dies at startup with an
/// actionable message, never after DB/relay/loop setup.
pub fn validate(c: &Config) -> anyhow::Result<()> {
    // Anchor: accept hex or bech32 `npub` (PublicKey::parse handles both).
    nostr_sdk::PublicKey::parse(&c.anchor_pubkey)
        .map_err(|e| anyhow::anyhow!("invalid anchor_pubkey: {e}"))?;
    anyhow::ensure!(!c.relays.is_empty(), "relays must be non-empty");
    anyhow::ensure!(c.ttl > Duration::ZERO, "ttl must be > 0");
    anyhow::ensure!(
        !c.database_url.trim().is_empty(),
        "database_url must be non-empty"
    );
    // Cheap shape check; the PgPool connect at startup is authoritative.
    anyhow::ensure!(
        c.database_url.contains("://"),
        "database_url must look like a URL (scheme://...)"
    );
    // Numeric fail-fast guards (OPS-01): each would otherwise hang or crash the
    // loop after expensive setup rather than at startup.
    anyhow::ensure!(c.concurrency > 0, "concurrency must be > 0");
    anyhow::ensure!(c.batch_size > 0, "batch_size must be > 0");
    anyhow::ensure!(c.reqs_per_second > 0, "reqs_per_second must be > 0");
    Ok(())
}
