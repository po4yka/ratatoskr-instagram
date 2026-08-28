//! Item-9 publication and privacy-resurrection integration tests.

use super::*;

#[tokio::test]
async fn unchanged_reresolution_appends_evidence_without_duplicate_update() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let capture_id = submitted_capture(
        &test.database,
        Uuid::now_v7(),
        ClientSource::IosShareExtension,
    )
    .await;
    let surface = FakePublicSurface::answering(SurfaceOutcome::Payload {
        body: REEL_FIXTURE.to_owned(),
    });
    resolve_once(&test, capture_id, &surface, OffsetDateTime::UNIX_EPOCH).await;
    resolve_once(
        &test,
        capture_id,
        &surface,
        OffsetDateTime::UNIX_EPOCH + time::Duration::minutes(1),
    )
    .await;

    assert_eq!(
        read_facts(&test, capture_id).await,
        ["social.source.captured.v1"]
    );
    let revisions: i64 = sqlx::query_scalar(
        "select count(*) from instagram_archive.media_revisions revision \
         join instagram_archive.captures capture on capture.media_id = revision.media_id \
         where capture.capture_id = $1",
    )
    .bind(capture_id)
    .fetch_one(test.database.pool())
    .await
    .expect("revision count reads");
    assert_eq!(revisions, 2, "unchanged refresh still appends raw evidence");

    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn late_knowledge_completion_cannot_resurrect_a_privacy_removed_source() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let owner = Uuid::now_v7();
    let capture_id =
        submitted_capture(&test.database, owner, ClientSource::IosShareExtension).await;
    resolve_once(
        &test,
        capture_id,
        &FakePublicSurface::answering(SurfaceOutcome::Payload {
            body: REEL_FIXTURE.to_owned(),
        }),
        OffsetDateTime::UNIX_EPOCH,
    )
    .await;
    let (captured, _) = decoded_captured_payload(&test, capture_id).await;
    let media_id: Uuid =
        sqlx::query_scalar("select media_id from instagram_archive.captures where capture_id = $1")
            .bind(capture_id)
            .fetch_one(test.database.pool())
            .await
            .expect("resolved media id reads");
    sqlx::query(
        "insert into instagram_archive.captures \
         (capture_id, user_ref, media_id, canonical_url, acquisition_method, saved_authority, \
          client_source, status, captured_at) values ($1, $2, $3, $4, 'share_extension', \
          'explicit_user_capture', 'android_share_target', 'resolved', now())",
    )
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .bind(media_id)
    .bind(REEL_PERMALINK)
    .execute(test.database.pool())
    .await
    .expect("another owner keeps shared media live");
    DeletionStore::new(&test.database)
        .apply(DeletionRequest {
            operation_id: Uuid::now_v7(),
            user_ref: owner,
            target: DeletionTarget::Capture(capture_id),
        })
        .await
        .expect("privacy deletion commits");
    let replacement_capture = Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.captures \
         (capture_id, user_ref, media_id, canonical_url, acquisition_method, saved_authority, \
          client_source, status, captured_at) values ($1, $2, $3, $4, 'share_extension', \
          'explicit_user_capture', 'ios_share_extension', 'resolved', now())",
    )
    .bind(replacement_capture)
    .bind(owner)
    .bind(media_id)
    .bind(REEL_PERMALINK)
    .execute(test.database.pool())
    .await
    .expect("late local intake recreates only a candidate row");

    let late_completion = completion_envelope(
        &SocialSourceAnalysisCompleted {
            owner: captured.source.owner,
            social_source_id: captured.source.social_source_id,
            content_digest: captured.source.content_digest,
            completed_at: captured.source.captured_at,
            extensions: Extensions::default(),
        },
        Uuid::now_v7(),
    );
    let outcome = test
        .database
        .ingest_analysis_completed(&late_completion)
        .await
        .expect("late completion remains a valid delivery");
    assert_eq!(
        outcome,
        ratatoskr_instagram_archive::AnalysisCompletionOutcome::Skipped
    );
    let links: i64 = sqlx::query_scalar(
        "select count(*) from instagram_archive.capture_analysis_links \
         where capture_id = $1",
    )
    .bind(replacement_capture)
    .fetch_one(test.database.pool())
    .await
    .expect("replacement linkage count reads");
    assert_eq!(links, 0, "privacy removal guard blocks resurrection");

    test.cleanup().await.expect("cleanup");
}
