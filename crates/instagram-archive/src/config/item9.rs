//! Strict disabled-by-default configuration for item-9 lifecycle capabilities.

use serde::Serialize;

use super::{Config, Violation, parse_positive, validate_range};

/// Explicit byte-retention policy; URL references remain the default when disabled.
#[derive(Debug, Clone, Serialize)]
pub struct MediaRetentionConfig {
    /// Whether provider media bytes may be retained after policy admission.
    pub enabled: bool,
    /// Maximum bytes admitted for one object.
    pub max_object_bytes: Option<u64>,
    /// Maximum retained bytes admitted for one owner.
    pub max_owner_bytes: Option<u64>,
    /// Maximum remaining source-URL lifetime considered archivable.
    pub max_url_lifetime_seconds: Option<u64>,
}

/// Finite settings for pending content-addressed blob deletions.
#[derive(Debug, Clone, Serialize)]
pub struct BlobDeletionConfig {
    /// Whether the deletion worker may process durable tasks.
    pub enabled: bool,
    /// Delay between bounded worker passes.
    pub poll_interval_ms: Option<u64>,
    /// Maximum tasks claimed by one pass.
    pub batch_size: Option<u32>,
    /// Maximum failed filesystem attempts before operator intervention.
    pub max_attempts: Option<u32>,
}

/// Finite budgets for refreshing recent explicit captures.
#[derive(Debug, Clone, Serialize)]
pub struct ReResolutionConfig {
    /// Whether scheduled re-resolution is enabled.
    pub enabled: bool,
    /// Oldest capture age admitted to a run.
    pub recency_window_seconds: Option<u64>,
    /// Maximum selected items in one run.
    pub item_budget: Option<u32>,
    /// Maximum outbound requests in one run.
    pub request_budget: Option<u32>,
    /// Maximum accepted response bytes in one run.
    pub response_byte_budget: Option<u64>,
    /// Maximum elapsed run duration.
    pub duration_budget_ms: Option<u64>,
    /// Maximum simultaneous provider requests.
    pub concurrency_limit: Option<u32>,
    /// Maximum calls admitted by the provider-specific budget.
    pub provider_call_budget: Option<u32>,
}

/// Finite settings for parser-version projection reprocessing.
#[derive(Debug, Clone, Serialize)]
pub struct ReprocessingConfig {
    /// Whether mutation through the operator CLI is enabled.
    pub enabled: bool,
    /// Maximum plan items committed by one CLI invocation.
    pub max_items_per_invocation: Option<u32>,
}

