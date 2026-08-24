//! Telemetry bootstrap: the executable form of the service-runtime spec.

use ratatoskr_instagram_archive::config::TelemetryConfig;
use ratatoskr_instagram_archive::init_telemetry;

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
