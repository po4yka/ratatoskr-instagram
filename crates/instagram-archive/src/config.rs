//! RED-phase skeleton: a permissive loader that ignores every entry and
//! returns defaults. The strictness tests must fail against this, and the
//! real implementation replaces it without changing the public surface.

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use base64::Engine as _;
use secrecy::{ExposeSecret as _, SecretString};
use serde::Serialize;

use crate::credentials::crypto::{CredentialKeyring, KEY_LEN};
use crate::own_media::OwnMediaSyncConfig;

const ENV_PREFIX: &str = "RATATOSKR__";

/// Process configuration with finite built-in limits.
#[derive(Debug, Clone, Serialize)]
pub struct Config {
    /// Operator listener configuration.
    pub admin: AdminConfig,
    /// Product listener configuration for capture intake.
    pub api: ApiConfig,
    /// Owned durable storage configuration.
    pub storage: StorageConfig,
    /// Telemetry pipeline configuration.
    pub telemetry: TelemetryConfig,
    /// Resource and shutdown limits.
    pub limits: Limits,
    /// Outbox publisher loop configuration.
    pub publisher: PublisherConfig,
    /// Disabled-by-default official Instagram OAuth configuration.
    pub oauth: OAuthConfig,
    /// Disabled-by-default connected-account own-media scheduler.
    pub own_media: OwnMediaSyncConfig,
    /// `JetStream` command-consumer configuration.
    pub bus: Option<BusConfig>,
}

/// The broker identity used only by the command consumer.
#[derive(Debug, Clone, Serialize)]
pub struct BusConfig {
    /// A credential-free `nats://` or `tls://` endpoint.
    pub url: String,
    /// Optional absolute file containing the role's NATS nkey seed.
    pub nkey_seed_path: Option<PathBuf>,
}

/// Official Instagram Login configuration.
#[derive(Clone, Serialize)]
pub struct OAuthConfig {
    /// Whether OAuth command routes are enabled.
    pub enabled: bool,
    /// Meta Instagram application id.
    pub client_id: Option<String>,
    /// Meta Instagram application secret.
    #[serde(skip_serializing)]
    pub client_secret: Option<SecretString>,
    /// Platform-owned public callback URI.
    pub redirect_uri: Option<String>,
    /// Loopback Platform code-relay base URI.
    pub platform_relay_url: Option<String>,
    /// Narrow audience-bound relay credential.
    #[serde(skip_serializing)]
    pub platform_relay_token: Option<SecretString>,
    /// Explicit Graph API path version.
    pub graph_version: String,
    /// Key version selected for new envelopes.
    pub current_key_version: Option<u32>,
    /// Versioned base64 AES-256 keys.
    #[serde(skip_serializing)]
    pub keyring: Option<SecretString>,
    /// TCP/TLS connection timeout.
    pub connect_timeout_ms: u64,
    /// Per-request read timeout.
    pub request_timeout_ms: u64,
    /// End-to-end provider operation deadline.
    pub total_timeout_ms: u64,
    /// Largest accepted provider JSON body.
    pub max_response_bytes: usize,
    /// Additional discovery attempts after the first.
    pub discovery_retries: u32,
    /// Maximum provider attempts in one operation.
    pub call_budget: u32,
    /// Pending flow lifetime.
    pub flow_ttl_seconds: u64,
}

impl std::fmt::Debug for OAuthConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OAuthConfig")
            .field("enabled", &self.enabled)
            .field("client_id", &self.client_id)
            .field("client_secret", &"[REDACTED]")
            .field("redirect_uri", &self.redirect_uri)
            .field("platform_relay_url", &self.platform_relay_url)
            .field("platform_relay_token", &"[REDACTED]")
            .field("graph_version", &self.graph_version)
            .field("current_key_version", &self.current_key_version)
            .field("keyring", &"[REDACTED]")
            .field("connect_timeout_ms", &self.connect_timeout_ms)
            .field("request_timeout_ms", &self.request_timeout_ms)
            .field("total_timeout_ms", &self.total_timeout_ms)
            .field("max_response_bytes", &self.max_response_bytes)
            .field("discovery_retries", &self.discovery_retries)
            .field("call_budget", &self.call_budget)
            .field("flow_ttl_seconds", &self.flow_ttl_seconds)
            .finish()
    }
}

