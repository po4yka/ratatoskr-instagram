//! Structured telemetry: the JSON log pipeline and the Prometheus registry.
//!
//! Installed exactly once per process. A second installation attempt is a
//! refusal, not a reset: two subscribers or two recorders would split every
//! observation after startup.

use metrics::{counter, gauge, histogram};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::util::SubscriberInitExt as _;

use crate::config::TelemetryConfig;

/// Closed Data Export pipeline stage label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataExportStage {
    /// Authenticated streaming receipt.
    Receipt,
    /// Hostile archive inspection.
    Inspect,
    /// Versioned format parsing.
    Parse,
    /// Projection/report reconciliation.
    Reconcile,
}

/// Closed Data Export stage outcome label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataExportOutcome {
    /// A new receipt was accepted.
    Accepted,
    /// An exact owner receipt was replayed.
    Replayed,
    /// A processing stage completed.
    Succeeded,
    /// Input or durable evidence was refused.
    Refused,
}

/// Closed completeness gap label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataExportGap {
    /// Comparable identity found in both sources.
    Matched,
    /// Identity found only in the export.
    ExportOnly,
    /// Comparable capture found only outside the export.
    CaptureOnly,
    /// Capture without a comparable stable identity.
    NonComparable,
}

/// Closed parsed category label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataExportCategory {
    /// Supported saved-post records.
    SavedPosts,
    /// Retained unknown archive material.
    Unknown,
}

/// Closed parser warning label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataExportWarning {
    /// One saved record had an unknown shape.
    UnknownSavedRecord,
    /// The supported section contained an unknown top-level field.
    UnknownSavedSectionField,
    /// An archive entry belongs to an unknown section.
    UnknownArchiveSection,
    /// Referenced media bytes remain only inside the raw archive.
    MediaBytesReferenceOnly,
}

/// Closed safe failure class label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataExportFailure {
    /// Authentication was refused before reading a body.
    Authentication,
    /// Streamed body exceeded its configured ceiling.
    BodyLimit,
    /// Request body streaming failed.
    BodyStream,
    /// Protected raw storage failed.
    RawStorage,
    /// Existing immutable evidence disagreed.
    ImmutableConflict,
    /// Archive path semantics were unsafe.
    UnsafeArchivePath,
    /// An archive resource ceiling was exceeded.
    ArchiveLimit,
    /// Compression or encryption was unsupported.
    UnsupportedEncoding,
    /// Entry type was unsupported.
    UnsupportedEntryType,
    /// ZIP structure was malformed.
    MalformedArchive,
    /// No parser supports the detected layout.
    UnsupportedLayout,
    /// Supported JSON was malformed.
    InvalidJson,
    /// `SocialSource` publication failed.
    Publish,
    /// Durable persistence failed.
    Persistence,
    /// A compare-and-swap transition lost its precondition.
    StateConflict,
}

/// Stable bounded Data Export stage label.
#[must_use]
pub const fn data_export_stage_label(stage: DataExportStage) -> &'static str {
    match stage {
        DataExportStage::Receipt => "receipt",
        DataExportStage::Inspect => "inspect",
        DataExportStage::Parse => "parse",
        DataExportStage::Reconcile => "reconcile",
    }
}

/// Stable bounded Data Export outcome label.
#[must_use]
pub const fn data_export_outcome_label(outcome: DataExportOutcome) -> &'static str {
    match outcome {
        DataExportOutcome::Accepted => "accepted",
        DataExportOutcome::Replayed => "replayed",
        DataExportOutcome::Succeeded => "succeeded",
        DataExportOutcome::Refused => "refused",
    }
}

/// Stable bounded completeness gap label.
#[must_use]
pub const fn data_export_gap_label(gap: DataExportGap) -> &'static str {
    match gap {
        DataExportGap::Matched => "matched",
        DataExportGap::ExportOnly => "export_only",
        DataExportGap::CaptureOnly => "capture_only",
        DataExportGap::NonComparable => "non_comparable",
    }
}

/// Stable bounded category label.
#[must_use]
pub const fn data_export_category_label(category: DataExportCategory) -> &'static str {
    match category {
        DataExportCategory::SavedPosts => "saved_posts",
        DataExportCategory::Unknown => "unknown",
    }
}

/// Stable bounded parser-warning label.
#[must_use]
pub const fn data_export_warning_label(warning: DataExportWarning) -> &'static str {
    match warning {
        DataExportWarning::UnknownSavedRecord => "unknown_saved_record",
        DataExportWarning::UnknownSavedSectionField => "unknown_saved_section_field",
        DataExportWarning::UnknownArchiveSection => "unknown_archive_section",
        DataExportWarning::MediaBytesReferenceOnly => "media_bytes_reference_only",
    }
}