#[expect(
    clippy::too_many_lines,
    reason = "one closed item-9 configuration matrix reports every independent and cross-field guard"
)]
pub(super) fn validate(config: &Config, violations: &mut Vec<Violation>) {
    validate_item9_section(
        config.media_retention.enabled,
        "RATATOSKR__MEDIA_RETENTION__ENABLED",
        &[
            (
                config.media_retention.max_object_bytes,
                1_024,
                1_073_741_824,
                "RATATOSKR__MEDIA_RETENTION__MAX_OBJECT_BYTES",
            ),
            (
                config.media_retention.max_owner_bytes,
                1_024,
                1_099_511_627_776,
                "RATATOSKR__MEDIA_RETENTION__MAX_OWNER_BYTES",
            ),
            (
                config.media_retention.max_url_lifetime_seconds,
                60,
                2_592_000,
                "RATATOSKR__MEDIA_RETENTION__MAX_URL_LIFETIME_SECONDS",
            ),
        ],
        violations,
    );
    if let (Some(object), Some(owner)) = (
        config.media_retention.max_object_bytes,
        config.media_retention.max_owner_bytes,
    ) && object > owner
    {
        violations.push(Violation {
            key: "RATATOSKR__MEDIA_RETENTION__MAX_OWNER_BYTES".to_owned(),
            rule: "must be at least the per-object byte ceiling",
        });
    }

    validate_item9_section(
        config.blob_deletion.enabled,
        "RATATOSKR__BLOB_DELETION__ENABLED",
        &[
            (
                config.blob_deletion.poll_interval_ms,
                100,
                86_400_000,
                "RATATOSKR__BLOB_DELETION__POLL_INTERVAL_MS",
            ),
            (
                config.blob_deletion.batch_size.map(u64::from),
                1,
                1_000,
                "RATATOSKR__BLOB_DELETION__BATCH_SIZE",
            ),
            (
                config.blob_deletion.max_attempts.map(u64::from),
                1,
                20,
                "RATATOSKR__BLOB_DELETION__MAX_ATTEMPTS",
            ),
        ],
        violations,
    );

    validate_item9_section(
        config.re_resolution.enabled,
        "RATATOSKR__RE_RESOLUTION__ENABLED",
        &[
            (
                config.re_resolution.recency_window_seconds,
                60,
                2_592_000,
                "RATATOSKR__RE_RESOLUTION__RECENCY_WINDOW_SECONDS",
            ),
            (
                config.re_resolution.item_budget.map(u64::from),
                1,
                10_000,
                "RATATOSKR__RE_RESOLUTION__ITEM_BUDGET",
            ),
            (
                config.re_resolution.request_budget.map(u64::from),
                1,
                10_000,
                "RATATOSKR__RE_RESOLUTION__REQUEST_BUDGET",
            ),
            (
                config.re_resolution.response_byte_budget,
                1_024,
                1_073_741_824,
                "RATATOSKR__RE_RESOLUTION__RESPONSE_BYTE_BUDGET",
            ),
            (
                config.re_resolution.duration_budget_ms,
                100,
                3_600_000,
                "RATATOSKR__RE_RESOLUTION__DURATION_BUDGET_MS",
            ),
            (
                config.re_resolution.concurrency_limit.map(u64::from),
                1,
                32,
                "RATATOSKR__RE_RESOLUTION__CONCURRENCY_LIMIT",
            ),
            (
                config.re_resolution.provider_call_budget.map(u64::from),
                1,
                10_000,
                "RATATOSKR__RE_RESOLUTION__PROVIDER_CALL_BUDGET",
            ),
        ],
        violations,
    );
    if let (Some(items), Some(requests)) = (
        config.re_resolution.item_budget,
        config.re_resolution.request_budget,
    ) && requests > items
    {
        violations.push(Violation {
            key: "RATATOSKR__RE_RESOLUTION__REQUEST_BUDGET".to_owned(),
            rule: "must not exceed the item budget",
        });
    }
    if let (Some(requests), Some(concurrency)) = (
        config.re_resolution.request_budget,
        config.re_resolution.concurrency_limit,
    ) && concurrency > requests
    {
        violations.push(Violation {
            key: "RATATOSKR__RE_RESOLUTION__CONCURRENCY_LIMIT".to_owned(),
            rule: "must not exceed the request budget",
        });
    }
    if let (Some(requests), Some(provider_calls)) = (
        config.re_resolution.request_budget,
        config.re_resolution.provider_call_budget,
    ) && provider_calls > requests
    {
        violations.push(Violation {
            key: "RATATOSKR__RE_RESOLUTION__PROVIDER_CALL_BUDGET".to_owned(),
            rule: "must not exceed the request budget",
        });
    }

    validate_item9_section(
        config.reprocessing.enabled,
        "RATATOSKR__REPROCESSING__ENABLED",
        &[(
            config.reprocessing.max_items_per_invocation.map(u64::from),
            1,
            10_000,
            "RATATOSKR__REPROCESSING__MAX_ITEMS_PER_INVOCATION",
        )],
        violations,
    );
}