/// Loopback-only operator listener configuration.
#[derive(Debug, Clone, Serialize)]
pub struct AdminConfig {
    /// Socket address for health, metrics, and version routes.
    pub listen_address: SocketAddr,
}

/// Loopback-only product listener configuration.
///
/// The capture intake serves the platform's service-to-service channel; user
/// authentication lives in `ratatoskr-platform`, so this listener defaults to
/// loopback and refuses every other binding until that boundary moves.
#[derive(Debug, Clone, Serialize)]
pub struct ApiConfig {
    /// Socket address for the capture intake routes.
    pub listen_address: SocketAddr,
}

/// `PostgreSQL` storage locations owned by this service.
#[derive(Clone, Serialize)]
pub struct StorageConfig {
    /// Archive `PostgreSQL` connection URL. Absent until configured; there is
    /// deliberately no default that is not either wrong or a secret in the
    /// source tree.
    #[serde(skip_serializing)]
    pub database_url: Option<SecretString>,
}

impl std::fmt::Debug for StorageConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StorageConfig")
            .field("database_url", &"[REDACTED]")
            .finish()
    }
}

/// Telemetry pipeline configuration.
#[derive(Debug, Clone, Serialize)]
pub struct TelemetryConfig {
    /// Structured log filter expression.
    pub log_filter: String,
}

/// Finite limits used by the process foundation.
#[derive(Debug, Clone, Serialize)]
pub struct Limits {
    /// Maximum database connections.
    pub database_connections: u32,
    /// Maximum wait for a database connection.
    pub database_acquire_timeout_ms: u64,
    /// Maximum graceful shutdown duration.
    pub shutdown_timeout_ms: u64,
}

/// Outbox publisher loop configuration. The loop only runs when storage is
/// configured; without a database there is nothing to publish.
#[derive(Debug, Clone, Serialize)]
pub struct PublisherConfig {
    /// Milliseconds between publisher passes.
    pub poll_interval_ms: u64,
    /// Maximum facts claimed per pass.
    pub batch_size: u32,
}

/// One configuration violation. The offending key and the rule it broke, and
/// never the supplied value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// The environment variable key.
    pub key: String,
    /// The rule the value violated.
    pub rule: &'static str,
}

/// Configuration loading failure carrying every violation found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    /// Every violation found, in first-seen order.
    pub violations: Vec<Violation>,
}

impl ConfigError {
    fn new(key: &str, rule: &'static str) -> Self {
        Self {
            violations: vec![Violation {
                key: key.to_owned(),
                rule,
            }],
        }
    }
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "configuration is invalid")?;
        for violation in &self.violations {
            write!(formatter, "\n  {} {}", violation.key, violation.rule)?;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Loads the current process environment.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] carrying every violation found.
    pub fn load() -> Result<Self, ConfigError> {
        let mut entries = Vec::new();
        for (key, value) in std::env::vars_os() {
            let Some(key) = key.into_string().ok() else {
                continue;
            };
            if !key.starts_with(ENV_PREFIX) {
                continue;
            }
            let Ok(value) = value.into_string() else {
                return Err(ConfigError::new(&key, "must contain Unicode text"));
            };
            entries.push((key, value));
        }

        Self::from_environment(entries)
    }

    /// Loads configuration from prefixed environment entries.
    ///
    /// Every entry under [`ENV_PREFIX`] must name a known key and carry a
    /// valid value; nothing is silently ignored. All entries are examined so
    /// one load reports every violation found, never only the first.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] carrying every violation found.
    pub fn from_environment<I, K, V>(entries: I) -> Result<Self, ConfigError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let mut violations = Vec::new();
        let mut config = Self::default();
        for (key, value) in entries {
            let key = key.as_ref();
            if !key.starts_with(ENV_PREFIX) {
                continue;
            }
            apply_entry(&mut config, key, value.as_ref(), &mut violations);
        }
        validate_oauth(&config.oauth, &mut violations);
        validate_own_media(&config, &mut violations);

        if config.bus.as_ref().is_none_or(|bus| bus.url.is_empty()) {
            violations.push(Violation {
                key: "RATATOSKR__BUS__URL".to_owned(),
                rule: "must configure the mandatory JetStream command-consumer endpoint",
            });
        }

        if violations.is_empty() {
            Ok(config)
        } else {
            Err(ConfigError { violations })
        }
    }
}

