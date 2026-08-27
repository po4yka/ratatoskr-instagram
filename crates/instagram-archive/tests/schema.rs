//! Schema contract: what exists after a fresh apply, that application is
//! idempotent, and that provenance vocabularies are enforced by the database.
//!
//! These tests talk to real `PostgreSQL`; `INSTAGRAM_ARCHIVE_TEST_DATABASE_URL`
//! selects the server and defaults to the `compose.yaml` endpoint. A missing
//! server is a failure, never a skip.

use uuid::Uuid;

use ratatoskr_instagram_archive::Database;
use ratatoskr_instagram_archive::test_support::{TestDatabase, admin_url};

/// The relations README.md's planned data model declares, no more, no fewer.
const DECLARED_TABLES: [&str; 25] = [
    "accounts",
    "account_capabilities",
    "account_credential_audit",
    "account_permission_observations",
    "credentials",
    "oauth_flows",
    "provider_api_usage",
    "profiles",
    "media",
    "media_relations",
    "media_revisions",
    "own_media_sync_state",
    "own_media_sync_runs",
    "own_media_sync_items",
    "own_media_authority",
    "captures",
    "capture_tombstones",
    "capture_analysis_links",
    "capture_notes",
    "export_snapshots",
    "import_runs",
    "raw_records",
    "availability_observations",
    "outbox_events",
    "inbox_events",
];

#[tokio::test]
async fn official_oauth_tables_match_the_current_schema() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let tables = archive_tables(test.database.pool()).await;
    for required in [
        "oauth_flows",
        "account_permission_observations",
        "account_capabilities",
        "account_credential_audit",
        "provider_api_usage",
    ] {
        assert!(
            tables.iter().any(|table| table == required),
            "official OAuth table {required} must exist"
        );
    }
    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn own_media_schema_carries_resumable_runs_watermarks_and_atomic_authority() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let tables = archive_tables(test.database.pool()).await;
    let missing = [
        "own_media_sync_state",
        "own_media_sync_runs",
        "own_media_sync_items",
        "own_media_authority",
    ]
    .into_iter()
    .filter(|required| !tables.iter().any(|table| table == required))
    .collect::<Vec<_>>();

    assert!(
        missing.is_empty(),
        "own-media schema objects must exist; missing {missing:?}"
    );

    let required_columns: i64 = sqlx::query_scalar(
        "select count(*) from information_schema.columns
         where table_schema = 'instagram_archive'
           and (table_name, column_name) in (
             ('own_media_sync_state', 'watermark_provider_media_id'),
             ('own_media_sync_state', 'next_due_at'),
             ('own_media_sync_runs', 'next_cursor'),
             ('own_media_sync_runs', 'candidate_watermark_provider_media_id'),
             ('own_media_sync_items', 'raw_record_id'),
             ('own_media_authority', 'run_id')
           )",
    )
    .fetch_one(test.database.pool())
    .await
    .expect("the catalog query answers");
    assert_eq!(
        required_columns, 6,
        "checkpoint, watermark, evidence, and authority columns must all exist"
    );

    let active_run_index: Option<String> = sqlx::query_scalar(
        "select coalesce(pg_get_expr(indexprs, indrelid), '') || coalesce(pg_get_expr(indpred, indrelid), '')
         from pg_index
         where indrelid = 'instagram_archive.own_media_sync_runs'::regclass
           and indisunique and indpred is not null",
    )
    .fetch_optional(test.database.pool())
    .await
    .expect("the index catalog query answers");
    assert!(
        active_run_index
            .as_deref()
            .is_some_and(|predicate| predicate.contains("retryable")),
        "one partial unique index must cover running and retryable runs"
    );

    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn credentials_are_unique_per_account() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let pool = test.database.pool();
    let account_id = Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.accounts
         (account_id, user_ref, provider_account_id, username, account_type,
          connection_status, scopes, connected_at)
         values ($1, $2, 'provider-account', 'redacted', 'business', 'connected',
                 array['instagram_business_basic'], now())",
    )
    .bind(account_id)
    .bind(Uuid::now_v7())
    .execute(pool)
    .await
    .expect("account inserts");
    let insert = "insert into instagram_archive.credentials
         (credential_id, account_id, access_token_envelope, key_version,
          granted_permissions, expires_at)
         values ($1, $2, $3, 1, array['instagram_business_basic'], now())";
    sqlx::query(insert)
        .bind(Uuid::now_v7())
        .bind(account_id)
        .bind(vec![1_u8; 32])
        .execute(pool)
        .await
        .expect("first credential inserts");
    let duplicate = sqlx::query(insert)
        .bind(Uuid::now_v7())
        .bind(account_id)
        .bind(vec![2_u8; 32])
        .execute(pool)
        .await;
    assert!(
        duplicate.is_err(),
        "a second active credential for one account must be refused"
    );
    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn usage_rows_carry_no_request_payload_columns() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let forbidden: Vec<String> = sqlx::query_scalar(
        "select column_name from information_schema.columns
         where table_schema = 'instagram_archive' and table_name = 'provider_api_usage'
           and column_name = any(array['url', 'request_url', 'request_body', 'response_body',
                                       'headers', 'token', 'authorization_code'])",
    )
    .fetch_all(test.database.pool())
    .await
    .expect("the catalog query answers");
    assert!(
        forbidden.is_empty(),
        "usage ledger leaks request data: {forbidden:?}"
    );
    let columns: i64 = sqlx::query_scalar(
        "select count(*) from information_schema.columns
         where table_schema = 'instagram_archive' and table_name = 'provider_api_usage'",
    )
    .fetch_one(test.database.pool())
    .await
    .expect("the catalog query answers");
    assert!(columns > 0, "provider_api_usage must exist");
    test.cleanup().await.expect("cleanup must drop");
}

