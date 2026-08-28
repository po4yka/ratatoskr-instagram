//! Owner privacy-deletion completeness and behavior tests.

#![allow(
    clippy::expect_used,
    reason = "disposable privacy fixtures and assertions require immediate test failure"
)]

use std::collections::BTreeSet;

use sha2::Digest as _;

use ratatoskr_instagram_archive::privacy_deletion::{
    CAPTURE_DELETION_CLASSIFICATIONS, CONNECTION_DELETION_CLASSIFICATIONS, DeletionRequest,
    DeletionStore, DeletionTarget, OWNED_DATA_CLASSES, PrivacyDeletionError,
};
use ratatoskr_instagram_archive::test_support::TestDatabase;

#[tokio::test]
async fn deletion_classifies_every_owned_data_and_blob_class() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let table_names: Vec<(String,)> = sqlx::query_as(
        "select table_name from information_schema.tables \
         where table_schema = 'instagram_archive' and table_type = 'BASE TABLE' \
         order by table_name",
    )
    .fetch_all(test.database.pool())
    .await
    .expect("owned table inventory query must answer");

    let mut authoritative = table_names
        .into_iter()
        .map(|(name,)| format!("table:{name}"))
        .collect::<BTreeSet<_>>();
    authoritative.extend([
        "blob:data_export_archive".to_owned(),
        "blob:provider_media".to_owned(),
        "blob:raw_response".to_owned(),
        "blob:user_upload".to_owned(),
    ]);

    let declared = OWNED_DATA_CLASSES
        .iter()
        .map(|class| class.key().to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        declared, authoritative,
        "closed inventory must equal schema plus BlobStore classes"
    );
    assert_eq!(
        OWNED_DATA_CLASSES.len(),
        declared.len(),
        "closed inventory must not contain duplicates"
    );

    for (target, classifications) in [
        ("capture", CAPTURE_DELETION_CLASSIFICATIONS),
        ("connection", CONNECTION_DELETION_CLASSIFICATIONS),
    ] {
        let classified = classifications
            .iter()
            .map(|entry| entry.class.key().to_owned())
            .collect::<BTreeSet<_>>();
        let missing = declared
            .difference(&classified)
            .cloned()
            .collect::<Vec<_>>();
        let unknown = classified
            .difference(&declared)
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            missing.is_empty() && unknown.is_empty() && classifications.len() == classified.len(),
            "{target} deletion classification is not total: missing={missing:?} unknown={unknown:?} duplicates={}",
            classifications.len().saturating_sub(classified.len())
        );
    }

    test.cleanup().await.expect("cleanup must drop");
}

async fn target_row_counts(pool: &sqlx::PgPool) -> (i64, i64, i64) {
    sqlx::query_as(
        "select \
           (select count(*) from instagram_archive.captures), \
           (select count(*) from instagram_archive.accounts), \
           (select count(*) from instagram_archive.deletion_operations)",
    )
    .fetch_one(pool)
    .await
    .expect("target count snapshot reads")
}

