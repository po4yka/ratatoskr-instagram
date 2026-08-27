//! Telemetry bootstrap: the executable form of the service-runtime spec.

use ratatoskr_instagram_archive::config::TelemetryConfig;
use ratatoskr_instagram_archive::init_telemetry;
use ratatoskr_instagram_archive::provider::{ProviderError, ProviderFailureClass};
use ratatoskr_instagram_archive::telemetry::{
    OAuthOperation, OAuthOutcome, oauth_operation_label, oauth_outcome_label,
};

#[test]
fn initialization_succeeds_once_then_reports_already_installed() {
    let config = TelemetryConfig {
        log_filter: "info".to_owned(),
    };

    init_telemetry(&config).expect("the first initialization must succeed");
    let error = init_telemetry(&config)
        .expect_err("a second initialization in one process must be refused");
    assert!(
        error.to_string().contains("telemetry"),
        "the failure must be the typed telemetry refusal: {error}"
    );
}

#[test]
fn oauth_failures_and_usage_metrics_have_bounded_labels() {
    let operations = [
        (OAuthOperation::Begin, "begin"),
        (OAuthOperation::Complete, "complete"),
        (OAuthOperation::Refresh, "refresh"),
        (OAuthOperation::Capabilities, "capabilities"),
        (OAuthOperation::Revoke, "revoke"),
    ];
    for (operation, expected) in operations {
        assert_eq!(oauth_operation_label(operation), expected);
    }
    let outcomes = [
        (OAuthOutcome::Succeeded, "succeeded"),
        (OAuthOutcome::Unavailable, "unavailable"),
        (OAuthOutcome::Invalid, "invalid"),
        (OAuthOutcome::Upstream, "upstream"),
        (OAuthOutcome::Internal, "internal"),
    ];
    for (outcome, expected) in outcomes {
        assert_eq!(oauth_outcome_label(outcome), expected);
    }
}

#[test]
fn synthetic_secrets_never_appear_in_logs_metrics_audit_usage_or_http_errors() {
    const SENTINEL: &str = "SYNTHETIC_TELEMETRY_SECRET_SENTINEL";
    let error = ProviderError {
        class: ProviderFailureClass::Authentication,
        http_status: Some(401),
    };
    let rendered = format!("{error:?} {error}");
    assert!(!rendered.contains(SENTINEL));
    for operation in [OAuthOperation::Begin, OAuthOperation::Complete] {
        assert!(!oauth_operation_label(operation).contains(SENTINEL));
    }
    for outcome in [OAuthOutcome::Upstream, OAuthOutcome::Internal] {
        assert!(!oauth_outcome_label(outcome).contains(SENTINEL));
    }
}
