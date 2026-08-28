//! Explicit owner-scoped parser reprocessing process mode.

use std::io::Write as _;
use std::process::ExitCode;
use std::time::Duration;

use ratatoskr_instagram_archive::data_export_reprocessing::{
    ReprocessClassification, ReprocessInput, ReprocessReport, ReprocessingStore,
    SUPPORTED_REPROCESSING_LAYOUT, SUPPORTED_REPROCESSING_PARSER,
};
use ratatoskr_instagram_archive::telemetry::SERVICE_NAME;
use ratatoskr_instagram_archive::{Config, Database};
use secrecy::ExposeSecret as _;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReprocessMode {
    DryRun,
    Apply,
}

#[derive(Debug, Clone, Copy)]
struct ReprocessCommand {
    mode: ReprocessMode,
    owner: Uuid,
    import_run_id: Uuid,
    operation_id: Option<Uuid>,
}

pub(super) fn run(arguments: &[String]) -> ExitCode {
    let command = match parse_reprocess_command(arguments) {
        Ok(command) => command,
        Err(message) => {
            eprintln!(
                "{SERVICE_NAME}: {message}\nusage: ratatoskr-instagram-archive reprocess-export dry-run --owner UUID --run-id UUID --parser TOKEN\n       ratatoskr-instagram-archive reprocess-export apply --owner UUID --run-id UUID --parser TOKEN --operation-id UUID"
            );
            return ExitCode::from(2);
        }
    };
    let config = match Config::load() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{SERVICE_NAME}: {error}");
            return ExitCode::from(78);
        }
    };
    let Some(database_url) = config.storage.database_url.as_ref() else {
        eprintln!("{SERVICE_NAME}: reprocess-export requires RATATOSKR__STORAGE__DATABASE_URL");
        return ExitCode::from(78);
    };
    if command.mode == ReprocessMode::Apply && !config.reprocessing.enabled {
        eprintln!(
            "{SERVICE_NAME}: reprocess-export apply requires RATATOSKR__REPROCESSING__ENABLED=true"
        );
        return ExitCode::from(78);
    }
    let max_items = config
        .reprocessing
        .max_items_per_invocation
        .map_or(1, |value| value as usize);
    run_reprocess_export(command, database_url.expose_secret(), max_items)
}

fn parse_reprocess_command(arguments: &[String]) -> Result<ReprocessCommand, &'static str> {
    if arguments.first().map(String::as_str) != Some("reprocess-export") {
        return Err("invalid reprocess-export command");
    }
    let mode = match arguments.get(1).map(String::as_str) {
        Some("dry-run") => ReprocessMode::DryRun,
        Some("apply") => ReprocessMode::Apply,
        _ => return Err("mode must be dry-run or apply"),
    };
    let mut owner = None;
    let mut import_run_id = None;
    let mut parser = None;
    let mut operation_id = None;
    let mut index = 2;
    while index < arguments.len() {
        let flag = arguments
            .get(index)
            .map(String::as_str)
            .ok_or("invalid reprocess-export flag")?;
        let value = arguments
            .get(index + 1)
            .ok_or("every flag requires one value")?;
        match flag {
            "--owner" if owner.is_none() => {
                owner = Some(value.parse().map_err(|_| "--owner must be a UUID")?);
            }
            "--run-id" if import_run_id.is_none() => {
                import_run_id = Some(value.parse().map_err(|_| "--run-id must be a UUID")?);
            }
            "--parser" if parser.is_none() => parser = Some(value.as_str()),
            "--operation-id" if operation_id.is_none() => {
                operation_id = Some(value.parse().map_err(|_| "--operation-id must be a UUID")?);
            }
            _ => return Err("unknown or duplicate reprocess-export flag"),
        }
        index += 2;
    }
    if parser != Some(SUPPORTED_REPROCESSING_PARSER) {
        return Err("--parser is not registered");
    }
    if mode == ReprocessMode::DryRun && operation_id.is_some() {
        return Err("dry-run does not accept --operation-id");
    }
    if mode == ReprocessMode::Apply && operation_id.is_none() {
        return Err("apply requires --operation-id");
    }
    Ok(ReprocessCommand {
        mode,
        owner: owner.ok_or("--owner is required")?,
        import_run_id: import_run_id.ok_or("--run-id is required")?,
        operation_id,
    })
}

