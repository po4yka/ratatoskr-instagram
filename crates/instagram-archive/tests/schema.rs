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
const DECLARED_TABLES: [&str; 13] = [
    "accounts",
    "credentials",
    "profiles",
    "media",
    "media_relations",
    "captures",
    "capture_notes",
    "export_snapshots",
    "import_runs",
    "raw_records",
    "availability_observations",
    "outbox_events",
    "inbox_events",
];

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

#[tokio::test]
async fn harness_databases_are_isolated_and_cleanup_drops_them() {
    const INSERT_ACCOUNT: &str = "insert into instagram_archive.accounts \
         (account_id, user_ref, provider_account_id, username, account_type, \
          connection_status, scopes, connected_at) \
         values ($1, $1, $2, 'who', 'business', 'connected', '', now())";

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
