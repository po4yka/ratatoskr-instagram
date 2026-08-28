//! Process contract for explicit parser-version reprocessing modes.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "process assertions and disposable fixture setup"
)]

use std::process::{Command, Output, Stdio};
use std::time::Duration;

use ratatoskr_instagram_archive::data_export_reprocessing::{
    SUPPORTED_REPROCESSING_LAYOUT, SUPPORTED_REPROCESSING_PARSER,
};
use ratatoskr_instagram_archive::test_support::{TestDatabase, admin_url};
use uuid::Uuid;

const NATS_URL: &str = "nats://127.0.0.1:5422";

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_ratatoskr-instagram-archive")
}

fn database_url(name: &str) -> String {
    let admin = admin_url();
    let (prefix, _) = admin.rsplit_once('/').expect("admin URL has database path");
    format!("{prefix}/{name}")
}

fn run(arguments: &[String], database_url: Option<&str>) -> Output {
    let mut command = Command::new(binary());
    command.args(arguments).env_clear();
    if let Some(database_url) = database_url {
        command
            .env("RATATOSKR__STORAGE__DATABASE_URL", database_url)
            .env("RATATOSKR__BUS__URL", NATS_URL)
            .env("RATATOSKR__REPROCESSING__ENABLED", "true")
            .env("RATATOSKR__REPROCESSING__MAX_ITEMS_PER_INVOCATION", "1000");
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().expect("service process runs");
    for _ in 0..400 {
        if child.try_wait().expect("process status reads").is_some() {
            return child
                .wait_with_output()
                .expect("service process output reads");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child.kill().expect("hung process is stopped");
    child
        .wait_with_output()
        .expect("stopped process output reads")
}

async fn import_run(test: &TestDatabase, owner: Uuid, archive_hash: Vec<u8>) -> Uuid {
    let snapshot_id = Uuid::now_v7();
    let run_id = Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.export_snapshots \
         (snapshot_id, user_ref, archive_hash, archive_blob_ref, archive_byte_size, received_at) \
         values ($1, $2, $3, 'instagram-archive/export/sha256/process-fixture', 7, now())",
    )
    .bind(snapshot_id)
    .bind(owner)
    .bind(archive_hash)
    .execute(test.database.pool())
    .await
    .expect("process fixture snapshot stores");
    sqlx::query(
        "insert into instagram_archive.import_runs \
         (run_id, snapshot_id, user_ref, state, detected_layout, parser_id, \
          records_processed, warning_count, received_at, updated_at, finished_at) \
         values ($1, $2, $3, 'reconciled', $4, $5, 0, 0, now(), now(), now())",
    )
    .bind(run_id)
    .bind(snapshot_id)
    .bind(owner)
    .bind(SUPPORTED_REPROCESSING_LAYOUT)
    .bind(SUPPORTED_REPROCESSING_PARSER)
    .execute(test.database.pool())
    .await
    .expect("process fixture import run stores");
    run_id
}

fn base_arguments(owner: Uuid, run_id: Uuid) -> Vec<String> {
    vec![
        "reprocess-export".to_owned(),
        "dry-run".to_owned(),
        "--owner".to_owned(),
        owner.to_string(),
        "--run-id".to_owned(),
        run_id.to_string(),
        "--parser".to_owned(),
        SUPPORTED_REPROCESSING_PARSER.to_owned(),
    ]
}

#[tokio::test]
async fn process_contract_separates_json_stdout_diagnostics_and_exit_codes() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let owner = Uuid::now_v7();
    let import_run_id = import_run(&test, owner, vec![0x72_u8; 32]).await;
    let database_url = database_url(test.name());
    let base = base_arguments(owner, import_run_id);

    let dry_run = run(&base, Some(&database_url));
    assert!(
        dry_run.status.success(),
        "dry-run stderr: {}",
        String::from_utf8_lossy(&dry_run.stderr)
    );
    assert!(dry_run.stderr.is_empty());
    assert!(
        dry_run.stdout.ends_with(b"\n")
            && !dry_run.stdout[..dry_run.stdout.len() - 1].contains(&b'\n')
    );
    let dry_json: serde_json::Value =
        serde_json::from_slice(&dry_run.stdout).expect("one JSON document");
    assert_eq!(dry_json["mode"], "dry-run");

    let mut apply = base.clone();
    apply[1] = "apply".to_owned();
    apply.extend(["--operation-id".to_owned(), Uuid::now_v7().to_string()]);
    let apply = run(&apply, Some(&database_url));
    assert!(
        apply.status.success(),
        "apply stderr: {}",
        String::from_utf8_lossy(&apply.stderr)
    );
    assert!(apply.stderr.is_empty());
    let apply_json: serde_json::Value =
        serde_json::from_slice(&apply.stdout).expect("one JSON document");
    assert_eq!(apply_json["mode"], "apply");

    let invalid = run(&["reprocess-export".to_owned(), "guess".to_owned()], None);
    assert_eq!(invalid.status.code(), Some(2));
    assert!(invalid.stdout.is_empty() && !invalid.stderr.is_empty());

    let bad_configuration = run(&base, None);
    assert_eq!(bad_configuration.status.code(), Some(78));
    assert!(bad_configuration.stdout.is_empty() && !bad_configuration.stderr.is_empty());

    let corrupt_run = import_run(&test, owner, vec![0x72_u8; 31]).await;
    let integrity = run(&base_arguments(owner, corrupt_run), Some(&database_url));
    assert_eq!(integrity.status.code(), Some(1));
    assert!(integrity.stdout.is_empty() && !integrity.stderr.is_empty());

    let operational = run(
        &base,
        Some("postgres://instagram:instagram@127.0.0.1:1/instagram"),
    );
    assert_eq!(operational.status.code(), Some(1));
    assert!(operational.stdout.is_empty() && !operational.stderr.is_empty());

    let mut broken_pipe = Command::new(binary())
        .args(&base)
        .env_clear()
        .env("RATATOSKR__STORAGE__DATABASE_URL", &database_url)
        .env("RATATOSKR__BUS__URL", NATS_URL)
        .env("RATATOSKR__REPROCESSING__ENABLED", "true")
        .env("RATATOSKR__REPROCESSING__MAX_ITEMS_PER_INVOCATION", "1000")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("broken-pipe process starts");
    drop(broken_pipe.stdout.take());
    let broken_pipe = broken_pipe
        .wait_with_output()
        .expect("broken-pipe process exits");
    assert_eq!(broken_pipe.status.code(), Some(1));
    assert!(!broken_pipe.stderr.is_empty());

    test.cleanup().await.expect("cleanup must drop");
}
