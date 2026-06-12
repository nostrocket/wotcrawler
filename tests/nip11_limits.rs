//! RELAY-02: NIP-11 `limitation` fields parse into the per-relay cache, missing
//! / hostile fields fall back to documented defaults, and the cached `max_limit`
//! is the value the pagination cap will use. Pure parse — no live relay.

use web_of_trust::relay::nip11::{
    limits_from_json, LimitCache, RelayLimits, DEFAULT_MAX_FILTERS, DEFAULT_MAX_LIMIT,
    DEFAULT_MAX_SUBSCRIPTIONS,
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