impl OAuthConfig {
    /// Decodes the validated keyring for credential encryption.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if called on an invalid enabled configuration.
    pub fn credential_keyring(&self) -> Result<Option<CredentialKeyring>, ConfigError> {
        if !self.enabled {
            return Ok(None);
        }
        let current = self.current_key_version.ok_or_else(|| {
            ConfigError::new(
                "RATATOSKR__OAUTH__CURRENT_KEY_VERSION",
                "must name a key present in the OAuth keyring",
            )
        })?;
        let encoded = self.keyring.as_ref().ok_or_else(|| {
            ConfigError::new(
                "RATATOSKR__OAUTH__KEYRING",
                "must contain versioned base64 AES-256 keys",
            )
        })?;
        let keys = decode_keyring(encoded.expose_secret())
            .map_err(|rule| ConfigError::new("RATATOSKR__OAUTH__KEYRING", rule))?;
        CredentialKeyring::new(current, keys)
            .map(Some)
            .map_err(|_| {
                ConfigError::new(
                    "RATATOSKR__OAUTH__CURRENT_KEY_VERSION",
                    "must name a key present in the OAuth keyring",
                )
            })
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "one closed configuration section reports every independent violation in one pass"
)]
fn validate_oauth(config: &OAuthConfig, violations: &mut Vec<Violation>) {
    if !config.enabled {
        return;
    }
    let mut require = |present: bool, key: &'static str, rule: &'static str| {
        if !present {
            violations.push(Violation {
                key: key.to_owned(),
                rule,
            });
        }
    };
    require(
        config
            .client_id
            .as_deref()
            .is_some_and(|value| !value.is_empty() && value.len() <= 128 && value.is_ascii()),
        "RATATOSKR__OAUTH__CLIENT_ID",
        "must be non-empty bounded ASCII text",
    );
    require(
        config.client_secret.is_some(),
        "RATATOSKR__OAUTH__CLIENT_SECRET",
        "is required when OAuth is enabled",
    );
    require(
        config.platform_relay_token.is_some(),
        "RATATOSKR__OAUTH__PLATFORM_RELAY_TOKEN",
        "is required when OAuth is enabled",
    );
    require(
        config.current_key_version.is_some(),
        "RATATOSKR__OAUTH__CURRENT_KEY_VERSION",
        "is required when OAuth is enabled",
    );
    require(
        config.keyring.is_some(),
        "RATATOSKR__OAUTH__KEYRING",
        "is required when OAuth is enabled",
    );

    validate_https_uri(
        config.redirect_uri.as_deref(),
        "RATATOSKR__OAUTH__REDIRECT_URI",
        violations,
    );
    validate_https_uri(
        config.platform_relay_url.as_deref(),
        "RATATOSKR__OAUTH__PLATFORM_RELAY_URL",
        violations,
    );
    if config.graph_version != "v26.0" {
        violations.push(Violation {
            key: "RATATOSKR__OAUTH__GRAPH_VERSION".to_owned(),
            rule: "must equal the reviewed provider profile version v26.0",
        });
    }
    if let (Some(current), Some(encoded)) = (config.current_key_version, config.keyring.as_ref()) {
        match decode_keyring(encoded.expose_secret()) {
            Ok(keys) if keys.contains_key(&current) => {}
            Ok(_) => violations.push(Violation {
                key: "RATATOSKR__OAUTH__CURRENT_KEY_VERSION".to_owned(),
                rule: "must name a key present in the OAuth keyring",
            }),
            Err(rule) => violations.push(Violation {
                key: "RATATOSKR__OAUTH__KEYRING".to_owned(),
                rule,
            }),
        }
    }
    validate_range(
        config.connect_timeout_ms,
        100,
        10_000,
        "RATATOSKR__OAUTH__CONNECT_TIMEOUT_MS",
        violations,
    );
    validate_range(
        config.request_timeout_ms,
        100,
        30_000,
        "RATATOSKR__OAUTH__REQUEST_TIMEOUT_MS",
        violations,
    );
    validate_range(
        config.total_timeout_ms,
        config.request_timeout_ms,
        60_000,
        "RATATOSKR__OAUTH__TOTAL_TIMEOUT_MS",
        violations,
    );
    validate_range(
        config.max_response_bytes as u64,
        1_024,
        1_048_576,
        "RATATOSKR__OAUTH__MAX_RESPONSE_BYTES",
        violations,
    );
    validate_range(
        u64::from(config.discovery_retries),
        0,
        2,
        "RATATOSKR__OAUTH__DISCOVERY_RETRIES",
        violations,
    );
    validate_range(
        u64::from(config.call_budget),
        3,
        10,
        "RATATOSKR__OAUTH__CALL_BUDGET",
        violations,
    );
    validate_range(
        config.flow_ttl_seconds,
        60,
        900,
        "RATATOSKR__OAUTH__FLOW_TTL_SECONDS",
        violations,
    );
}

