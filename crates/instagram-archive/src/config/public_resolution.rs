//! Fail-closed configuration of the public-resolution surface and worker (XR-021 CONTRACTS.md
//! S10 CD5, S02 rule 5).
//!
//! The service owns a broker consumer, so the surface it needs to finish that work must be
//! configured too: [`validate`] refuses a configuration with a bus and no access-token path.

use std::path::PathBuf;

use secrecy::SecretString;
use serde::Serialize;

use super::{ConfigError, Violation, parse_positive, validate_range};

const ENDPOINT_KEY: &str = "RATATOSKR__PUBLIC_RESOLUTION__ENDPOINT";
const TOKEN_PATH_KEY: &str = "RATATOSKR__PUBLIC_RESOLUTION__ACCESS_TOKEN_PATH";

/// The Meta oEmbed endpoint used when none is configured.
const DEFAULT_ENDPOINT: &str = "https://graph.facebook.com/v25.0/instagram_oembed";

/// The only hosts the service will send its app access token to.
const ALLOWED_HOSTS: [&str; 2] = ["graph.facebook.com", "graph.instagram.com"];

/// Largest access-token file the service will read, in bytes.
const MAX_TOKEN_FILE_BYTES: u64 = 4 * 1024;

/// Public-resolution surface and worker settings.
#[derive(Debug, Clone, Serialize)]
pub struct PublicResolutionConfig {
    /// The `instagram_oembed` Graph endpoint: https on a Meta Graph host, no credentials, no
    /// query.
    pub endpoint: String,
    /// Absolute path of the file holding the Meta app access token. Required with a bus.
    pub access_token_path: Option<PathBuf>,
    /// Attempts before a transient failure becomes terminal.
    pub max_attempts: u32,
    /// Captures claimed per worker pass.
    pub batch_size: u32,
    /// Milliseconds between worker passes.
    pub poll_interval_ms: u64,
}

impl Default for PublicResolutionConfig {
    fn default() -> Self {
        Self {
            endpoint: DEFAULT_ENDPOINT.to_owned(),
            access_token_path: None,
            max_attempts: 5,
            batch_size: 8,
            poll_interval_ms: 2_000,
        }
    }
}

impl PublicResolutionConfig {
    /// Reads the access token from its file, once, at startup.
    ///
    /// The file content is trimmed of surrounding whitespace. Neither the path's content nor the
    /// token appears in the error.
    ///
    /// # Errors
    ///
    /// [`ConfigError`] naming the key when no path is configured, the file is unreadable or
    /// larger than 4 KiB, or the token is empty.
    pub fn load_access_token(&self) -> Result<SecretString, ConfigError> {
        let refused = |rule: &'static str| ConfigError::new(TOKEN_PATH_KEY, rule);
        let path = self
            .access_token_path
            .as_ref()
            .ok_or_else(|| refused("is required"))?;
        let metadata =
            std::fs::metadata(path).map_err(|_| refused("must name a readable token file"))?;
        if !metadata.is_file() || metadata.len() > MAX_TOKEN_FILE_BYTES {
            return Err(refused("must name a regular file of at most 4 KiB"));
        }
        let content = std::fs::read_to_string(path)
            .map_err(|_| refused("must name a readable UTF-8 token file"))?;
        let token = content.trim();
        if token.is_empty() {
            return Err(refused("must not name an empty token file"));
        }
        Ok(SecretString::from(token))
    }
}

/// Applies one `RATATOSKR__PUBLIC_RESOLUTION__*` entry; `false` when the key is unknown.
pub(super) fn apply_environment(
    config: &mut PublicResolutionConfig,
    key: &str,
    value: &str,
    violations: &mut Vec<Violation>,
) -> bool {
    let refused = |rule: &'static str| Violation {
        key: key.to_owned(),
        rule,
    };
    match key {
        ENDPOINT_KEY => value.clone_into(&mut config.endpoint),
        TOKEN_PATH_KEY => {
            let path = PathBuf::from(value);
            if path.is_absolute() {
                config.access_token_path = Some(path);
            } else {
                violations.push(refused("must be an absolute readable token-file path"));
            }
        }
        "RATATOSKR__PUBLIC_RESOLUTION__MAX_ATTEMPTS" => match parse_positive::<u32>(value) {
            Ok(parsed) => config.max_attempts = parsed,
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__PUBLIC_RESOLUTION__BATCH_SIZE" => match parse_positive::<u32>(value) {
            Ok(parsed) => config.batch_size = parsed,
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__PUBLIC_RESOLUTION__POLL_INTERVAL_MS" => match parse_positive::<u64>(value) {
            Ok(parsed) => config.poll_interval_ms = parsed,
            Err(rule) => violations.push(refused(rule)),
        },
        _ => return false,
    }
    true
}

/// Validates the loaded section. `bus_configured` makes the access-token path mandatory.
pub(super) fn validate(
    config: &PublicResolutionConfig,
    bus_configured: bool,
    violations: &mut Vec<Violation>,
) {
    if !endpoint_allowed(&config.endpoint) {
        violations.push(Violation {
            key: ENDPOINT_KEY.to_owned(),
            rule: "must be an https URL on graph.facebook.com or graph.instagram.com without credentials, query, or fragment",
        });
    }
    if bus_configured && config.access_token_path.is_none() {
        violations.push(Violation {
            key: TOKEN_PATH_KEY.to_owned(),
            rule: "is required when the bus is configured: a consumer without a resolution surface cannot finish its captures",
        });
    }
    validate_range(
        u64::from(config.max_attempts),
        1,
        10,
        "RATATOSKR__PUBLIC_RESOLUTION__MAX_ATTEMPTS",
        violations,
    );
    validate_range(
        u64::from(config.batch_size),
        1,
        100,
        "RATATOSKR__PUBLIC_RESOLUTION__BATCH_SIZE",
        violations,
    );
    validate_range(
        config.poll_interval_ms,
        100,
        60_000,
        "RATATOSKR__PUBLIC_RESOLUTION__POLL_INTERVAL_MS",
        violations,
    );
}

fn endpoint_allowed(endpoint: &str) -> bool {
    reqwest::Url::parse(endpoint).is_ok_and(|url| {
        url.scheme() == "https"
            && url
                .host_str()
                .is_some_and(|host| ALLOWED_HOSTS.contains(&host))
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.port().is_none()
    })
}
