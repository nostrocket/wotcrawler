//! `crawler` daemon binary entry point (OPS-01).
//!
//! Parses `--config <path>` (clap derive), loads + validates the daemon
//! [`Config`](web_of_trust::daemon::config::Config), and — only if validation
//! passes — hands off to [`web_of_trust::daemon::run`].
//!
//! # Fail-fast (OPS-01 / threat T-04-04)
//!
//! A missing/malformed config file or a config that fails semantic
//! [`validate`](web_of_trust::daemon::config::validate) (bad anchor pubkey,
//! empty relay set, empty/non-URL `database_url`, non-positive TTL) prints an
//! actionable message to stderr and exits NON-ZERO **before any crawl work, DB
//! connection, or relay traffic begins**. The `database_url` is NEVER printed
//! (T-04-13) — config-load/validate errors are surfaced without echoing the URL,
//! and the daemon's only DB-URL use is the `store::connect` call inside `run`.

use std::process::ExitCode;

use clap::Parser;

use web_of_trust::daemon::{self, config};

/// Command-line arguments for the `crawler` daemon (OPS-01).
#[derive(Debug, Parser)]
#[command(
    name = "crawler",
    about = "Nostr web-of-trust crawler daemon: continuously fetches and refreshes the follow graph from a single anchor pubkey."
)]
struct Args {
    /// Path to the TOML config file (a `WOT__*` environment overlay is applied
    /// on top of it). Required.
    #[arg(long)]
    config: std::path::PathBuf,
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();

    // Load: a missing file or malformed value fails fast, non-zero, before the
    // daemon touches the DB or any relay. The path is converted to a &str for the
    // `config` crate's File source.
    let path = match args.config.to_str() {
        Some(p) => p,
        None => {
            eprintln!("error: --config path is not valid UTF-8");
            return ExitCode::FAILURE;
        }
    };

    let cfg = match config::load_config(path) {
        Ok(cfg) => cfg,
        Err(e) => {
            // The error never contains the database_url (T-04-13); config-load
            // errors reference the offending field/value only.
            eprintln!("error: failed to load config from {path}: {e:#}");
            return ExitCode::FAILURE;
        }
    };

    // Validate: semantic fail-fast (bad anchor, empty relays, bad TTL/URL shape)
    // BEFORE any crawl work begins (OPS-01 / T-04-04).
    if let Err(e) = config::validate(&cfg) {
        eprintln!("error: invalid config: {e:#}");
        return ExitCode::FAILURE;
    }

    // Run the daemon. Tracing is initialized inside `run`, so a run error is
    // logged structurally there and also surfaced to stderr here as a backstop.
    match daemon::run(cfg).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "crawler daemon exited with error");
            eprintln!("error: crawler daemon failed: {e:#}");
            ExitCode::FAILURE
        }
    }
}
