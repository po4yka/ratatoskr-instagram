//! Parser-version reprocessing and dry-run fidelity tests use synthetic exports only.

#![allow(
    clippy::expect_used,
    reason = "disposable fixture setup and assertions require immediate test failure"
)]

use sha2::{Digest as _, Sha256};

use ratatoskr_instagram_archive::data_export_reprocessing::{
    ReprocessClassification, ReprocessInput, ReprocessingError, ReprocessingStore,
    RetainedExportReceipt, SUPPORTED_REPROCESSING_LAYOUT, SUPPORTED_REPROCESSING_PARSER,
    begin_reprocessing, migration_apply, migration_dry_run,
};
use ratatoskr_instagram_archive::test_support::TestDatabase;

#[test]
fn reprocessing_refuses_tampered_receipts_and_unsupported_parser_versions_before_mutation() {
    let bytes = b"synthetic retained export";
    let correct_hash: [u8; 32] = Sha256::digest(bytes).into();
    let cases = [
        (
            RetainedExportReceipt {
                bytes: b"tampered synthetic retained export",
                expected_hash: correct_hash,
                expected_length: bytes.len() as u64,
                detected_layout: SUPPORTED_REPROCESSING_LAYOUT,
            },
            SUPPORTED_REPROCESSING_PARSER,
            ReprocessingError::ReceiptIntegrity,
        ),
        (
            RetainedExportReceipt {
                bytes,
                expected_hash: correct_hash,
                expected_length: bytes.len() as u64,
                detected_layout: SUPPORTED_REPROCESSING_LAYOUT,
            },
            "unregistered-parser",
            ReprocessingError::UnsupportedParser,
        ),
    ];

    for (receipt, parser, expected) in cases {
        let mut projection_calls = 0_u32;
        assert_eq!(
            begin_reprocessing(receipt, parser, || projection_calls += 1),
            Err(expected)
        );
        assert_eq!(projection_calls, 0, "refusal must precede mutation");
    }
}

fn reprocessing_inputs() -> Vec<ReprocessInput> {
    vec![
        ReprocessInput {
            item_key: "a".to_owned(),
            classification: ReprocessClassification::Normalized,
            prospective_digest: Some("6".repeat(64)),
        },
        ReprocessInput {
            item_key: "b".to_owned(),
            classification: ReprocessClassification::UnknownRecord,
            prospective_digest: None,
        },
        ReprocessInput {
            item_key: "c".to_owned(),
            classification: ReprocessClassification::Warning,
            prospective_digest: None,
        },
    ]
}

