//! Structured telemetry: the JSON log pipeline and the Prometheus registry.
//!
//! Installed exactly once per process. A second installation attempt is a
//! refusal, not a reset: two subscribers or two recorders would split every
//! observation after startup.

use metrics::{counter, gauge};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::util::SubscriberInitExt as _;

use crate::config::TelemetryConfig;

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