fn validate_own_media(config: &Config, violations: &mut Vec<Violation>) {
    if !config.own_media.enabled {
        return;
    }
    if !config.oauth.enabled {
        violations.push(Violation {
            key: "RATATOSKR__OWN_MEDIA__ENABLED".to_owned(),
            rule: "requires the reviewed OAuth account lane to be enabled",
        });
    }
    validate_range(
        config.own_media.cadence_seconds,
        60,
        86_400,
        "RATATOSKR__OWN_MEDIA__CADENCE_SECONDS",
        violations,
    );
    validate_range(
        u64::from(config.own_media.accounts_per_tick),
        1,
        100,
        "RATATOSKR__OWN_MEDIA__ACCOUNTS_PER_TICK",
        violations,
    );
    validate_range(
        u64::from(config.own_media.pages_per_run),
        1,
        100,
        "RATATOSKR__OWN_MEDIA__PAGES_PER_RUN",
        violations,
    );
    validate_range(
        u64::from(config.own_media.call_budget),
        1,
        100,
        "RATATOSKR__OWN_MEDIA__CALL_BUDGET",
        violations,
    );
}

fn validate_https_uri(value: Option<&str>, key: &str, violations: &mut Vec<Violation>) {
    let valid = value
        .and_then(|value| reqwest::Url::parse(value).ok())
        .is_some_and(|url| {
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none()
        });
    if !valid {
        violations.push(Violation {
            key: key.to_owned(),
            rule: "must be an HTTPS URI without credentials or fragment",
        });
    }
}

fn validate_range(
    value: u64,
    minimum: u64,
    maximum: u64,
    key: &str,
    violations: &mut Vec<Violation>,
) {
    if !(minimum..=maximum).contains(&value) {
        violations.push(Violation {
            key: key.to_owned(),
            rule: "must be within its finite documented range",
        });
    }
}

fn decode_keyring(encoded: &str) -> Result<BTreeMap<u32, [u8; KEY_LEN]>, &'static str> {
    if encoded.is_empty() {
        return Err("must contain versioned base64 AES-256 keys");
    }
    let mut keys = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for entry in encoded.split(',') {
        let Some((version, encoded_key)) = entry.split_once(':') else {
            return Err("must contain version:key entries");
        };
        let version = version
            .parse::<u32>()
            .ok()
            .filter(|version| *version > 0)
            .ok_or("must contain positive integer key versions")?;
        if !seen.insert(version) {
            return Err("must not repeat a key version");
        }
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded_key)
            .map_err(|_| "must contain valid base64 key material")?;
        let key = <[u8; KEY_LEN]>::try_from(decoded)
            .map_err(|_| "each decoded key must contain exactly 32 bytes")?;
        keys.insert(version, key);
    }
    Ok(keys)
}

