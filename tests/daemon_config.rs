//! Daemon config load / override / default / validation tests (OPS-01, 04-02).
//!
//! Pure-unit (no DB): each test writes a small TOML to a unique tempfile under
//! `std::env::temp_dir()` and exercises `load_config` + `validate`. The
//! `override_precedence` test mutates a process env var, so the WHOLE suite must
//! run single-threaded:
//!
//! ```text
//! SQLX_OFFLINE=true cargo test --test daemon_config -- --test-threads=1
//! ```

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use web_of_trust::daemon::config::{load_config, validate, LogFormat};

/// A real, valid hex pubkey (the nostr "01" test key x-only coordinate) — accepted
/// by `PublicKey::parse`. Used as the anchor in the happy-path TOMLs.
const VALID_ANCHOR: &str = "82341f882b6eabcd2ba7f1ef90aad961cf074af15b9ef44a09f9d2a8fbfbe6a2";

static SEQ: AtomicU64 = AtomicU64::new(0);

/// Write `body` to a unique `*.toml` file under the OS temp dir and return its
/// path. `config::File::with_name` strips the extension, so the path is built
/// without the trailing `.toml` for the loader while the file on disk keeps it.
struct TempToml {
    /// Full on-disk path including `.toml`.
    disk: PathBuf,
    /// Path passed to `load_config` (extension stripped — `config` adds it).
    stem: String,
}

impl TempToml {
    fn new(body: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let stem = std::env::temp_dir().join(format!("wot_daemon_config_{}_{n}", std::process::id()));
        let disk = stem.with_extension("toml");
        let mut f = std::fs::File::create(&disk).expect("create temp config");
        f.write_all(body.as_bytes()).expect("write temp config");
        f.flush().expect("flush temp config");
        TempToml {
            disk,
            stem: stem.to_string_lossy().into_owned(),
        }
    }
}

impl Drop for TempToml {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.disk);
    }
}

/// A minimal TOML with only the required fields, leaving every optional field to
/// its `DEFAULT_*`-backed default.
fn minimal_toml() -> String {
    format!(
        r#"
anchor_pubkey = "{VALID_ANCHOR}"
relays = ["wss://relay.example.com"]
database_url = "postgres://user:pw@localhost/wot"
ttl = "24h"
metrics_addr = "127.0.0.1:9100"
"#
    )
}

/// Omitted optional fields fall back to the existing `DEFAULT_*` constants.
#[test]
fn default_fill() {
    let tmp = TempToml::new(&minimal_toml());
    let cfg = load_config(&tmp.stem).expect("minimal config loads");
    validate(&cfg).expect("minimal config is valid");

    assert_eq!(cfg.concurrency, 8, "concurrency defaults to DEFAULT_CONCURRENCY");
    assert_eq!(cfg.batch_size, 64, "batch_size defaults to DEFAULT_BATCH_SIZE");
    assert_eq!(cfg.max_attempts, 3, "max_attempts defaults to DEFAULT_MAX_ATTEMPTS");
    assert_eq!(cfg.reqs_per_second, 4, "reqs_per_second defaults to DEFAULT_REQS_PER_SECOND");
    assert_eq!(cfg.log_level, "info", "log_level defaults to info");
    assert_eq!(cfg.log_format, LogFormat::Human, "log_format defaults to Human");
}

/// `WOT__*` env vars override the TOML file (env beats file).
#[test]
fn override_precedence() {
    let tmp = TempToml::new(&minimal_toml());

    // SAFETY: the suite runs single-threaded (`--test-threads=1`), so no other
    // test observes these env mutations.
    unsafe {
        std::env::set_var("WOT__CONCURRENCY", "16");
        std::env::set_var("WOT__LOG_FORMAT", "json");
    }
    let cfg = load_config(&tmp.stem).expect("config loads with env overlay");
    unsafe {
        std::env::remove_var("WOT__CONCURRENCY");
        std::env::remove_var("WOT__LOG_FORMAT");
    }

    assert_eq!(cfg.concurrency, 16, "env WOT__CONCURRENCY overrides file/default");
    assert_eq!(cfg.log_format, LogFormat::Json, "env WOT__LOG_FORMAT overrides default");
}

/// An unparseable anchor pubkey fails `validate()` with an "anchor" message.
#[test]
fn invalid_anchor_rejected() {
    let body = minimal_toml().replace(VALID_ANCHOR, "not-a-key");
    let tmp = TempToml::new(&body);
    let cfg = load_config(&tmp.stem).expect("config with bad anchor still deserializes");
    let err = validate(&cfg).expect_err("bad anchor must fail validation");
    assert!(
        err.to_string().contains("anchor"),
        "error should mention anchor, got: {err}"
    );
}

/// A TTL of zero is rejected (FRESH-02 requires TTL > 0).
#[test]
fn ttl_zero_rejected() {
    let body = minimal_toml().replace(r#"ttl = "24h""#, r#"ttl = "0s""#);
    let tmp = TempToml::new(&body);
    let cfg = load_config(&tmp.stem).expect("config with zero ttl deserializes");
    let err = validate(&cfg).expect_err("ttl = 0 must fail validation");
    assert!(
        err.to_string().contains("ttl"),
        "error should mention ttl, got: {err}"
    );
}

/// The committed `config.example.toml` template loads and validates — it is the
/// operator's starting point, so a broken example is a correctness defect.
#[test]
fn example_config_is_valid() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/config.example");
    let cfg = load_config(path).expect("config.example.toml loads");
    validate(&cfg).expect("config.example.toml is valid");
}

/// An empty relay set is rejected.
#[test]
fn empty_relays_rejected() {
    let body = minimal_toml().replace(r#"relays = ["wss://relay.example.com"]"#, "relays = []");
    let tmp = TempToml::new(&body);
    let cfg = load_config(&tmp.stem).expect("config with empty relays deserializes");
    let err = validate(&cfg).expect_err("empty relays must fail validation");
    assert!(
        err.to_string().contains("relays"),
        "error should mention relays, got: {err}"
    );
}
