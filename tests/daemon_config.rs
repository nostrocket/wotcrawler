//! Daemon configuration load + validation (filled in 04-02).
//!
//! Wave 0 scaffold: named `#[ignore]` stubs that plan 04-02 fills once the
//! `daemon::config` struct, TOML+env layered load, and fail-fast `validate()`
//! exist. Until then they are listed but skipped so `cargo test --test
//! daemon_config -- --list` shows the target shape.

mod common;

/// CLI/env overrides take precedence over the TOML config file, which in turn
/// overrides the `DEFAULT_*` consts.
#[tokio::test]
#[ignore = "filled in 04-02"]
async fn override_precedence() {
    unimplemented!("04-02: layered config precedence (env > file > DEFAULT_*)");
}

/// Omitted fields fall back to the existing `DEFAULT_*` constants.
#[tokio::test]
#[ignore = "filled in 04-02"]
async fn default_fill() {
    unimplemented!("04-02: omitted fields default to DEFAULT_* consts");
}

/// An unparseable anchor pubkey (hex/bech32) fails validation at startup.
#[tokio::test]
#[ignore = "filled in 04-02"]
async fn invalid_anchor_rejected() {
    unimplemented!("04-02: invalid anchor pubkey rejected by validate()");
}

/// A TTL of zero is rejected (FRESH-02 requires TTL > 0).
#[tokio::test]
#[ignore = "filled in 04-02"]
async fn ttl_zero_rejected() {
    unimplemented!("04-02: TTL = 0 rejected by validate()");
}
