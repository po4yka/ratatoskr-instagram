//! Configuration strictness: the executable form of the service-runtime spec.

use secrecy::ExposeSecret as _;

use ratatoskr_instagram_archive::{Config, StorageConfig};

#[test]
fn missing_bus_configuration_is_refused() {
    let error = Config::from_environment(Vec::<(String, String)>::new())
        .expect_err("a service that owns a command consumer needs a broker endpoint");

    assert!(error.to_string().contains("RATATOSKR__BUS__URL"));
}

#[test]
fn api_listen_address_override_is_honored_and_non_loopback_refused() {
    let config = Config::from_environment([
        ("RATATOSKR__API__LISTEN_ADDRESS", "127.0.0.1:9183"),
        ("RATATOSKR__BUS__URL", "nats://127.0.0.1:4222"),
    ])
    .expect("a loopback override must load");
    assert_eq!(config.api.listen_address.to_string(), "127.0.0.1:9183");

    let error = Config::from_environment([
        ("RATATOSKR__API__LISTEN_ADDRESS", "10.0.0.9:9183"),
        ("RATATOSKR__BUS__URL", "nats://127.0.0.1:4222"),
    ])
    .expect_err("the product listener is loopback-only like the operator one");
    let rendered = error.to_string();
    assert!(
        rendered.contains("RATATOSKR__API__LISTEN_ADDRESS"),
        "{rendered}"
    );
    assert!(rendered.contains("loopback"), "{rendered}");
    assert!(!rendered.contains("10.0.0.9"), "values never render");
}

#[test]
fn unknown_prefixed_key_is_refused_naming_the_key() {
    let error = Config::from_environment([
        ("RATATOSKR__NOT_A_SECTION__VALUE", "1"),
        ("RATATOSKR__BUS__URL", "nats://127.0.0.1:4222"),
    ])
    .expect_err("an unknown key must be refused");

    let rendered = error.to_string();
    assert!(
        rendered.contains("RATATOSKR__NOT_A_SECTION__VALUE"),
        "the report must name the offending key: {rendered}"
    );
    assert!(
        !rendered.contains('1'),
        "the report must not echo the supplied value: {rendered}"
    );
}

#[test]
fn multiple_violations_are_reported_together_without_values() {
    let error = Config::from_environment([
        ("RATATOSKR__ADMIN__LISTEN_ADDRESS", "10.0.0.1:9082"),
        ("RATATOSKR__LIMITS__DATABASE_CONNECTIONS", "0"),
        ("RATATOSKR__BUS__URL", "nats://127.0.0.1:4222"),
    ])
    .expect_err("two independent violations must both be refused");

    assert_eq!(
        error.violations.len(),
        2,
        "both violations must be reported together: {error}"
    );
    let rendered = error.to_string();
    assert!(rendered.contains("RATATOSKR__ADMIN__LISTEN_ADDRESS"));
    assert!(rendered.contains("loopback"));
    assert!(rendered.contains("RATATOSKR__LIMITS__DATABASE_CONNECTIONS"));
    assert!(rendered.contains("must be a positive integer"));
    assert!(!rendered.contains("10.0.0.1"), "values must never render");
    assert!(
        !rendered.contains('0'),
        "the supplied value leaked into the report"
    );
}

#[test]
fn malformed_database_url_is_refused() {
    let error = Config::from_environment([
        ("RATATOSKR__STORAGE__DATABASE_URL", "not a url at all"),
        ("RATATOSKR__BUS__URL", "nats://127.0.0.1:4222"),
    ])
    .expect_err("a malformed database URL must be refused");

    assert!(error.to_string().contains("PostgreSQL"));
}

#[test]
fn recognized_override_changes_exactly_its_own_field() {
    let config = Config::from_environment([
        ("RATATOSKR__LIMITS__DATABASE_CONNECTIONS", "3"),
        (
            "RATATOSKR__STORAGE__DATABASE_URL",
            "postgres://instagram:instagram@127.0.0.1:5436/instagram",
        ),
        ("RATATOSKR__BUS__URL", "nats://127.0.0.1:4222"),
    ])
    .expect("valid overrides must load");

    assert_eq!(config.limits.database_connections, 3);
    assert_eq!(config.limits.shutdown_timeout_ms, 10_000);
    let url = config
        .storage
        .database_url
        .as_ref()
        .expect("the override must set the database URL");
    assert_eq!(
        url.expose_secret(),
        "postgres://instagram:instagram@127.0.0.1:5436/instagram"
    );
}

#[test]
fn debug_rendering_of_storage_redacts_the_database_url() {
    let config = Config {
        storage: StorageConfig {
            database_url: Some(secrecy::SecretString::from(
                "postgres://instagram:hunter2@127.0.0.1:5436/instagram",
            )),
        },
        ..Config::default()
    };

    let rendered = format!("{config:?}");
    assert!(rendered.contains("[REDACTED]"), "{rendered}");
    assert!(
        !rendered.contains("hunter2"),
        "the secret leaked into Debug: {rendered}"
    );
}