fn validate_item9_section(
    enabled: bool,
    enabled_key: &str,
    guards: &[(Option<u64>, u64, u64, &str)],
    violations: &mut Vec<Violation>,
) {
    let before = violations.len();
    for (value, minimum, maximum, key) in guards {
        match value {
            Some(value) => validate_range(*value, *minimum, *maximum, key, violations),
            None if enabled => violations.push(Violation {
                key: (*key).to_owned(),
                rule: "is required when the capability is enabled",
            }),
            None => {}
        }
    }
    if enabled && violations.len() > before {
        violations.push(Violation {
            key: enabled_key.to_owned(),
            rule: "requires every finite nonzero guard to be valid",
        });
    }
}

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
        "RATATOSKR__MEDIA_RETENTION__ENABLED" => match value {
            "true" => config.media_retention.enabled = true,
            "false" => config.media_retention.enabled = false,
            _ => violations.push(refused("must be true or false")),
        },
        "RATATOSKR__MEDIA_RETENTION__MAX_OBJECT_BYTES" => match parse_positive::<u64>(value) {
            Ok(parsed) => config.media_retention.max_object_bytes = Some(parsed),
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__MEDIA_RETENTION__MAX_OWNER_BYTES" => match parse_positive::<u64>(value) {
            Ok(parsed) => config.media_retention.max_owner_bytes = Some(parsed),
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__MEDIA_RETENTION__MAX_URL_LIFETIME_SECONDS" => {
            match parse_positive::<u64>(value) {
                Ok(parsed) => config.media_retention.max_url_lifetime_seconds = Some(parsed),
                Err(rule) => violations.push(refused(rule)),
            }
        }
        "RATATOSKR__BLOB_DELETION__ENABLED" => match value {
            "true" => config.blob_deletion.enabled = true,
            "false" => config.blob_deletion.enabled = false,
            _ => violations.push(refused("must be true or false")),
        },
        "RATATOSKR__BLOB_DELETION__POLL_INTERVAL_MS" => match parse_positive::<u64>(value) {
            Ok(parsed) => config.blob_deletion.poll_interval_ms = Some(parsed),
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__BLOB_DELETION__BATCH_SIZE" => match parse_positive::<u32>(value) {
            Ok(parsed) => config.blob_deletion.batch_size = Some(parsed),
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__BLOB_DELETION__MAX_ATTEMPTS" => match parse_positive::<u32>(value) {
            Ok(parsed) => config.blob_deletion.max_attempts = Some(parsed),
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__RE_RESOLUTION__ENABLED" => match value {
            "true" => config.re_resolution.enabled = true,
            "false" => config.re_resolution.enabled = false,
            _ => violations.push(refused("must be true or false")),
        },
        "RATATOSKR__RE_RESOLUTION__RECENCY_WINDOW_SECONDS" => match parse_positive::<u64>(value) {
            Ok(parsed) => config.re_resolution.recency_window_seconds = Some(parsed),
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__RE_RESOLUTION__ITEM_BUDGET" => match parse_positive::<u32>(value) {
            Ok(parsed) => config.re_resolution.item_budget = Some(parsed),
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__RE_RESOLUTION__REQUEST_BUDGET" => match parse_positive::<u32>(value) {
            Ok(parsed) => config.re_resolution.request_budget = Some(parsed),
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__RE_RESOLUTION__RESPONSE_BYTE_BUDGET" => match parse_positive::<u64>(value) {
            Ok(parsed) => config.re_resolution.response_byte_budget = Some(parsed),
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__RE_RESOLUTION__DURATION_BUDGET_MS" => match parse_positive::<u64>(value) {
            Ok(parsed) => config.re_resolution.duration_budget_ms = Some(parsed),
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__RE_RESOLUTION__CONCURRENCY_LIMIT" => match parse_positive::<u32>(value) {
            Ok(parsed) => config.re_resolution.concurrency_limit = Some(parsed),
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__RE_RESOLUTION__PROVIDER_CALL_BUDGET" => match parse_positive::<u32>(value) {
            Ok(parsed) => config.re_resolution.provider_call_budget = Some(parsed),
            Err(rule) => violations.push(refused(rule)),
        },
        "RATATOSKR__REPROCESSING__ENABLED" => match value {
            "true" => config.reprocessing.enabled = true,
            "false" => config.reprocessing.enabled = false,
            _ => violations.push(refused("must be true or false")),
        },
        "RATATOSKR__REPROCESSING__MAX_ITEMS_PER_INVOCATION" => match parse_positive::<u32>(value) {
            Ok(parsed) => config.reprocessing.max_items_per_invocation = Some(parsed),
            Err(rule) => violations.push(refused(rule)),
        },
        _ => return false,
    }
    true
}
