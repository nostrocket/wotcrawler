//! RELAY-02: NIP-11 `limitation` fields parse into the per-relay cache, missing
//! / hostile fields fall back to documented defaults, and the cached `max_limit`
//! is the value the pagination cap will use. Pure parse — no live relay.

use web_of_trust::relay::nip11::{
    limits_from_bytes, limits_from_json, LimitCache, RelayLimits, DEFAULT_MAX_FILTERS,
    DEFAULT_MAX_LIMIT, DEFAULT_MAX_SUBSCRIPTIONS, MAX_ADVERTISED_LIMIT, MAX_NIP11_BYTES,
};

#[test]
fn parses_full_limitation_block() {
    let json = r#"{
        "name": "example relay",
        "limitation": {
            "max_limit": 1000,
            "max_subscriptions": 30,
            "max_filters": 12
        }
    }"#;
    let limits = limits_from_json(json).expect("valid NIP-11 document");
    assert_eq!(limits.max_limit, 1000);
    assert_eq!(limits.max_subscriptions, 30);
    assert_eq!(limits.max_filters, 12);
}

#[test]
fn missing_limitation_block_yields_defaults() {
    let json = r#"{ "name": "minimal relay" }"#;
    let limits = limits_from_json(json).expect("a doc without limitation is still valid");
    assert_eq!(limits, RelayLimits::defaults());
    assert_eq!(limits.max_limit, DEFAULT_MAX_LIMIT);
    assert_eq!(limits.max_subscriptions, DEFAULT_MAX_SUBSCRIPTIONS);
    assert_eq!(limits.max_filters, DEFAULT_MAX_FILTERS);
}

#[test]
fn omitted_individual_fields_default_per_field() {
    // limitation present but only max_subscriptions set: the other two default.
    let json = r#"{ "limitation": { "max_subscriptions": 7 } }"#;
    let limits = limits_from_json(json).expect("partial limitation is valid");
    assert_eq!(limits.max_subscriptions, 7);
    assert_eq!(limits.max_limit, DEFAULT_MAX_LIMIT);
    assert_eq!(limits.max_filters, DEFAULT_MAX_FILTERS);
}

#[test]
fn non_positive_advertised_values_fall_back_to_defaults() {
    // Adversarial doc: zero / negative limits must NOT become the cap (T-02-13).
    let json = r#"{ "limitation": { "max_limit": 0, "max_subscriptions": -5, "max_filters": -1 } }"#;
    let limits = limits_from_json(json).expect("hostile values are still parseable");
    assert_eq!(limits.max_limit, DEFAULT_MAX_LIMIT);
    assert_eq!(limits.max_subscriptions, DEFAULT_MAX_SUBSCRIPTIONS);
    assert_eq!(limits.max_filters, DEFAULT_MAX_FILTERS);
}

#[test]
fn advertised_max_limit_is_upper_clamped() {
    // A relay advertising an absurd max_limit must NOT produce a cap that large:
    // it is clamped to MAX_ADVERTISED_LIMIT so it cannot defeat count-vs-cap
    // pagination by making one EOSE window look complete (WR-02, Pitfall 1).
    let json = r#"{ "limitation": { "max_limit": 2000000000 } }"#;
    let limits = limits_from_json(json).expect("absurd advertised value is still parseable");
    assert_eq!(
        limits.max_limit, MAX_ADVERTISED_LIMIT,
        "an advertised max_limit above the ceiling is clamped down to MAX_ADVERTISED_LIMIT"
    );
}

#[test]
fn advertised_max_limit_below_ceiling_is_preserved() {
    // A reasonable advertised value under the ceiling is honored as-is.
    let json = r#"{ "limitation": { "max_limit": 1000 } }"#;
    let limits = limits_from_json(json).expect("valid doc");
    assert!(1000 <= MAX_ADVERTISED_LIMIT);
    assert_eq!(limits.max_limit, 1000);
}

#[test]
fn oversized_body_is_rejected_without_parsing() {
    // A hostile relay streams a body larger than MAX_NIP11_BYTES: limits_from_bytes
    // must reject it (T-02-19 memory DoS) rather than buffer/parse it.
    let oversized = vec![b'x'; MAX_NIP11_BYTES + 1];
    let result = limits_from_bytes("wss://hostile.example", &oversized);
    assert!(
        result.is_err(),
        "a body exceeding MAX_NIP11_BYTES must be rejected"
    );
}

#[test]
fn bounded_body_within_limit_parses() {
    // A body at or under the bound is parsed normally through the existing seam.
    let json = br#"{ "limitation": { "max_limit": 1000 } }"#;
    assert!(json.len() <= MAX_NIP11_BYTES);
    let limits = limits_from_bytes("wss://ok.example", json).expect("bounded body parses");
    assert_eq!(limits.max_limit, 1000);
}

#[test]
fn cached_max_limit_is_the_pagination_cap() {
    // The value the pagination planner reads is exactly the cached max_limit.
    let cache = LimitCache::new();
    cache.insert(
        "wss://relay.example",
        RelayLimits {
            max_limit: 250,
            max_subscriptions: 15,
            max_filters: 8,
        },
    );
    let limits = cache.get("wss://relay.example").expect("seeded relay is cached");
    assert_eq!(
        limits.max_limit, 250,
        "the pagination planner caps each filter at the cached max_limit"
    );
    // An unseeded relay is simply absent (fetched lazily by get_or_fetch).
    assert!(cache.get("wss://unknown.example").is_none());
}