#[tokio::main]
async fn run_reprocess_export(
    command: ReprocessCommand,
    database_url: &str,
    max_items: usize,
) -> ExitCode {
    let database = match Database::connect(database_url, 2, Duration::from_secs(5)).await {
        Ok(database) => database,
        Err(error) => {
            eprintln!("{SERVICE_NAME}: reprocess-export database connection failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = database.apply_schema().await {
        eprintln!("{SERVICE_NAME}: reprocess-export schema check failed: {error}");
        return ExitCode::FAILURE;
    }
    let (inputs, state_fingerprint) = match load_reprocessing_inputs(&database, command).await {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("{SERVICE_NAME}: reprocess-export receipt load failed: {error}");
            database.close().await;
            return ExitCode::FAILURE;
        }
    };
    let store = ReprocessingStore::new(&database);
    let rendered = match command.mode {
        ReprocessMode::DryRun => match store
            .dry_run(
                command.owner,
                command.import_run_id,
                &inputs,
                &state_fingerprint,
            )
            .await
        {
            Ok(report) => render_cli_report("dry-run", command, &report, None),
            Err(error) => {
                eprintln!("{SERVICE_NAME}: reprocess-export dry-run failed: {error}");
                database.close().await;
                return ExitCode::FAILURE;
            }
        },
        ReprocessMode::Apply => match store
            .apply_chunk(
                command.owner,
                command.import_run_id,
                command.operation_id.unwrap_or(Uuid::nil()),
                &inputs,
                &state_fingerprint,
                max_items,
            )
            .await
        {
            Ok(outcome) => render_cli_report(
                "apply",
                command,
                &outcome.report,
                Some((outcome.reprocessing_run_id, outcome.completed)),
            ),
            Err(error) => {
                eprintln!("{SERVICE_NAME}: reprocess-export apply failed: {error}");
                database.close().await;
                return ExitCode::FAILURE;
            }
        },
    };
    database.close().await;
    match write_json_stdout(&rendered) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{SERVICE_NAME}: reprocess-export stdout failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn load_reprocessing_inputs(
    database: &Database,
    command: ReprocessCommand,
) -> Result<(Vec<ReprocessInput>, String), sqlx::Error> {
    let archive_hash: Vec<u8> = sqlx::query_scalar(
        "select s.archive_hash from instagram_archive.import_runs r \
         join instagram_archive.export_snapshots s on s.snapshot_id = r.snapshot_id \
         where r.run_id = $1 and r.user_ref = $2 and r.state = 'reconciled' \
           and r.detected_layout = $3 and r.parser_id = $4",
    )
    .bind(command.import_run_id)
    .bind(command.owner)
    .bind(SUPPORTED_REPROCESSING_LAYOUT)
    .bind(SUPPORTED_REPROCESSING_PARSER)
    .fetch_one(database.pool())
    .await?;
    let records: Vec<(Uuid, String)> = sqlx::query_as(
        "select record_id, record_kind from instagram_archive.export_records \
         where run_id = $1 order by record_id",
    )
    .bind(command.import_run_id)
    .fetch_all(database.pool())
    .await?;
    let inputs = records
        .into_iter()
        .map(|(record_id, record_kind)| {
            Ok(ReprocessInput {
                item_key: record_id.to_string(),
                classification: classification(&record_kind)?,
                prospective_digest: None,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let state_fingerprint = archive_hash.iter().fold(String::new(), |mut output, byte| {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
        output
    });
    if state_fingerprint.len() != 64 {
        return Err(sqlx::Error::Protocol(
            "archive hash is not a SHA-256 digest".to_owned(),
        ));
    }
    Ok((inputs, state_fingerprint))
}

fn classification(value: &str) -> Result<ReprocessClassification, sqlx::Error> {
    let classification = match value {
        "normalized" => ReprocessClassification::Normalized,
        "unknown_record" => ReprocessClassification::UnknownRecord,
        "unknown_section" => ReprocessClassification::UnknownSection,
        "conflict" => ReprocessClassification::Conflict,
        "warning" => ReprocessClassification::Warning,
        _ => {
            return Err(sqlx::Error::Protocol(
                "unregistered retained export record kind".to_owned(),
            ));
        }
    };
    Ok(classification)
}

fn render_cli_report(
    mode: &str,
    command: ReprocessCommand,
    report: &ReprocessReport,
    applied: Option<(Uuid, bool)>,
) -> serde_json::Value {
    let items = report
        .items
        .iter()
        .map(|item| {
            serde_json::json!({
                "item_key": item.item_key,
                "classification": item.classification.wire_name(),
                "digest": item.digest,
                "retained_prior_state": item.retained_prior_state,
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "mode": mode,
        "owner": command.owner,
        "import_run_id": command.import_run_id,
        "parser_id": SUPPORTED_REPROCESSING_PARSER,
        "operation_id": command.operation_id,
        "reprocessing_run_id": applied.map(|value| value.0),
        "completed": applied.map(|value| value.1),
        "report": {
            "items": items,
            "counts": report.counts,
            "warnings": report.warnings,
            "conflicts": report.conflicts,
            "completeness_evidence": report.completeness_evidence,
            "plan_fingerprint": report.plan_fingerprint,
            "state_fingerprint": report.state_fingerprint,
        }
    })
}

fn write_json_stdout(value: &serde_json::Value) -> std::io::Result<()> {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, value).map_err(std::io::Error::other)?;
    stdout.write_all(b"\n")?;
    stdout.flush()
}
