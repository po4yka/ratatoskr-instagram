//! Outbox publisher loop and removal resend configuration (XR-021 CONTRACTS.md R5-11 rule 4).

use serde::Serialize;

use super::{Config, Violation, parse_positive, validate_range};
use crate::publishing::ResendPolicy;

/// Outbox publisher loop configuration. The loop only runs when storage is
/// configured; without a database there is nothing to publish.
#[derive(Debug, Clone, Serialize)]
pub struct PublisherConfig {
    /// Milliseconds between publisher passes.
    pub poll_interval_ms: u64,
    /// Maximum facts claimed per pass.
    pub batch_size: u32,
}

impl Default for PublisherConfig {
    fn default() -> Self {
        Self {
            poll_interval_ms: 1_000,
            batch_size: 16,
        }
    }
}

/// Resend timing of a `social.source.removed.v1` fact that Knowledge has not acknowledged.
///
/// The events stream collapses a repeated message id inside its 120 second duplicate window, so a
/// resend sooner than that is a no-op at the broker: the default of 600 seconds is deliberate and
/// the minimum of 30 seconds exists for tests.
#[derive(Debug, Clone, Serialize)]
pub struct TombstoneConfig {
    /// Seconds before the first republish; each later delay doubles. 30 to 86400.
    pub resend_after_seconds: u64,
    /// The longest delay between two republishes in seconds: at least `resend_after_seconds`, at
    /// most 604800.
    pub resend_cap_seconds: u64,
}

impl Default for TombstoneConfig {
    fn default() -> Self {
        let defaults = ResendPolicy::default();
        Self {
            resend_after_seconds: defaults.after.as_secs(),
            resend_cap_seconds: defaults.cap.as_secs(),
        }
    }
}

impl TombstoneConfig {
    /// The timing as the publisher pass takes it.
    #[must_use]
    pub const fn resend_policy(&self) -> ResendPolicy {
        ResendPolicy::new(
            std::time::Duration::from_secs(self.resend_after_seconds),
            std::time::Duration::from_secs(self.resend_cap_seconds),
        )
    }
}

const AFTER_KEY: &str = "RATATOSKR__TOMBSTONE__RESEND_AFTER_SECONDS";
const CAP_KEY: &str = "RATATOSKR__TOMBSTONE__RESEND_CAP_SECONDS";

/// The longest resend cap in seconds: seven days.
const MAX_CAP_SECONDS: u64 = 604_800;

/// Applies one `RATATOSKR__PUBLISHER__*` or `RATATOSKR__TOMBSTONE__*` entry; `false` when the key
/// is unknown. A tombstone value is stored as parsed and range-checked by [`validate`].
pub(super) fn apply_environment(
    config: &mut Config,
    key: &str,
    value: &str,
    violations: &mut Vec<Violation>,
) -> bool {
    let refused = |rule: &'static str| Violation {
        key: key.to_owned(),
        rule,
    };
    match key {
        "RATATOSKR__PUBLISHER__POLL_INTERVAL_MS" => match parse_positive::<u64>(value) {
            Ok(parsed) => config.publisher.poll_interval_ms = parsed,
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__PUBLISHER__BATCH_SIZE" => match parse_positive::<u32>(value) {
            Ok(parsed) => config.publisher.batch_size = parsed,
            Err(rule) => violations.push(refused(rule)),
        },
        AFTER_KEY => match parse_positive::<u64>(value) {
            Ok(parsed) => config.tombstone.resend_after_seconds = parsed,
            Err(rule) => violations.push(refused(rule)),
        },
        CAP_KEY => match parse_positive::<u64>(value) {
            Ok(parsed) => config.tombstone.resend_cap_seconds = parsed,
            Err(rule) => violations.push(refused(rule)),
        },
        _ => return false,
    }
    true
}

/// Validates the loaded tombstone section.
pub(super) fn validate(config: &TombstoneConfig, violations: &mut Vec<Violation>) {
    validate_range(
        config.resend_after_seconds,
        30,
        86_400,
        AFTER_KEY,
        violations,
    );
    if config.resend_cap_seconds < config.resend_after_seconds
        || config.resend_cap_seconds > MAX_CAP_SECONDS
    {
        violations.push(Violation {
            key: CAP_KEY.to_owned(),
            rule: "must be at least the resend delay and at most 604800 seconds",
        });
    }
}