#[tokio::test]
async fn preview_matches_apply_counts_for_unchanged_state() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let owner = uuid::Uuid::now_v7();
    let capture_id = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.captures \
         (capture_id, user_ref, canonical_url, acquisition_method, saved_authority, \
          client_source, status, captured_at) \
         values ($1, $2, 'https://www.instagram.com/p/PREVIEW01/', 'share_extension', \
          'explicit_user_capture', 'ios_share_extension', 'accepted', now())",
    )
    .bind(capture_id)
    .bind(owner)
    .execute(test.database.pool())
    .await
    .expect("capture target stores");
    let account_id = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.accounts \
         (account_id, user_ref, provider_account_id, username, account_type, connection_status, \
          scopes, connected_at) values ($1, $2, $3, 'redacted', 'business', 'connected', \
          array['instagram_business_basic'], now())",
    )
    .bind(account_id)
    .bind(owner)
    .bind(format!("provider-{account_id}"))
    .execute(test.database.pool())
    .await
    .expect("connection target stores");
    let store = DeletionStore::new(&test.database);

    for (target, expected_class) in [
        (DeletionTarget::Capture(capture_id), "table:captures"),
        (DeletionTarget::Connection(account_id), "table:accounts"),
    ] {
        let request = DeletionRequest {
            operation_id: uuid::Uuid::now_v7(),
            user_ref: owner,
            target,
        };
        let before = target_row_counts(test.database.pool()).await;
        let plan = store.preview(request).await.expect("preview answers");
        assert_eq!(
            target_row_counts(test.database.pool()).await,
            before,
            "preview must be read-only"
        );
        assert_eq!(
            plan.effects
                .iter()
                .find(|effect| effect.class.key() == expected_class)
                .map(|effect| effect.affected_count),
            Some(1),
            "preview must enumerate the owned target row"
        );
        let applied = store.apply(request).await.expect("apply succeeds");
        assert_eq!(
            applied.effects, plan.effects,
            "apply must recompute the same counts under lock"
        );
    }

    test.cleanup().await.expect("cleanup must drop");
}

async fn lifecycle_counts(pool: &sqlx::PgPool) -> (i64, i64, i64, i64) {
    sqlx::query_as(
        "select \
           (select count(*) from instagram_archive.captures), \
           (select count(*) from instagram_archive.deletion_operations), \
           (select count(*) from instagram_archive.outbox_events), \
           (select count(*) from instagram_archive.blob_deletion_tasks)",
    )
    .fetch_one(pool)
    .await
    .expect("lifecycle count snapshot reads")
}

