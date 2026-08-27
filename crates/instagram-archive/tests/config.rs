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

fn complete_oauth_environment() -> Vec<(&'static str, &'static str)> {
    vec![
        ("RATATOSKR__BUS__URL", "nats://127.0.0.1:4222"),
        ("RATATOSKR__OAUTH__ENABLED", "true"),
        ("RATATOSKR__OAUTH__CLIENT_ID", "123456789"),
        ("RATATOSKR__OAUTH__CLIENT_SECRET", "synthetic-client-secret"),
        (
            "RATATOSKR__OAUTH__REDIRECT_URI",
            "https://platform.example.test/v1/oauth/callback/instagram",
        ),
        (
            "RATATOSKR__OAUTH__PLATFORM_RELAY_URL",
            "https://platform.example.test/v1/oauth/relay",
        ),
        (
            "RATATOSKR__OAUTH__PLATFORM_RELAY_TOKEN",
            "synthetic-relay-token",
        ),
        ("RATATOSKR__OAUTH__GRAPH_VERSION", "v26.0"),
        ("RATATOSKR__OAUTH__CURRENT_KEY_VERSION", "7"),
        (
            "RATATOSKR__OAUTH__KEYRING",
            "7:QkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkI=",
        ),
    ]
}

#[test]
fn oauth_disabled_accepts_missing_secrets() {
    let config = Config::from_environment([
        ("RATATOSKR__BUS__URL", "nats://127.0.0.1:4222"),
        ("RATATOSKR__OAUTH__ENABLED", "false"),
    ])
    .expect("disabled OAuth needs no provider credentials");
    assert!(!config.oauth.enabled);
    assert!(config.oauth.client_secret.is_none());
    assert!(config.oauth.keyring.is_none());
}

#[test]
fn oauth_enabled_requires_complete_bounded_configuration() {
    let error = Config::from_environment([("RATATOSKR__OAUTH__ENABLED", "true")])
        .expect_err("enabled OAuth must fail closed without every secret and binding");
    assert!(error.to_string().contains("OAUTH"), "{error}");

    let mut excessive = complete_oauth_environment();
    excessive.push(("RATATOSKR__OAUTH__CALL_BUDGET", "999999"));
    assert!(Config::from_environment(excessive).is_err());
}

#[test]
fn oauth_keyring_rejects_invalid_or_missing_current_version_without_echo() {
    for keyring in [
        "7:not-base64",
        "8:QkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkI=",
    ] {
        let mut entries = complete_oauth_environment();
        if let Some((_, value)) = entries
            .iter_mut()
            .find(|(key, _)| *key == "RATATOSKR__OAUTH__KEYRING")
        {
            *value = keyring;
        }
        let error = Config::from_environment(entries).expect_err("invalid keyring is refused");
        let rendered = error.to_string();
        assert!(rendered.contains("KEYRING") || rendered.contains("CURRENT_KEY_VERSION"));
        assert!(
            !rendered.contains(keyring),
            "key material leaked: {rendered}"
        );
    }
}

#[test]
fn oauth_effective_config_omits_all_secret_fields() {
    let config = Config::from_environment(complete_oauth_environment())
        .expect("complete synthetic OAuth config loads");
    let serialized = serde_json::to_string(&config).expect("effective config serializes");
    for secret in [
        "synthetic-client-secret",
        "synthetic-relay-token",
        "QkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkI=",
    ] {
        assert!(!serialized.contains(secret), "secret leaked: {serialized}");
    }
}

#[test]
fn production_provider_hosts_cannot_be_overridden() {
    let mut entries = complete_oauth_environment();
    entries.push(("RATATOSKR__OAUTH__PROVIDER_BASE_URL", "http://127.0.0.1:9"));
    let error = Config::from_environment(entries).expect_err("production hosts stay fixed");
    assert!(error.to_string().contains("PROVIDER_BASE_URL"));
}

#[test]
fn own_media_scheduler_is_disabled_by_default_and_rejects_unbounded_limits() {
    let defaults = Config::from_environment([("RATATOSKR__BUS__URL", "nats://127.0.0.1:4222")])
        .expect("configuration with the mandatory bus loads");
    assert!(!defaults.own_media.enabled);

    let mut entries = complete_oauth_environment();
    entries.extend([
        ("RATATOSKR__OWN_MEDIA__ENABLED", "true"),
        ("RATATOSKR__OWN_MEDIA__CADENCE_SECONDS", "999999"),
        ("RATATOSKR__OWN_MEDIA__ACCOUNTS_PER_TICK", "999999"),
        ("RATATOSKR__OWN_MEDIA__PAGES_PER_RUN", "999999"),
        ("RATATOSKR__OWN_MEDIA__CALL_BUDGET", "999999"),
    ]);
    let error = Config::from_environment(entries)
        .expect_err("enabled own-media scheduling requires finite reviewed limits");
    assert!(error.to_string().contains("OWN_MEDIA"));
}