#[allow(
    clippy::too_many_lines,
    reason = "the strict environment-key inventory is intentionally one exhaustive match"
)]
fn apply_entry(config: &mut Config, key: &str, value: &str, violations: &mut Vec<Violation>) {
    let refused = |rule: &'static str| Violation {
        key: key.to_owned(),
        rule,
    };
    match key {
        "RATATOSKR__ADMIN__LISTEN_ADDRESS" => match value.parse::<SocketAddr>() {
            Ok(address) if address.ip().is_loopback() && address.port() != 0 => {
                config.admin.listen_address = address;
            }
            Ok(_) => violations.push(refused("must be a loopback address with a port")),
            Err(_) => violations.push(refused("must be a socket address")),
        },
        "RATATOSKR__API__LISTEN_ADDRESS" => match value.parse::<SocketAddr>() {
            Ok(address) if address.ip().is_loopback() && address.port() != 0 => {
                config.api.listen_address = address;
            }
            Ok(_) => violations.push(refused("must be a loopback address with a port")),
            Err(_) => violations.push(refused("must be a socket address")),
        },
        "RATATOSKR__STORAGE__DATABASE_URL" => {
            match value.parse::<sqlx::postgres::PgConnectOptions>() {
                Ok(_) => {
                    config.storage.database_url = Some(SecretString::from(value));
                }
                Err(_) => violations.push(refused(
                    "must be a PostgreSQL connection URL naming user, password, host, and database",
                )),
            }
        }
        "RATATOSKR__TELEMETRY__LOG_FILTER" => {
            if value.trim().is_empty() {
                violations.push(refused("must be a non-empty tracing filter expression"));
            } else {
                value.clone_into(&mut config.telemetry.log_filter);
            }
        }
        "RATATOSKR__LIMITS__DATABASE_CONNECTIONS" => match parse_positive::<u32>(value) {
            Ok(parsed) => config.limits.database_connections = parsed,
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__LIMITS__DATABASE_ACQUIRE_TIMEOUT_MS" => match parse_positive::<u64>(value) {
            Ok(parsed) => config.limits.database_acquire_timeout_ms = parsed,
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__LIMITS__SHUTDOWN_TIMEOUT_MS" => match parse_positive::<u64>(value) {
            Ok(parsed) => config.limits.shutdown_timeout_ms = parsed,
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__PUBLISHER__POLL_INTERVAL_MS" => match parse_positive::<u64>(value) {
            Ok(parsed) => config.publisher.poll_interval_ms = parsed,
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__PUBLISHER__BATCH_SIZE" => match parse_positive::<u32>(value) {
            Ok(parsed) => config.publisher.batch_size = parsed,
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__OAUTH__ENABLED" => match value {
            "true" => config.oauth.enabled = true,
            "false" => config.oauth.enabled = false,
            _ => violations.push(refused("must be true or false")),
        },
        "RATATOSKR__OAUTH__CLIENT_ID" => config.oauth.client_id = Some(value.to_owned()),
        "RATATOSKR__OAUTH__CLIENT_SECRET" => {
            config.oauth.client_secret = Some(SecretString::from(value));
        }
        "RATATOSKR__OAUTH__REDIRECT_URI" => config.oauth.redirect_uri = Some(value.to_owned()),
        "RATATOSKR__OAUTH__PLATFORM_RELAY_URL" => {
            config.oauth.platform_relay_url = Some(value.to_owned());
        }
        "RATATOSKR__OAUTH__PLATFORM_RELAY_TOKEN" => {
            config.oauth.platform_relay_token = Some(SecretString::from(value));
        }
        "RATATOSKR__OAUTH__GRAPH_VERSION" => value.clone_into(&mut config.oauth.graph_version),
        "RATATOSKR__OAUTH__CURRENT_KEY_VERSION" => match parse_positive::<u32>(value) {
            Ok(parsed) => config.oauth.current_key_version = Some(parsed),
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__OAUTH__KEYRING" => {
            config.oauth.keyring = Some(SecretString::from(value));
        }
        "RATATOSKR__OAUTH__CONNECT_TIMEOUT_MS" => match parse_positive::<u64>(value) {
            Ok(parsed) => config.oauth.connect_timeout_ms = parsed,
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__OAUTH__REQUEST_TIMEOUT_MS" => match parse_positive::<u64>(value) {
            Ok(parsed) => config.oauth.request_timeout_ms = parsed,
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__OAUTH__TOTAL_TIMEOUT_MS" => match parse_positive::<u64>(value) {
            Ok(parsed) => config.oauth.total_timeout_ms = parsed,
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__OAUTH__MAX_RESPONSE_BYTES" => match parse_positive::<usize>(value) {
            Ok(parsed) => config.oauth.max_response_bytes = parsed,
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__OAUTH__DISCOVERY_RETRIES" => match value.parse::<u32>() {
            Ok(parsed) => config.oauth.discovery_retries = parsed,
            Err(_) => violations.push(refused("must be a non-negative integer")),
        },
        "RATATOSKR__OAUTH__CALL_BUDGET" => match parse_positive::<u32>(value) {
            Ok(parsed) => config.oauth.call_budget = parsed,
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__OAUTH__FLOW_TTL_SECONDS" => match parse_positive::<u64>(value) {
            Ok(parsed) => config.oauth.flow_ttl_seconds = parsed,
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__OWN_MEDIA__ENABLED" => match value {
            "true" => config.own_media.enabled = true,
            "false" => config.own_media.enabled = false,
            _ => violations.push(refused("must be true or false")),
        },
        "RATATOSKR__OWN_MEDIA__CADENCE_SECONDS" => match parse_positive::<u64>(value) {
            Ok(parsed) => config.own_media.cadence_seconds = parsed,
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__OWN_MEDIA__ACCOUNTS_PER_TICK" => match parse_positive::<u32>(value) {
            Ok(parsed) => config.own_media.accounts_per_tick = parsed,
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__OWN_MEDIA__PAGES_PER_RUN" => match parse_positive::<u32>(value) {
            Ok(parsed) => config.own_media.pages_per_run = parsed,
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__OWN_MEDIA__CALL_BUDGET" => match parse_positive::<u32>(value) {
            Ok(parsed) => config.own_media.call_budget = parsed,
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__BUS__URL" => {
            if matches!(value.split("://").next(), Some("nats" | "tls")) && !value.contains('@') {
                let bus = config.bus.get_or_insert_with(default_bus);
                value.clone_into(&mut bus.url);
            } else {
                violations.push(refused("must be a credential-free nats:// or tls:// URL"));
            }
        }
        "RATATOSKR__BUS__NKEY_SEED_PATH" => {
            let path = PathBuf::from(value);
            if path.is_absolute() {
                let bus = config.bus.get_or_insert_with(default_bus);
                bus.nkey_seed_path = Some(path);
            } else {
                violations.push(refused("must be an absolute readable seed-file path"));
            }
        }
        _ => violations.push(refused("is not recognized")),
    }
}