const INSERT_CAPTURE: &str = "insert into instagram_archive.captures \
     (capture_id, user_ref, canonical_url, acquisition_method, saved_authority, \
      client_source, status, captured_at) \
     values ($1, $2, $3, $4, $5, $6, $7, now())";

const ACQUISITIONS: [&str; 5] = [
    "official_api",
    "share_extension",
    "browser_extension",
    "data_export",
    "legacy_import",
];

const AUTHORITIES: [&str; 4] = [
    "explicit_user_capture",
    "export_observation",
    "authoritative_platform_state",
    "legacy_observation",
];

#[expect(
    clippy::expect_used,
    reason = "integration-test helper: an unanswered catalog query is the failure"
)]
async fn archive_tables(pool: &sqlx::PgPool) -> Vec<String> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "select table_name from information_schema.tables \
         where table_schema = 'instagram_archive' order by table_name",
    )
    .fetch_all(pool)
    .await
    .expect("the catalog query must answer");
    rows.into_iter().map(|(name,)| name).collect()
}

async fn connect_shared(
    name: &str,
) -> Result<Database, ratatoskr_instagram_archive::PersistenceError> {
    let base = admin_url();
    let (prefix, _) = base.rsplit_once('/').unwrap_or(("", ""));
    let url = format!("{prefix}/{name}");
    Database::connect(&url, 2, std::time::Duration::from_secs(5)).await
}