#[tokio::test]
async fn apply_resumes_after_committed_checkpoint_and_completed_replay_adds_nothing() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let owner = uuid::Uuid::now_v7();
    let import_run_id = synthetic_import_run(&test, owner).await;
    let inputs = reprocessing_inputs();
    let state = "7".repeat(64);
    let operation_id = uuid::Uuid::now_v7();
    let store = ReprocessingStore::new(&test.database);

    let interrupted = store
        .apply_chunk(owner, import_run_id, operation_id, &inputs, &state, 1)
        .await
        .expect("first chunk commits");
    assert!(!interrupted.completed);
    let resumed = store
        .apply_chunk(
            owner,
            import_run_id,
            operation_id,
            &inputs,
            &state,
            usize::MAX,
        )
        .await
        .expect("same operation resumes");
    let fresh = store
        .apply_chunk(
            owner,
            import_run_id,
            uuid::Uuid::now_v7(),
            &inputs,
            &state,
            usize::MAX,
        )
        .await
        .expect("fresh uninterrupted run completes");
    assert_eq!(resumed.report, fresh.report);
    let before_replay: (i64, i64) = sqlx::query_as(
        "select \
           (select count(*) from instagram_archive.export_reprocessing_runs), \
           (select count(*) from instagram_archive.export_reprocessing_items)",
    )
    .fetch_one(test.database.pool())
    .await
    .expect("completed counts read");
    let replay = store
        .apply_chunk(
            owner,
            import_run_id,
            operation_id,
            &inputs,
            &state,
            usize::MAX,
        )
        .await
        .expect("completed operation replays");
    let after_replay: (i64, i64) = sqlx::query_as(
        "select \
           (select count(*) from instagram_archive.export_reprocessing_runs), \
           (select count(*) from instagram_archive.export_reprocessing_items)",
    )
    .fetch_one(test.database.pool())
    .await
    .expect("replay counts read");

    assert_eq!(replay, resumed);
    assert_eq!(after_replay, before_replay);

    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end omission fixture snapshots every retained evidence class"
)]
async fn parser_omission_never_deletes_existing_capture_source_media_or_prior_evidence() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let owner = uuid::Uuid::now_v7();
    let import_run_id = synthetic_import_run(&test, owner).await;
    let raw_record_id = uuid::Uuid::now_v7();
    let raw_body = b"retained synthetic export evidence";
    let raw_digest = Sha256::digest(raw_body).to_vec();
    sqlx::query(
        "insert into instagram_archive.raw_records \
         (raw_record_id, record_kind, blob_ref, content_hash, byte_size, body, observed_at) \
         values ($1, 'export_section', 'retained-export-record', $2, $3, $4, now())",
    )
    .bind(raw_record_id)
    .bind(&raw_digest)
    .bind(i64::try_from(raw_body.len()).expect("synthetic size fits"))
    .bind(raw_body.as_slice())
    .execute(test.database.pool())
    .await
    .expect("prior raw evidence stores");
    let media_id = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.media \
         (media_id, provider_media_id, permalink, media_type, acquisition_method, \
          saved_authority, upstream_status) values ($1, 'omission-media', \
          'https://www.instagram.com/p/OMISSION1/', 'image', 'data_export', \
          'export_observation', 'available')",
    )
    .bind(media_id)
    .execute(test.database.pool())
    .await
    .expect("existing export projection stores");
    let revision_id = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.media_revisions \
         (revision_id, media_id, raw_record_id, parser_version, resolved_at) \
         values ($1, $2, $3, $4, now())",
    )
    .bind(revision_id)
    .bind(media_id)
    .bind(raw_record_id)
    .bind(SUPPORTED_REPROCESSING_PARSER)
    .execute(test.database.pool())
    .await
    .expect("prior revision stores");
    sqlx::query("update instagram_archive.media set current_revision_id = $2 where media_id = $1")
        .bind(media_id)
        .bind(revision_id)
        .execute(test.database.pool())
        .await
        .expect("current revision links");
    sqlx::query(
        "insert into instagram_archive.captures \
         (capture_id, user_ref, media_id, canonical_url, acquisition_method, saved_authority, \
          client_source, status, captured_at) values ($1, $2, $3, \
          'https://www.instagram.com/p/OMISSION1/', 'data_export', 'export_observation', \
          'browser_extension', 'resolved', now())",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(owner)
    .bind(media_id)
    .execute(test.database.pool())
    .await
    .expect("existing capture projection stores");
    let before: (i64, i64, i64, i64) = sqlx::query_as(
        "select \
           (select count(*) from instagram_archive.captures), \
           (select count(*) from instagram_archive.media), \
           (select count(*) from instagram_archive.media_revisions), \
           (select count(*) from instagram_archive.raw_records)",
    )
    .fetch_one(test.database.pool())
    .await
    .expect("projection snapshot reads");
    let inputs = vec![ReprocessInput {
        item_key: media_id.to_string(),
        classification: ReprocessClassification::Omitted,
        prospective_digest: None,
    }];

    let outcome = ReprocessingStore::new(&test.database)
        .apply_chunk(
            owner,
            import_run_id,
            uuid::Uuid::now_v7(),
            &inputs,
            &"8".repeat(64),
            usize::MAX,
        )
        .await
        .expect("omission is a reportable completed outcome");
    let after: (i64, i64, i64, i64) = sqlx::query_as(
        "select \
           (select count(*) from instagram_archive.captures), \
           (select count(*) from instagram_archive.media), \
           (select count(*) from instagram_archive.media_revisions), \
           (select count(*) from instagram_archive.raw_records)",
    )
    .fetch_one(test.database.pool())
    .await
    .expect("projection snapshot reads again");

    assert_eq!(
        outcome.report.items[0].classification,
        ReprocessClassification::Omitted
    );
    assert!(
        outcome.report.items[0].retained_prior_state,
        "omission report must explicitly state prior projection retention"
    );
    assert_eq!(
        after, before,
        "parser omission never deletes prior evidence"
    );

    test.cleanup().await.expect("cleanup must drop");
}