#[tokio::test]
async fn cross_owner_or_unknown_target_refuses_without_mutation() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let owner = uuid::Uuid::now_v7();
    let capture_id = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.captures \
         (capture_id, user_ref, canonical_url, acquisition_method, saved_authority, \
          client_source, status, captured_at) \
         values ($1, $2, 'https://www.instagram.com/p/PRIVACY01/', 'share_extension', \
          'explicit_user_capture', 'ios_share_extension', 'accepted', now())",
    )
    .bind(capture_id)
    .bind(owner)
    .execute(test.database.pool())
    .await
    .expect("owned capture stores");
    let before = lifecycle_counts(test.database.pool()).await;
    let store = DeletionStore::new(&test.database);

    for request in [
        DeletionRequest {
            operation_id: uuid::Uuid::now_v7(),
            user_ref: uuid::Uuid::now_v7(),
            target: DeletionTarget::Capture(capture_id),
        },
        DeletionRequest {
            operation_id: uuid::Uuid::now_v7(),
            user_ref: owner,
            target: DeletionTarget::Capture(uuid::Uuid::now_v7()),
        },
    ] {
        assert!(matches!(
            store.apply(request).await,
            Err(PrivacyDeletionError::TargetNotFound)
        ));
        assert_eq!(lifecycle_counts(test.database.pool()).await, before);
    }

    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn deleting_one_duplicate_capture_preserves_shared_source_and_emits_no_removal() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let pool = test.database.pool();
    let owner = uuid::Uuid::now_v7();
    let raw_record_id = uuid::Uuid::now_v7();
    let raw_body = b"synthetic shared raw evidence";
    let raw_digest = sha2::Sha256::digest(raw_body).to_vec();
    sqlx::query(
        "insert into instagram_archive.raw_records \
         (raw_record_id, record_kind, blob_ref, content_hash, byte_size, body, observed_at) \
         values ($1, 'oembed_response', 'shared-raw', $2, $3, $4, now())",
    )
    .bind(raw_record_id)
    .bind(&raw_digest)
    .bind(i64::try_from(raw_body.len()).expect("synthetic size fits"))
    .bind(raw_body.as_slice())
    .execute(pool)
    .await
    .expect("raw evidence stores");
    let media_id = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.media \
         (media_id, permalink, media_type, acquisition_method, saved_authority, upstream_status) \
         values ($1, 'https://www.instagram.com/p/SHARED01/', 'image', \
          'public_resolution', 'explicit_user_capture', 'available')",
    )
    .bind(media_id)
    .execute(pool)
    .await
    .expect("shared media stores");
    let revision_id = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.media_revisions \
         (revision_id, media_id, raw_record_id, parser_version, resolved_at) \
         values ($1, $2, $3, 'instagram.oembed.v1', now())",
    )
    .bind(revision_id)
    .bind(media_id)
    .bind(raw_record_id)
    .execute(pool)
    .await
    .expect("revision stores");
    sqlx::query("update instagram_archive.media set current_revision_id = $2 where media_id = $1")
        .bind(media_id)
        .bind(revision_id)
        .execute(pool)
        .await
        .expect("current revision links");
    let first_capture = uuid::Uuid::now_v7();
    for (capture_id, suffix) in [
        (first_capture, "DUPLICATE01"),
        (uuid::Uuid::now_v7(), "DUPLICATE02"),
    ] {
        sqlx::query(
            "insert into instagram_archive.captures \
             (capture_id, user_ref, media_id, canonical_url, acquisition_method, saved_authority, \
              client_source, status, captured_at) values ($1, $2, $3, $4, 'share_extension', \
              'explicit_user_capture', 'ios_share_extension', 'resolved', now())",
        )
        .bind(capture_id)
        .bind(owner)
        .bind(media_id)
        .bind(format!("https://www.instagram.com/p/{suffix}/"))
        .execute(pool)
        .await
        .expect("duplicate capture stores");
    }

    DeletionStore::new(&test.database)
        .apply(DeletionRequest {
            operation_id: uuid::Uuid::now_v7(),
            user_ref: owner,
            target: DeletionTarget::Capture(first_capture),
        })
        .await
        .expect("one duplicate capture deletes");

    let counts: (i64, i64, i64, i64, i64) = sqlx::query_as(
        "select \
           (select count(*) from instagram_archive.captures where media_id = $1), \
           (select count(*) from instagram_archive.media where media_id = $1), \
           (select count(*) from instagram_archive.media_revisions where media_id = $1), \
           (select count(*) from instagram_archive.raw_records where raw_record_id = $2), \
           (select count(*) from instagram_archive.outbox_events \
            where event_type = 'social.source.removed.v1')",
    )
    .bind(media_id)
    .bind(raw_record_id)
    .fetch_one(pool)
    .await
    .expect("shared source counts read");
    assert_eq!(counts, (1, 1, 1, 1, 0));

    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end fixture proves complete erasure, audit, blob work, and event atomicity"
)]
async fn deleting_final_capture_commits_complete_erasure_audit_blob_work_and_one_removal() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let pool = test.database.pool();
    let owner = uuid::Uuid::now_v7();
    let capture_id = uuid::Uuid::now_v7();
    let media_id = uuid::Uuid::now_v7();
    let raw_record_id = uuid::Uuid::now_v7();
    let revision_id = uuid::Uuid::now_v7();
    let operation_id = uuid::Uuid::now_v7();
    let private_note = "private-note-must-disappear-42";
    let raw_body = br#"{"title":"private caption must disappear"}"#;
    let raw_digest = sha2::Sha256::digest(raw_body).to_vec();
    let raw_digest_hex = raw_digest.iter().fold(String::new(), |mut output, byte| {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
        output
    });
    sqlx::query(
        "insert into instagram_archive.raw_records \
         (raw_record_id, record_kind, blob_ref, content_hash, byte_size, body, observed_at) \
         values ($1, 'oembed_response', $2, $3, $4, $5, now())",
    )
    .bind(raw_record_id)
    .bind(&raw_digest_hex)
    .bind(&raw_digest)
    .bind(i64::try_from(raw_body.len()).expect("synthetic size fits"))
    .bind(raw_body.as_slice())
    .execute(pool)
    .await
    .expect("raw evidence stores");
    let media_digest = vec![0x33_u8; 32];
    sqlx::query(
        "insert into instagram_archive.media \
         (media_id, permalink, media_type, caption, acquisition_method, saved_authority, \
          upstream_status, blob_ref, content_hash, byte_size, media_state, retention_class) \
         values ($1, 'https://www.instagram.com/p/FINAL001/', 'image', 'private caption', \
          'public_resolution', 'explicit_user_capture', 'available', \
          'instagram-archive/media/sha256/provider-final', $2, 17, \
          'bytes_archived', 'explicit_archive')",
    )
    .bind(media_id)
    .bind(&media_digest)
    .execute(pool)
    .await
    .expect("media stores");
    sqlx::query(
        "insert into instagram_archive.media_revisions \
         (revision_id, media_id, raw_record_id, parser_version, resolved_at) \
         values ($1, $2, $3, 'instagram.oembed.v1', now())",
    )
    .bind(revision_id)
    .bind(media_id)
    .bind(raw_record_id)
    .execute(pool)
    .await
    .expect("revision stores");
    sqlx::query("update instagram_archive.media set current_revision_id = $2 where media_id = $1")
        .bind(media_id)
        .bind(revision_id)
        .execute(pool)
        .await
        .expect("current revision links");
    sqlx::query(
        "insert into instagram_archive.captures \
         (capture_id, user_ref, media_id, canonical_url, acquisition_method, saved_authority, \
          client_source, status, note, captured_at) values ($1, $2, $3, \
          'https://www.instagram.com/p/FINAL001/', 'share_extension', \
          'explicit_user_capture', 'ios_share_extension', 'resolved', $4, now())",
    )
    .bind(capture_id)
    .bind(owner)
    .bind(media_id)
    .bind(private_note)
    .execute(pool)
    .await
    .expect("final capture stores");
    sqlx::query(
        "insert into instagram_archive.capture_notes \
         (note_id, capture_id, body, created_at) values ($1, $2, $3, now())",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(capture_id)
    .bind(private_note)
    .execute(pool)
    .await
    .expect("private note stores");
    sqlx::query(
        "insert into instagram_archive.capture_analysis_links \
         (capture_id, content_digest, completed_at) values ($1, $2, now())",
    )
    .bind(capture_id)
    .bind("synthetic-analysis-digest")
    .execute(pool)
    .await
    .expect("analysis link stores");
    sqlx::query(
        "insert into instagram_archive.availability_observations \
         (observation_id, media_id, capture_id, availability, resolver_version, observed_at) \
         values ($1, $2, $3, 'available', 'instagram.oembed.v1', now())",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(media_id)
    .bind(capture_id)
    .execute(pool)
    .await
    .expect("availability stores");

    DeletionStore::new(&test.database)
        .apply(DeletionRequest {
            operation_id,
            user_ref: owner,
            target: DeletionTarget::Capture(capture_id),
        })
        .await
        .expect("final capture deletion succeeds");

    let counts: (i64, i64, i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "select \
           (select count(*) from instagram_archive.captures where capture_id = $1), \
           (select count(*) from instagram_archive.capture_notes where capture_id = $1), \
           (select count(*) from instagram_archive.capture_analysis_links where capture_id = $1), \
           (select count(*) from instagram_archive.availability_observations \
            where capture_id = $1 or media_id = $2), \
           (select count(*) from instagram_archive.media where media_id = $2), \
           (select count(*) from instagram_archive.media_revisions where media_id = $2), \
           (select count(*) from instagram_archive.raw_records where raw_record_id = $3), \
           (select count(*) from instagram_archive.local_source_removals where operation_id = $4), \
           (select count(*) from instagram_archive.outbox_events \
            where event_type = 'social.source.removed.v1' and aggregate_id = $1)",
    )
    .bind(capture_id)
    .bind(media_id)
    .bind(raw_record_id)
    .bind(operation_id)
    .fetch_one(pool)
    .await
    .expect("erasure counts read");
    assert_eq!(counts, (0, 0, 0, 0, 0, 0, 0, 1, 1));
    let blob_tasks: i64 = sqlx::query_scalar(
        "select count(*) from instagram_archive.blob_deletion_tasks where operation_id = $1",
    )
    .bind(operation_id)
    .fetch_one(pool)
    .await
    .expect("blob tasks count reads");
    assert_eq!(blob_tasks, 2, "raw and provider-media blobs are scheduled");
    let audit_json: String = sqlx::query_scalar(
        "select jsonb_build_object(\
           'operation', row_to_json(o), \
           'effects', coalesce(jsonb_agg(row_to_json(e)) filter (where e.operation_id is not null), '[]'))::text \
         from instagram_archive.deletion_operations o \
         left join instagram_archive.deletion_effects e using (operation_id) \
         where o.operation_id = $1 group by o.operation_id",
    )
    .bind(operation_id)
    .fetch_one(pool)
    .await
    .expect("content-free audit reads");
    for forbidden in [
        "instagram.com",
        "private caption",
        private_note,
        "oembed_response",
        "provider-final",
    ] {
        assert!(!audit_json.contains(forbidden), "audit leaked {forbidden}");
    }

    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end connection fixture contrasts exclusive erasure with shared capture retention"
)]
async fn connection_deletion_erases_credentials_and_exclusive_state_but_preserves_an_explicit_capture()
 {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let pool = test.database.pool();
    let owner = uuid::Uuid::now_v7();
    let account_id = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.accounts \
         (account_id, user_ref, provider_account_id, username, account_type, connection_status, \
          scopes, connected_at) values ($1, $2, 'provider-shared', 'private-username', \
          'business', 'connected', array['instagram_business_basic'], now())",
    )
    .bind(account_id)
    .bind(owner)
    .execute(pool)
    .await
    .expect("account stores");
    sqlx::query(
        "insert into instagram_archive.credentials \
         (credential_id, account_id, access_token_envelope, refresh_token_envelope, key_version, \
          granted_permissions) values ($1, $2, $3, $4, 1, array['instagram_business_basic'])",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(account_id)
    .bind(vec![0x11_u8; 32])
    .bind(vec![0x22_u8; 32])
    .execute(pool)
    .await
    .expect("encrypted credential stores");
    let raw_record_id = uuid::Uuid::now_v7();
    let raw_body = b"official raw response retained by explicit capture";
    let raw_digest = sha2::Sha256::digest(raw_body).to_vec();
    sqlx::query(
        "insert into instagram_archive.raw_records \
         (raw_record_id, record_kind, blob_ref, content_hash, byte_size, body, observed_at) \
         values ($1, 'api_response', 'official-shared-raw', $2, $3, $4, now())",
    )
    .bind(raw_record_id)
    .bind(&raw_digest)
    .bind(i64::try_from(raw_body.len()).expect("synthetic size fits"))
    .bind(raw_body.as_slice())
    .execute(pool)
    .await
    .expect("official raw stores");
    let media_id = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.media \
         (media_id, account_id, provider_media_id, permalink, media_type, acquisition_method, \
          saved_authority, upstream_status) values ($1, $2, 'provider-media-shared', \
          'https://www.instagram.com/p/CONNECTIONSHARED/', 'image', 'official_api', \
          'authoritative_platform_state', 'available')",
    )
    .bind(media_id)
    .bind(account_id)
    .execute(pool)
    .await
    .expect("official media stores");
    let revision_id = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.media_revisions \
         (revision_id, media_id, raw_record_id, parser_version, resolved_at) \
         values ($1, $2, $3, 'instagram.official.v1', now())",
    )
    .bind(revision_id)
    .bind(media_id)
    .bind(raw_record_id)
    .execute(pool)
    .await
    .expect("official revision stores");
    sqlx::query("update instagram_archive.media set current_revision_id = $2 where media_id = $1")
        .bind(media_id)
        .bind(revision_id)
        .execute(pool)
        .await
        .expect("current revision links");
    let capture_id = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.captures \
         (capture_id, user_ref, media_id, canonical_url, acquisition_method, saved_authority, \
          client_source, status, captured_at) values ($1, $2, $3, \
          'https://www.instagram.com/p/CONNECTIONSHARED/', 'share_extension', \
          'explicit_user_capture', 'ios_share_extension', 'resolved', now())",
    )
    .bind(capture_id)
    .bind(owner)
    .bind(media_id)
    .execute(pool)
    .await
    .expect("explicit capture stores");

    DeletionStore::new(&test.database)
        .apply(DeletionRequest {
            operation_id: uuid::Uuid::now_v7(),
            user_ref: owner,
            target: DeletionTarget::Connection(account_id),
        })
        .await
        .expect("connection deletion succeeds");

    let counts: (i64, i64, i64, i64, i64, i64, Option<uuid::Uuid>) = sqlx::query_as(
        "select \
           (select count(*) from instagram_archive.accounts where account_id = $1), \
           (select count(*) from instagram_archive.credentials where account_id = $1), \
           (select count(*) from instagram_archive.captures where capture_id = $2), \
           (select count(*) from instagram_archive.media where media_id = $3), \
           (select count(*) from instagram_archive.media_revisions where media_id = $3), \
           (select count(*) from instagram_archive.raw_records where raw_record_id = $4), \
           (select account_id from instagram_archive.media where media_id = $3)",
    )
    .bind(account_id)
    .bind(capture_id)
    .bind(media_id)
    .bind(raw_record_id)
    .fetch_one(pool)
    .await
    .expect("connection cleanup counts read");
    assert_eq!(counts, (0, 0, 1, 1, 1, 1, None));

    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn connection_only_sources_each_emit_one_removal_and_completed_replay_is_idempotent() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let pool = test.database.pool();
    let owner = uuid::Uuid::now_v7();
    let account_id = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.accounts \
         (account_id, user_ref, provider_account_id, username, account_type, connection_status, \
          scopes, connected_at) values ($1, $2, 'provider-exclusive', 'redacted', \
          'business', 'connected', array['instagram_business_basic'], now())",
    )
    .bind(account_id)
    .bind(owner)
    .execute(pool)
    .await
    .expect("exclusive account stores");
    for (provider_id, suffix) in [("exclusive-one", "ONLYONE1"), ("exclusive-two", "ONLYTWO2")] {
        sqlx::query(
            "insert into instagram_archive.media \
             (media_id, account_id, provider_media_id, permalink, media_type, acquisition_method, \
              saved_authority, upstream_status) values ($1, $2, $3, $4, 'image', 'official_api', \
              'authoritative_platform_state', 'available')",
        )
        .bind(uuid::Uuid::now_v7())
        .bind(account_id)
        .bind(provider_id)
        .bind(format!("https://www.instagram.com/p/{suffix}/"))
        .execute(pool)
        .await
        .expect("connection-only media stores");
    }
    let request = DeletionRequest {
        operation_id: uuid::Uuid::now_v7(),
        user_ref: owner,
        target: DeletionTarget::Connection(account_id),
    };
    let store = DeletionStore::new(&test.database);
    let first = store.apply(request).await.expect("first deletion succeeds");
    let replay = store
        .apply(request)
        .await
        .expect("completed replay succeeds");
    assert_eq!(replay, first);

    let counts: (i64, i64, i64, i64, i64) = sqlx::query_as(
        "select \
           (select count(*) from instagram_archive.accounts where account_id = $1), \
           (select count(*) from instagram_archive.media where account_id = $1), \
           (select count(*) from instagram_archive.outbox_events \
            where event_type = 'social.source.removed.v1'), \
           (select count(*) from instagram_archive.local_source_removals \
            where operation_id = $2), \
           (select count(*) from instagram_archive.deletion_operations \
            where operation_id = $2)",
    )
    .bind(account_id)
    .bind(request.operation_id)
    .fetch_one(pool)
    .await
    .expect("replay counts read");
    assert_eq!(counts, (0, 0, 2, 2, 1));
    assert_eq!(
        first.effects.len(),
        OWNED_DATA_CLASSES.len(),
        "one total effect audit is retained"
    );

    test.cleanup().await.expect("cleanup must drop");
}
