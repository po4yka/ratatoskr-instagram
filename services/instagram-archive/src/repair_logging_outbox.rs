//! Closed stopped-service command for repairing logging-era false acknowledgements.

use std::io::Write as _;
use std::process::ExitCode;
use std::time::Duration;

use ratatoskr_instagram_archive::telemetry::SERVICE_NAME;
use ratatoskr_instagram_archive::{Config, Database};
use secrecy::ExposeSecret as _;

const CONFIRMATION: &str = "logging-transport-never-delivered";

pub(super) fn run(arguments: &[String]) -> ExitCode {
    if arguments != ["repair-logging-outbox", "--confirm", CONFIRMATION] {
        eprintln!(
            "{SERVICE_NAME}: usage: ratatoskr-instagram-archive repair-logging-outbox \
             --confirm {CONFIRMATION}"
        );
        return ExitCode::from(2);
    }
    let config = match Config::load() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{SERVICE_NAME}: {error}");
            return ExitCode::from(78);
        }
    };
    let Some(database_url) = config.storage.database_url.as_ref() else {
        eprintln!(
            "{SERVICE_NAME}: repair-logging-outbox requires \
             RATATOSKR__STORAGE__DATABASE_URL"
        );
        return ExitCode::from(78);
    };
    run_repair(database_url.expose_secret())
}

#[tokio::main]
async fn run_repair(database_url: &str) -> ExitCode {
    let Ok(database) = Database::connect(database_url, 2, Duration::from_secs(5)).await else {
        eprintln!("{SERVICE_NAME}: repair-logging-outbox database connection failed");
        return ExitCode::FAILURE;
    };
    let result =
        ratatoskr_instagram_archive::outbox_repair::repair_logging_outbox(database.pool()).await;
    database.close().await;
    if let Ok(repaired) = result {
        if writeln!(std::io::stdout().lock(), "{repaired}").is_ok() {
            ExitCode::SUCCESS
        } else {
            eprintln!("{SERVICE_NAME}: repair-logging-outbox stdout failed");
            ExitCode::FAILURE
        }
    } else {
        eprintln!("{SERVICE_NAME}: repair-logging-outbox transaction failed");
        ExitCode::FAILURE
    }
}