async fn synthetic_import_run(test: &TestDatabase, owner: uuid::Uuid) -> uuid::Uuid {
    let snapshot_id = uuid::Uuid::now_v7();
    let run_id = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.export_snapshots \
         (snapshot_id, user_ref, archive_hash, archive_blob_ref, archive_byte_size, received_at) \
         values ($1, $2, $3, 'instagram-archive/export/sha256/synthetic', 7, now())",
    )
    .bind(snapshot_id)
    .bind(owner)
    .bind(vec![0x22_u8; 32])
    .execute(test.database.pool())
    .await
    .expect("synthetic export receipt stores");
    sqlx::query(
        "insert into instagram_archive.import_runs \
         (run_id, snapshot_id, user_ref, state, detected_layout, parser_id, \
          records_processed, warning_count, received_at, updated_at, finished_at) \
         values ($1, $2, $3, 'reconciled', $4, $5, 1, 0, now(), now(), now())",
    )
    .bind(run_id)
    .bind(snapshot_id)
    .bind(owner)
    .bind(SUPPORTED_REPROCESSING_LAYOUT)
    .bind(SUPPORTED_REPROCESSING_PARSER)
    .execute(test.database.pool())
    .await
    .expect("synthetic import run stores");
    run_id
}

#[tokio::test]
async fn dry_run_does_not_change_database_blob_outbox_or_checkpoint_state() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let owner = uuid::Uuid::now_v7();
    let import_run_id = synthetic_import_run(&test, owner).await;
    let before: (i64, i64, i64, i64, i64) = sqlx::query_as(
        "select \
           (select count(*) from instagram_archive.export_snapshots), \
           (select count(*) from instagram_archive.import_runs), \
           (select count(*) from instagram_archive.export_reprocessing_runs), \
           (select count(*) from instagram_archive.export_reprocessing_items), \
           (select count(*) from instagram_archive.outbox_events)",
    )
    .fetch_one(test.database.pool())
    .await
    .expect("dry-run snapshot reads");
    let inputs = vec![
        ReprocessInput {
            item_key: "known".to_owned(),
            classification: ReprocessClassification::Normalized,
            prospective_digest: Some("4".repeat(64)),
        },
        ReprocessInput {
            item_key: "unknown".to_owned(),
            classification: ReprocessClassification::UnknownRecord,
            prospective_digest: None,
        },
        ReprocessInput {
            item_key: "warning".to_owned(),
            classification: ReprocessClassification::Warning,
            prospective_digest: None,
        },
        ReprocessInput {
            item_key: "conflict".to_owned(),
            classification: ReprocessClassification::Conflict,
            prospective_digest: None,
        },
    ];

    ReprocessingStore::new(&test.database)
        .dry_run(owner, import_run_id, &inputs, &"5".repeat(64))
        .await
        .expect("dry-run report answers");
    let after: (i64, i64, i64, i64, i64) = sqlx::query_as(
        "select \
           (select count(*) from instagram_archive.export_snapshots), \
           (select count(*) from instagram_archive.import_runs), \
           (select count(*) from instagram_archive.export_reprocessing_runs), \
           (select count(*) from instagram_archive.export_reprocessing_items), \
           (select count(*) from instagram_archive.outbox_events)",
    )
    .fetch_one(test.database.pool())
    .await
    .expect("dry-run snapshot reads again");
    assert_eq!(after, before, "dry-run must have zero durable mutation");

    test.cleanup().await.expect("cleanup must drop");
}

#[test]
fn migration_dry_run_matches_apply_report_for_unchanged_state() {
    let inputs = vec![
        ReprocessInput {
            item_key: "z-warning".to_owned(),
            classification: ReprocessClassification::Warning,
            prospective_digest: None,
        },
        ReprocessInput {
            item_key: "a-normalized".to_owned(),
            classification: ReprocessClassification::Normalized,
            prospective_digest: Some("1".repeat(64)),
        },
        ReprocessInput {
            item_key: "m-conflict".to_owned(),
            classification: ReprocessClassification::Conflict,
            prospective_digest: Some("2".repeat(64)),
        },
        ReprocessInput {
            item_key: "u-unknown".to_owned(),
            classification: ReprocessClassification::UnknownSection,
            prospective_digest: None,
        },
    ];
    let state_fingerprint = "3".repeat(64);

    let dry_run = migration_dry_run(&inputs, &state_fingerprint);
    let applied = migration_apply(&inputs, &state_fingerprint);

    assert_eq!(
        dry_run, applied,
        "dry-run and apply must render one shared deterministic plan"
    );
}