fn parse_positive<T>(value: &str) -> Result<T, &'static str>
where
    T: std::str::FromStr + Default + PartialOrd,
{
    let parsed = value
        .parse::<T>()
        .map_err(|_| "must be a positive integer")?;
    if parsed <= T::default() {
        return Err("must be a positive integer");
    }
    Ok(parsed)
}

impl Default for Config {
    fn default() -> Self {
        Self {
            admin: AdminConfig {
                listen_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9082),
            },
            api: ApiConfig {
                listen_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9083),
            },
            storage: StorageConfig { database_url: None },
            telemetry: TelemetryConfig {
                log_filter: "info".to_owned(),
            },
            limits: Limits {
                database_connections: 8,
                database_acquire_timeout_ms: 5_000,
                shutdown_timeout_ms: 10_000,
            },
            publisher: PublisherConfig {
                poll_interval_ms: 1_000,
                batch_size: 16,
            },
            oauth: OAuthConfig {
                enabled: false,
                client_id: None,
                client_secret: None,
                redirect_uri: None,
                platform_relay_url: None,
                platform_relay_token: None,
                graph_version: "v26.0".to_owned(),
                current_key_version: None,
                keyring: None,
                connect_timeout_ms: 3_000,
                request_timeout_ms: 10_000,
                total_timeout_ms: 20_000,
                max_response_bytes: 256 * 1024,
                discovery_retries: 1,
                call_budget: 5,
                flow_ttl_seconds: 600,
            },
            own_media: OwnMediaSyncConfig {
                enabled: false,
                cadence_seconds: 3_600,
                accounts_per_tick: 8,
                pages_per_run: 8,
                call_budget: 8,
            },
            bus: None,
        }
    }
}

fn default_bus() -> BusConfig {
    BusConfig {
        url: String::new(),
        nkey_seed_path: None,
    }
}
