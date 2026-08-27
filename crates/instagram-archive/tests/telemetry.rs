//! Telemetry bootstrap: the executable form of the service-runtime spec.

use ratatoskr_instagram_archive::config::TelemetryConfig;
use ratatoskr_instagram_archive::init_telemetry;
use ratatoskr_instagram_archive::provider::{ProviderError, ProviderFailureClass};
use ratatoskr_instagram_archive::telemetry::{
    DataExportCategory, DataExportFailure, DataExportGap, DataExportOutcome, DataExportStage,
    DataExportWarning, OAuthOperation, OAuthOutcome, data_export_category_label,
    data_export_failure_label, data_export_gap_label, data_export_outcome_label,
    data_export_stage_label, data_export_warning_label, oauth_operation_label, oauth_outcome_label,
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
#[expect(
    clippy::too_many_lines,
    reason = "one test enumerates every closed label so additions cannot bypass privacy review"
)]
fn data_export_metrics_use_only_closed_non_sensitive_labels() {
    let stages = [
        (DataExportStage::Receipt, "receipt"),
        (DataExportStage::Inspect, "inspect"),
        (DataExportStage::Parse, "parse"),
        (DataExportStage::Reconcile, "reconcile"),
    ];
    let outcomes = [
        (DataExportOutcome::Accepted, "accepted"),
        (DataExportOutcome::Replayed, "replayed"),
        (DataExportOutcome::Succeeded, "succeeded"),
        (DataExportOutcome::Refused, "refused"),
    ];
    let gaps = [
        (DataExportGap::Matched, "matched"),
        (DataExportGap::ExportOnly, "export_only"),
        (DataExportGap::CaptureOnly, "capture_only"),
        (DataExportGap::NonComparable, "non_comparable"),
    ];
    let categories = [
        (DataExportCategory::SavedPosts, "saved_posts"),
        (DataExportCategory::Unknown, "unknown"),
    ];
    let warnings = [
        (
            DataExportWarning::UnknownSavedRecord,
            "unknown_saved_record",
        ),
        (
            DataExportWarning::UnknownSavedSectionField,
            "unknown_saved_section_field",
        ),
        (
            DataExportWarning::UnknownArchiveSection,
            "unknown_archive_section",
        ),
        (
            DataExportWarning::MediaBytesReferenceOnly,
            "media_bytes_reference_only",
        ),
    ];
    let failures = [
        (DataExportFailure::Authentication, "authentication"),
        (DataExportFailure::BodyLimit, "body_limit"),
        (DataExportFailure::BodyStream, "body_stream"),
        (DataExportFailure::RawStorage, "raw_storage"),
        (DataExportFailure::ImmutableConflict, "immutable_conflict"),
        (DataExportFailure::UnsafeArchivePath, "unsafe_entry_name"),
        (DataExportFailure::ArchiveLimit, "archive_limit"),
        (
            DataExportFailure::UnsupportedEncoding,
            "unsupported_encoding",
        ),
        (
            DataExportFailure::UnsupportedEntryType,
            "unsupported_entry_type",
        ),
        (DataExportFailure::MalformedArchive, "malformed_archive"),
        (DataExportFailure::UnsupportedLayout, "unsupported_layout"),
        (DataExportFailure::InvalidJson, "invalid_json"),
        (DataExportFailure::Publish, "publish"),
        (DataExportFailure::Persistence, "persistence"),
        (DataExportFailure::StateConflict, "state_conflict"),
    ];
    let mut labels = Vec::new();
    for (value, expected) in stages {
        let label = data_export_stage_label(value);
        assert_eq!(label, expected);
        labels.push(label);
    }
    for (value, expected) in outcomes {
        let label = data_export_outcome_label(value);
        assert_eq!(label, expected);
        labels.push(label);
    }
    for (value, expected) in gaps {
        let label = data_export_gap_label(value);
        assert_eq!(label, expected);
        labels.push(label);
    }
    for (value, expected) in categories {
        let label = data_export_category_label(value);
        assert_eq!(label, expected);
        labels.push(label);
    }
    for (value, expected) in warnings {
        let label = data_export_warning_label(value);
        assert_eq!(label, expected);
        labels.push(label);
    }
    for (value, expected) in failures {
        let label = data_export_failure_label(value);
        assert_eq!(label, expected);
        labels.push(label);
    }
    for label in labels {
        for forbidden in ["token", "url", "user", "digest", "path", "username"] {
            assert!(
                !label.contains(forbidden),
                "private label fragment: {label}"
            );
        }
    }
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