/// Stable bounded failure class label.
#[must_use]
pub const fn data_export_failure_label(failure: DataExportFailure) -> &'static str {
    match failure {
        DataExportFailure::Authentication => "authentication",
        DataExportFailure::BodyLimit => "body_limit",
        DataExportFailure::BodyStream => "body_stream",
        DataExportFailure::RawStorage => "raw_storage",
        DataExportFailure::ImmutableConflict => "immutable_conflict",
        DataExportFailure::UnsafeArchivePath => "unsafe_entry_name",
        DataExportFailure::ArchiveLimit => "archive_limit",
        DataExportFailure::UnsupportedEncoding => "unsupported_encoding",
        DataExportFailure::UnsupportedEntryType => "unsupported_entry_type",
        DataExportFailure::MalformedArchive => "malformed_archive",
        DataExportFailure::UnsupportedLayout => "unsupported_layout",
        DataExportFailure::InvalidJson => "invalid_json",
        DataExportFailure::Publish => "publish",
        DataExportFailure::Persistence => "persistence",
        DataExportFailure::StateConflict => "state_conflict",
    }
}

/// Records one pipeline-stage result and its bounded wall time.
pub fn record_data_export_stage(
    stage: DataExportStage,
    outcome: DataExportOutcome,
    duration: std::time::Duration,
) {
    let stage = data_export_stage_label(stage);
    let outcome = data_export_outcome_label(outcome);
    counter!(
        "instagram_data_export_stage_total",
        "stage" => stage,
        "outcome" => outcome,
    )
    .increment(1);
    histogram!("instagram_data_export_stage_duration_seconds", "stage" => stage)
        .record(duration.as_secs_f64());
}

/// Records one streaming receipt result with its bounded wall time.
pub fn record_data_export_receipt(outcome: DataExportOutcome, duration: std::time::Duration) {
    record_data_export_stage(DataExportStage::Receipt, outcome, duration);
}

/// Records a refusal through its closed non-sensitive class.
pub fn record_data_export_failure(failure: DataExportFailure) {
    counter!(
        "instagram_data_export_failure_total",
        "failure" => data_export_failure_label(failure),
    )
    .increment(1);
}

/// Adds a bounded parsed/unknown category count.
pub fn record_data_export_category(category: DataExportCategory, count: u64) {
    counter!(
        "instagram_data_export_category_records_total",
        "category" => data_export_category_label(category),
    )
    .increment(count);
}

/// Adds a bounded parser warning count.
pub fn record_data_export_warning(warning: DataExportWarning, count: u64) {
    counter!(
        "instagram_data_export_warnings_total",
        "warning_kind" => data_export_warning_label(warning),
    )
    .increment(count);
}

/// Sets the most recently reconciled gap cardinality by closed class.
pub fn record_data_export_gap(gap: DataExportGap, count: u64) {
    let value = f64::from(u32::try_from(count).unwrap_or(u32::MAX));
    gauge!(
        "instagram_data_export_completeness_gap_count",
        "gap" => data_export_gap_label(gap),
    )
    .set(value);
}

/// Closed official-account operation label inventory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthOperation {
    /// Begin authorization.
    Begin,
    /// Complete a callback relay.
    Complete,
    /// Refresh token and provider evidence.
    Refresh,
    /// Read current capabilities.
    Capabilities,
    /// Revoke and scrub locally.
    Revoke,
}

/// Closed official-account outcome label inventory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthOutcome {
    /// Operation completed.
    Succeeded,
    /// Feature, owner, flow, or account was unavailable.
    Unavailable,
    /// Caller input was invalid.
    Invalid,
    /// Provider or relay failed.
    Upstream,
    /// Internal persistence or configuration failed.
    Internal,
}

/// Stable bounded operation label.
#[must_use]
pub const fn oauth_operation_label(operation: OAuthOperation) -> &'static str {
    match operation {
        OAuthOperation::Begin => "begin",
        OAuthOperation::Complete => "complete",
        OAuthOperation::Refresh => "refresh",
        OAuthOperation::Capabilities => "capabilities",
        OAuthOperation::Revoke => "revoke",
    }
}

/// Stable bounded outcome label.
#[must_use]
pub const fn oauth_outcome_label(outcome: OAuthOutcome) -> &'static str {
    match outcome {
        OAuthOutcome::Succeeded => "succeeded",
        OAuthOutcome::Unavailable => "unavailable",
        OAuthOutcome::Invalid => "invalid",
        OAuthOutcome::Upstream => "upstream",
        OAuthOutcome::Internal => "internal",
    }
}