#[tokio::test]
async fn fresh_apply_creates_exactly_the_declared_relations() {
    let test = TestDatabase::create().await.expect("a fresh test database");

    let tables = archive_tables(test.database.pool()).await;
    let mut declared = DECLARED_TABLES.map(str::to_owned);
    declared.sort_unstable();
    assert_eq!(
        tables, declared,
        "the applied schema must match the declared inventory exactly"
    );

    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn second_apply_succeeds_identically() {
    let test = TestDatabase::create_raw().await.expect("a raw database");

    test.database.apply_schema().await.expect("first apply");
    let before = archive_tables(test.database.pool()).await;
    test.database.apply_schema().await.expect("second apply");
    let after = archive_tables(test.database.pool()).await;

    assert_eq!(before.len(), DECLARED_TABLES.len());
    assert_eq!(before, after, "a second apply must change nothing");

    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn concurrent_applications_both_succeed_applying_once() {
    let test = TestDatabase::create_raw()
        .await
        .expect("a raw database for two racers");

    let one = connect_shared(test.name())
        .await
        .expect("racer one connects");
    let two = connect_shared(test.name())
        .await
        .expect("racer two connects");

    let (first, second) = tokio::join!(one.apply_schema(), two.apply_schema());
    first.expect("the first concurrent application succeeds");
    second.expect("the second concurrent application succeeds");

    let tables = archive_tables(one.pool()).await;
    assert_eq!(tables.len(), DECLARED_TABLES.len(), "applied exactly once");

    one.close().await;
    two.close().await;
    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn unknown_acquisition_method_is_refused_and_documented_values_are_accepted() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let pool = test.database.pool();

    let refused = sqlx::query(INSERT_CAPTURE)
        .bind(Uuid::now_v7())
        .bind(Uuid::now_v7())
        .bind("https://www.instagram.com/reel/example/")
        .bind("carrier_pigeon")
        .bind("explicit_user_capture")
        .bind("ios_share_extension")
        .bind("accepted")
        .execute(pool)
        .await;
    let error = refused.expect_err("an unknown acquisition method must be refused");
    assert!(
        error
            .to_string()
            .contains("captures_acquisition_method_check"),
        "the named CHECK constraint must reject it: {error}"
    );

    for acquisition in ACQUISITIONS {
        let inserted = sqlx::query(INSERT_CAPTURE)
            .bind(Uuid::now_v7())
            .bind(Uuid::now_v7())
            .bind("https://www.instagram.com/p/example/")
            .bind(acquisition)
            .bind("explicit_user_capture")
            .bind("ios_share_extension")
            .bind("accepted")
            .execute(pool)
            .await;
        assert!(
            inserted.is_ok(),
            "documented acquisition {acquisition} must be accepted: {:?}",
            inserted.err().map(|e| e.to_string())
        );
    }
    for authority in AUTHORITIES {
        let inserted = sqlx::query(INSERT_CAPTURE)
            .bind(Uuid::now_v7())
            .bind(Uuid::now_v7())
            .bind("https://www.instagram.com/reel/example/")
            .bind("share_extension")
            .bind(authority)
            .bind("browser_extension")
            .bind("accepted")
            .execute(pool)
            .await;
        assert!(
            inserted.is_ok(),
            "documented authority {authority} must be accepted: {:?}",
            inserted.err().map(|e| e.to_string())
        );
    }

    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn public_resolution_is_accepted_on_provenance_tables() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let pool = test.database.pool();

    let media = sqlx::query(
        "insert into instagram_archive.media \
         (media_id, permalink, media_type, acquisition_method, saved_authority, upstream_status) \
         values ($1, $2, 'image', 'public_resolution', 'explicit_user_capture', 'available')",
    )
    .bind(Uuid::now_v7())
    .bind("https://www.instagram.com/p/example/")
    .execute(pool)
    .await;
    assert!(
        media.is_ok(),
        "public_resolution must be accepted on media: {:?}",
        media.err().map(|e| e.to_string())
    );

    let capture = sqlx::query(INSERT_CAPTURE)
        .bind(Uuid::now_v7())
        .bind(Uuid::now_v7())
        .bind("https://www.instagram.com/reel/example/")
        .bind("public_resolution")
        .bind("explicit_user_capture")
        .bind("telegram")
        .bind("accepted")
        .execute(pool)
        .await;
    assert!(
        capture.is_ok(),
        "public_resolution must be accepted on captures: {:?}",
        capture.err().map(|e| e.to_string())
    );

    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn catalog_shows_zero_cross_schema_foreign_keys() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let pool = test.database.pool();

    let crossing: i64 = sqlx::query_scalar(
        "select count(*) from pg_constraint con \
         join pg_class rel on rel.oid = con.conrelid \
         join pg_namespace nsp on nsp.oid = rel.relnamespace \
         where nsp.nspname = 'instagram_archive' \
           and con.contype = 'f' \
           and exists ( \
               select 1 from pg_class other \
               join pg_namespace onsp on onsp.oid = other.relnamespace \
               where other.oid = con.confrelid and onsp.nspname <> 'instagram_archive')",
    )
    .fetch_one(pool)
    .await
    .expect("the catalog query must answer");

    assert_eq!(crossing, 0, "no foreign key may leave instagram_archive");

    test.cleanup().await.expect("cleanup must drop");
}

async fn insert_capture(
    pool: &sqlx::PgPool,
    capture_id: Uuid,
    user_ref: Uuid,
    canonical_url: &str,
) -> sqlx::Result<sqlx::postgres::PgQueryResult> {
    sqlx::query(INSERT_CAPTURE)
        .bind(capture_id)
        .bind(user_ref)
        .bind(canonical_url)
        .bind("share_extension")
        .bind("explicit_user_capture")
        .bind("ios_share_extension")
        .bind("accepted")
        .execute(pool)
        .await
}

#[tokio::test]
async fn duplicate_user_and_canonical_url_pair_is_refused_by_uniqueness() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let pool = test.database.pool();
    let user = Uuid::now_v7();
    let url = "https://www.instagram.com/p/DHcxI7hpS5t/";

    insert_capture(pool, Uuid::now_v7(), user, url)
        .await
        .expect("the first capture for the pair inserts");
    let duplicate = insert_capture(pool, Uuid::now_v7(), user, url).await;

    let error = duplicate.expect_err("a duplicate (user_ref, canonical_url) must be refused");
    assert!(
        error.to_string().contains("captures_user_canonical_key"),
        "the named uniqueness constraint must reject it: {error}"
    );

    // Distinct pairs are not duplicates.
    insert_capture(
        pool,
        Uuid::now_v7(),
        user,
        "https://www.instagram.com/reel/DHab_c9-x/",
    )
    .await
    .expect("same user, different URL inserts");
    insert_capture(pool, Uuid::now_v7(), Uuid::now_v7(), url)
        .await
        .expect("different user, same URL inserts");

    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn captures_carry_a_nullable_client_idempotency_key() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let pool = test.database.pool();

    let present: Option<String> = sqlx::query_scalar(
        "select column_name from information_schema.columns \
         where table_schema = 'instagram_archive' and table_name = 'captures' \
           and column_name = 'client_idempotency_key'",
    )
    .fetch_one(pool)
    .await
    .expect("the catalog query answers");
    assert!(
        present.is_some(),
        "captures.client_idempotency_key must exist for platform correlation"
    );

    let capture_id = Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.captures \
         (capture_id, user_ref, canonical_url, acquisition_method, saved_authority, \
          client_source, status, captured_at, client_idempotency_key) \
         values ($1, $2, $3, $4, $5, $6, $7, now(), $8)",
    )
    .bind(capture_id)
    .bind(Uuid::now_v7())
    .bind("https://www.instagram.com/p/DHcxI7hpS5t/")
    .bind("share_extension")
    .bind("explicit_user_capture")
    .bind("ios_share_extension")
    .bind("accepted")
    .bind("018f1a2b-3c4d-5e6f-8a9b-0c1d2e3f4a5b")
    .execute(pool)
    .await
    .expect("a capture with a client idempotency key inserts");
    let stored: Option<String> = sqlx::query_scalar(
        "select client_idempotency_key from instagram_archive.captures where capture_id = $1",
    )
    .bind(capture_id)
    .fetch_one(pool)
    .await
    .expect("the stored row is readable");
    assert_eq!(
        stored.as_deref(),
        Some("018f1a2b-3c4d-5e6f-8a9b-0c1d2e3f4a5b"),
        "the client key round-trips"
    );

    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn harness_databases_are_isolated_and_cleanup_drops_them() {
    const INSERT_ACCOUNT: &str = "insert into instagram_archive.accounts \
         (account_id, user_ref, provider_account_id, username, account_type, \
          connection_status, scopes, connected_at) \
         values ($1, $1, $2, 'who', 'business', 'connected', '{}', now())";

    let one = TestDatabase::create().await.expect("database one");
    let two = TestDatabase::create().await.expect("database two");
    assert_ne!(one.name(), two.name());

    for db in [&one, &two] {
        sqlx::query(INSERT_ACCOUNT)
            .bind(Uuid::now_v7())
            .bind(format!("p-{}", Uuid::now_v7()))
            .execute(db.database.pool())
            .await
            .expect("the marker row inserts");
    }

    let one_rows: i64 = sqlx::query_scalar("select count(*) from instagram_archive.accounts")
        .fetch_one(one.database.pool())
        .await
        .expect("count in database one");
    assert_eq!(one_rows, 1, "isolation: each database sees only its row");

    let name_one = one.name().to_owned();
    let name_two = two.name().to_owned();
    one.cleanup().await.expect("drop one");
    two.cleanup().await.expect("drop two");

    let admin = Database::connect(&admin_url(), 1, std::time::Duration::from_secs(5))
        .await
        .expect("admin pool connects");
    let remaining: Vec<(String,)> =
        sqlx::query_as("select datname from pg_database where datname = any($1)")
            .bind(vec![name_one.clone(), name_two.clone()])
            .fetch_all(admin.pool())
            .await
            .expect("existence check answers");
    assert!(
        remaining.is_empty(),
        "both databases must be gone: {remaining:?}"
    );
    admin.close().await;
}
