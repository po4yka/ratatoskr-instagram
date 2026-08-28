use super::*;

async fn assert_export_fact_identities_use_provider_id(test: &TestDatabase) {
    let fact_identities: Vec<(String, Uuid)> = sqlx::query_as(
        "select payload #>> '{payload,source,external_post_id}', aggregate_id
         from instagram_archive.outbox_events
         where event_type = 'social.source.captured.v1'
         order by payload #>> '{payload,source,external_post_id}'",
    )
    .fetch_all(test.database.pool())
    .await
    .expect("export fact identities read");
    let owner = Uuid::parse_str(OWNER).expect("owner UUID");
    let expected = ["SYNTHETIC01", "SYNTHETIC02"]
        .into_iter()
        .map(|provider_id| {
            (
                provider_id.to_owned(),
                ratatoskr_instagram_archive::publishing::own_media_source_identity(
                    owner,
                    provider_id,
                ),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        fact_identities, expected,
        "Data Export facts must use stable owner plus provider identity"
    );
}

#[tokio::test]
async fn reconciliation_replay_is_idempotent_and_export_absence_is_non_destructive() {
    const PATH: &str = "your_instagram_activity/saved/saved_posts.json";
    let test = TestDatabase::create().await.expect("fresh database");
    let root = test_root();
    let archive = data_export_zip(
        &[(
            PATH,
            include_bytes!("../fixtures/data_export/saved_posts.json"),
        )],
        CompressionMethod::Deflated,
    )
    .expect("fixture export builds");
    let absent_capture = insert_capture(
        &test,
        "https://www.instagram.com/p/SYNTHETIC99/",
        "resolved",
    )
    .await;
    let (store, run_id) = parsed_run(&test, &root, archive).await;

    let first = store
        .reconcile(run_id)
        .await
        .expect("first reconciliation succeeds");
    let counts_after_first: (i64, i64, i64, i64) = sqlx::query_as(
        "select
            (select count(*) from instagram_archive.export_records where run_id = $1),
            (select count(*) from instagram_archive.media),
            (select count(*) from instagram_archive.raw_records),
            (select count(*) from instagram_archive.outbox_events)",
    )
    .bind(run_id)
    .fetch_one(test.database.pool())
    .await
    .expect("projection counts read");
    let second = store.reconcile(run_id).await.expect("replay converges");
    let counts_after_replay: (i64, i64, i64, i64) = sqlx::query_as(
        "select
            (select count(*) from instagram_archive.export_records where run_id = $1),
            (select count(*) from instagram_archive.media),
            (select count(*) from instagram_archive.raw_records),
            (select count(*) from instagram_archive.outbox_events)",
    )
    .bind(run_id)
    .fetch_one(test.database.pool())
    .await
    .expect("replayed projection counts read");

    assert_eq!(first, second);
    assert_eq!(
        counts_after_replay, counts_after_first,
        "replay duplicated evidence"
    );
    assert_eq!(
        counts_after_first.1, 2,
        "two normalized sources are projected"
    );
    assert_eq!(
        counts_after_first.3, 2,
        "each first observation publishes once"
    );
    assert_export_fact_identities_use_provider_id(&test).await;
    let capture_state: (String, i64, i64) = sqlx::query_as(
        "select c.status,
                (select count(*) from instagram_archive.local_source_removals r
                 where r.capture_id = c.capture_id),
                (select count(*) from instagram_archive.outbox_events o
                 where o.event_type = 'social.source.removed.v1')
         from instagram_archive.captures c where c.capture_id = $1",
    )
    .bind(absent_capture)
    .fetch_one(test.database.pool())
    .await
    .expect("absent capture remains readable");
    assert_eq!(capture_state, ("resolved".to_owned(), 0, 0));

    tokio::fs::remove_dir_all(&root)
        .await
        .expect("storage cleans up");
    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn completeness_report_lists_exact_overlap_differences_and_non_comparable_captures() {
    const PATH: &str = "your_instagram_activity/saved/saved_posts.json";
    let json = br#"{"saved_saved_media":[
        {"title":"a","string_map_data":{"Saved on":{"href":"https://www.instagram.com/p/MATCH01/","timestamp":1700000000}}},
        {"title":"b","string_map_data":{"Saved on":{"href":"https://www.instagram.com/reel/MATCH02/","timestamp":1700000001}}},
        {"title":"e","string_map_data":{"Saved on":{"href":"https://www.instagram.com/p/EXPORT03/","timestamp":1700000002}}}
    ]}"#;
    let test = TestDatabase::create().await.expect("fresh database");
    let root = test_root();
    for url in [
        "https://www.instagram.com/p/MATCH01/",
        "https://www.instagram.com/reel/MATCH02/",
        "https://www.instagram.com/p/CAPTURE03/",
        "https://www.instagram.com/stories/synthetic/123/",
    ] {
        insert_capture(&test, url, "accepted").await;
    }
    let archive = data_export_zip(&[(PATH, json)], CompressionMethod::Deflated)
        .expect("coverage fixture builds");
    let (store, run_id) = parsed_run(&test, &root, archive).await;

    let report = store.reconcile(run_id).await.expect("coverage reconciles");
    assert_eq!(report.matched, ["MATCH01", "MATCH02"]);
    assert_eq!(report.export_only, ["EXPORT03"]);
    assert_eq!(report.capture_only, ["CAPTURE03"]);
    assert_eq!(report.non_comparable_count(), 1);
    assert_eq!(report.matched_count(), 2);
    assert_eq!(report.export_only_count(), 1);
    assert_eq!(report.capture_only_count(), 1);
    assert!(report.authority_disclaimer.contains("native Saved"));
    assert!(report.authority_disclaimer.contains("deletion"));
    assert!(
        report
            .authority_disclaimer
            .contains("complete account history")
    );
    let persisted: (i64, i64, i64, i64) = sqlx::query_as(
        "select matched_count, export_only_count, capture_only_count, non_comparable_count
         from instagram_archive.export_completeness_reports where run_id = $1",
    )
    .bind(run_id)
    .fetch_one(test.database.pool())
    .await
    .expect("report persisted");
    assert_eq!(persisted, (2, 1, 1, 1));

    tokio::fs::remove_dir_all(&root)
        .await
        .expect("storage cleans up");
    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn ambiguous_identity_stays_conflicted_and_stronger_provenance_survives() {
    const PATH: &str = "your_instagram_activity/saved/saved_posts.json";
    let json = br#"{"saved_saved_media":[
        {"title":"strong","string_map_data":{"Saved on":{"href":"https://www.instagram.com/p/STRONG01/","timestamp":1700000000}}},
        {"title":"ambiguous","string_map_data":{"Saved on":{"href":"https://www.instagram.com/p/AMBIG01/","timestamp":1700000001}}}
    ]}"#;
    let test = TestDatabase::create().await.expect("fresh database");
    let root = test_root();
    let strong_id = Uuid::now_v7();
    let provider_match_id = Uuid::now_v7();
    let permalink_match_id = Uuid::now_v7();
    for (media_id, provider_id, permalink, acquisition, authority) in [
        (
            strong_id,
            "STRONG01",
            "https://www.instagram.com/p/STRONG01/",
            "official_api",
            "authoritative_platform_state",
        ),
        (
            provider_match_id,
            "AMBIG01",
            "https://www.instagram.com/p/OTHER01/",
            "public_resolution",
            "explicit_user_capture",
        ),
        (
            permalink_match_id,
            "OTHER02",
            "https://www.instagram.com/p/AMBIG01/",
            "share_extension",
            "explicit_user_capture",
        ),
    ] {
        sqlx::query(
            "insert into instagram_archive.media
             (media_id, provider_media_id, permalink, media_type, acquisition_method,
              saved_authority, upstream_status)
             values ($1, $2, $3, 'unknown', $4, $5, 'available')",
        )
        .bind(media_id)
        .bind(provider_id)
        .bind(permalink)
        .bind(acquisition)
        .bind(authority)
        .execute(test.database.pool())
        .await
        .expect("existing synthetic media inserts");
    }
    let archive = data_export_zip(&[(PATH, json)], CompressionMethod::Deflated)
        .expect("identity fixture builds");
    let (store, run_id) = parsed_run(&test, &root, archive).await;

    store
        .reconcile(run_id)
        .await
        .expect("identity run reconciles");
    let strong: (String, String, Option<Uuid>) = sqlx::query_as(
        "select acquisition_method, saved_authority, current_revision_id
         from instagram_archive.media where media_id = $1",
    )
    .bind(strong_id)
    .fetch_one(test.database.pool())
    .await
    .expect("strong projection reads");
    assert_eq!(
        strong,
        (
            "official_api".to_owned(),
            "authoritative_platform_state".to_owned(),
            None,
        ),
        "export observation demoted stronger current provenance"
    );
    let conflicts: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "select evidence_key, payload from instagram_archive.export_records
         where run_id = $1 and record_kind = 'conflict' order by evidence_key",
    )
    .bind(run_id)
    .fetch_all(test.database.pool())
    .await
    .expect("conflicts read");
    assert_eq!(conflicts.len(), 1);
    assert_eq!(
        conflicts
            .first()
            .and_then(|row| row.1.get("reason"))
            .and_then(serde_json::Value::as_str),
        Some("provider_and_permalink_resolve_to_distinct_media")
    );
    let ambiguous_facts: i64 = sqlx::query_scalar(
        "select count(*) from instagram_archive.outbox_events
         where payload #>> '{payload,source,external_post_id}' = 'AMBIG01'",
    )
    .fetch_one(test.database.pool())
    .await
    .expect("ambiguous fact count reads");
    assert_eq!(ambiguous_facts, 0, "conflict escaped as a normalized fact");

    tokio::fs::remove_dir_all(&root)
        .await
        .expect("storage cleans up");
    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn successful_import_records_exact_durable_transition_history() {
    let test = TestDatabase::create().await.expect("fresh database");
    let root = test_root();
    let config = data_export_config(&root);
    let store = DataExportStore::new(&test.database, &config).expect("store builds");
    let archive = ratatoskr_instagram_archive::test_support::synthetic_saved_posts_export_zip()
        .expect("fixture export builds");
    let receipt = store
        .receive(
            Uuid::parse_str(OWNER).expect("owner UUID"),
            stream::iter([Ok::<Vec<u8>, Infallible>(archive)]),
        )
        .await
        .expect("archive receives");
    let run_id = receipt.receipt().run_id;
    let worker = DataExportWorker::new(&test.database, &config).expect("worker builds");

    let pass = worker.run_once().await.expect("worker pass succeeds");
    assert_eq!(pass.selected, 1);
    assert_eq!(pass.reconciled, 1);
    assert_eq!(pass.failed, 0);
    let transitions: Vec<String> = sqlx::query_scalar(
        "select to_state from instagram_archive.import_run_transitions
         where run_id = $1 order by ordinal",
    )
    .bind(run_id)
    .fetch_all(test.database.pool())
    .await
    .expect("transition history reads");
    assert_eq!(
        transitions,
        ["received", "inspected", "parsed", "reconciled"]
    );
    let report_count: i64 = sqlx::query_scalar(
        "select count(*) from instagram_archive.export_completeness_reports where run_id = $1",
    )
    .bind(run_id)
    .fetch_one(test.database.pool())
    .await
    .expect("terminal report count reads");
    assert_eq!(report_count, 1);

    tokio::fs::remove_dir_all(&root)
        .await
        .expect("storage cleans up");
    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn stage_failure_is_terminal_and_retains_archive() {
    let test = TestDatabase::create().await.expect("fresh database");
    let root = test_root();
    let config = data_export_config(&root);
    let store = DataExportStore::new(&test.database, &config).expect("store builds");
    let receipt = store
        .receive(
            Uuid::parse_str(OWNER).expect("owner UUID"),
            stream::iter([Ok::<Vec<u8>, Infallible>(b"not a zip".to_vec())]),
        )
        .await
        .expect("hostile bytes are preserved first");
    let run_id = receipt.receipt().run_id;
    let digest = receipt.receipt().archive.digest.hex.as_str().to_owned();
    let worker = DataExportWorker::new(&test.database, &config).expect("worker builds");

    let first = worker
        .run_once()
        .await
        .expect("failure is a handled outcome");
    assert_eq!(first.selected, 1);
    assert_eq!(first.failed, 1);
    let second = worker.run_once().await.expect("terminal replay is a no-op");
    assert_eq!(second.selected, 0);
    let state: String =
        sqlx::query_scalar("select state from instagram_archive.import_runs where run_id = $1")
            .bind(run_id)
            .fetch_one(test.database.pool())
            .await
            .expect("terminal state reads");
    assert_eq!(state, "failed");
    assert!(
        root.join("blobs").join("sha256").join(digest).is_file(),
        "terminal failure retains exact raw archive"
    );

    tokio::fs::remove_dir_all(&root)
        .await
        .expect("storage cleans up");
    test.cleanup().await.expect("cleanup must drop");
}