/// Records one bounded official-account lifecycle outcome.
pub fn record_oauth_operation(operation: OAuthOperation, outcome: OAuthOutcome) {
    counter!(
        "instagram_oauth_operations_total",
        "operation" => oauth_operation_label(operation),
        "outcome" => oauth_outcome_label(outcome),
    )
    .increment(1);
}

/// The one wire identity of this bounded context.
pub const SERVICE_NAME: &str = "ratatoskr-instagram-archive";

/// The deployable role this binary serves. One process, one role.
pub const ROLE: &str = "archive";

/// The crate version, compiled in.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The build's git commit, provided by the container build, or `unknown`
/// outside one — the first thing anyone checks when a deployment misbehaves.
pub const GIT_SHA: &str = match option_env!("RATATOSKR_GIT_SHA") {
    Some(sha) => sha,
    None => "unknown",
};

/// The declared toolchain.
pub const RUST_VERSION: &str = env!("CARGO_PKG_RUST_VERSION");

/// The build-identity gauge: one series, labelled with the compiled identity.
const BUILD_INFO_METRIC: &str = "instagram_build_info";

/// Telemetry bootstrap failure.
#[derive(Debug, thiserror::Error)]
#[error("telemetry could not be initialized")]
pub struct TelemetryError(#[source] Box<dyn std::error::Error + Send + Sync>);

/// Owns the telemetry runtime for the life of the process.
#[derive(Debug)]
pub struct TelemetryGuard {
    /// The text exposition renderer of the installed recorder.
    metrics_handle: PrometheusHandle,
}

impl TelemetryGuard {
    /// A cloneable renderer of the installed recorder, handed to whatever
    /// surface serves the exposition text.
    #[must_use]
    pub fn metrics_handle(&self) -> PrometheusHandle {
        self.metrics_handle.clone()
    }

    /// Releases telemetry resources before exit.
    pub fn shutdown(self) {}
}

/// Installs the process-wide structured telemetry once.
///
/// # Errors
///
/// Returns [`TelemetryError`] when the filter expression is invalid, a global
/// subscriber is already installed, or the Prometheus recorder cannot be
/// installed.
pub fn init_telemetry(config: &TelemetryConfig) -> Result<TelemetryGuard, TelemetryError> {
    let filter =
        EnvFilter::try_new(&config.log_filter).map_err(|error| TelemetryError(Box::new(error)))?;

    let metrics_handle = PrometheusBuilder::new()
        .install_recorder()
        .map_err(|error| TelemetryError(Box::new(error)))?;

    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .finish();
    subscriber.try_init().map_err(|error| {
        // The recorder is already global at this point; nothing can uninstall
        // it, so the caller must treat this failure as fatal rather than retry
        // into a half-installed state.
        TelemetryError(Box::new(error))
    })?;

    gauge!(BUILD_INFO_METRIC,
        "service" => SERVICE_NAME,
        "role" => ROLE,
        "version" => VERSION,
        "git_sha" => GIT_SHA,
        "rust_version" => RUST_VERSION,
    )
    .set(1.0);

    Ok(TelemetryGuard { metrics_handle })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A shared in-memory writer capturing what the formatter produces.
    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Capture {
        fn new() -> Self {
            Self(Arc::new(Mutex::new(Vec::new())))
        }

        fn snapshot(&self) -> String {
            let guard = self.0.lock().unwrap();
            String::from_utf8(guard.clone()).expect("captured logs are UTF-8")
        }
    }

    struct CaptureWriter<'a>(&'a Capture);

    impl std::io::Write for CaptureWriter<'_> {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.0.lock().unwrap().write(buf)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            self.0.0.lock().unwrap().flush()
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
        type Writer = CaptureWriter<'a>;

        fn make_writer(&'a self) -> Self::Writer {
            CaptureWriter(self)
        }
    }

    /// The JSON formatter passes structured identity fields through verbatim,
    /// so an operator's first log line answers "what is running".
    #[test]
    fn json_records_carry_identity_fields() {
        let capture = Capture::new();

        let filter = EnvFilter::try_new("info").expect("the default filter expression parses");
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .with_writer(capture.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(
                service_name = SERVICE_NAME,
                version = VERSION,
                git_sha = GIT_SHA,
                "startup"
            );
        });

        let rendered = capture.snapshot();
        let last_line = rendered.lines().next_back();
        assert!(last_line.is_some(), "the formatter produced no output");
        let record: serde_json::Value = serde_json::from_str(last_line.unwrap_or_default())
            .expect("the last log line must parse as JSON");

        assert_eq!(record["fields"]["service_name"], SERVICE_NAME);
        assert_eq!(record["fields"]["version"], VERSION);
        assert_eq!(record["fields"]["git_sha"], GIT_SHA);
    }
}
