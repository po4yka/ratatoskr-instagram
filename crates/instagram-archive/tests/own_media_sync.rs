//! Scheduled own-media behavior against disposable `PostgreSQL` and deterministic provider seams.

#![expect(
    clippy::expect_used,
    reason = "integration-test failures are assertions"
)]

use std::collections::BTreeMap;
use std::sync::Mutex;

use ratatoskr_instagram_archive::capability_reconciliation::{
    AccountObservation, AccountType, PermissionStatus,
};
use ratatoskr_instagram_archive::credentials::crypto::{
    CredentialKeyring, KEY_LEN, TokenBinding, TokenKind,
};
use ratatoskr_instagram_archive::own_media::{
    OwnMediaSyncConfig, OwnMediaSyncExecutor, OwnMediaSyncOutcome,
};
use ratatoskr_instagram_archive::provider::{
    ExchangedToken, InstagramProvider, ProviderAccount, ProviderError, ProviderFailureClass,
    ProviderFuture, ProviderOwnMediaItem, ProviderOwnMediaPage, ProviderPermissions,
};
use ratatoskr_instagram_archive::test_support::{
    FakeInstagramProvider, FakeProviderStep, TestDatabase,
};
use secrecy::SecretString;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

fn keyring() -> CredentialKeyring {
    CredentialKeyring::new(1, BTreeMap::from([(1, [7_u8; KEY_LEN])]))
        .expect("synthetic keyring is valid")
}

fn config() -> OwnMediaSyncConfig {
    OwnMediaSyncConfig {
        enabled: true,
        cadence_seconds: 3_600,
        accounts_per_tick: 8,
        pages_per_run: 8,
        call_budget: 8,
    }
}

#[tokio::test]
async fn disabled_scheduler_pass_is_an_immediate_truthful_noop() {
    let test = TestDatabase::create().await.expect("fresh database");
    let provider = FakeInstagramProvider::new([]);
    let keyring = keyring();
    let mut disabled = config();
    disabled.enabled = false;
    let summary = OwnMediaSyncExecutor::new(&test.database, &keyring, &provider, disabled)
        .run_due_once(OffsetDateTime::UNIX_EPOCH)
        .await
        .expect("disabled scheduler pass returns directly");
    assert_eq!(summary.attempted, 0);
    assert_eq!(provider.calls(), 0);
    test.cleanup().await.expect("cleanup must drop");
}

async fn account_with_capability(
    test: &TestDatabase,
    account_type: AccountType,
    permission: PermissionStatus,
) -> (Uuid, Uuid, Uuid) {
    let account_id = Uuid::now_v7();
    let user_ref = Uuid::now_v7();
    let raw_record_id = Uuid::now_v7();
    let account_type_wire = match account_type {
        AccountType::Business => "business",
        AccountType::Creator => "creator",
        AccountType::Personal => "personal",
        AccountType::Unknown => "unknown",
    };
    sqlx::query(
        "insert into instagram_archive.accounts
         (account_id, user_ref, provider_account_id, username, account_type,
          connection_status, scopes, connected_at)
         values ($1, $2, $3, 'synthetic', $4, 'connected', '{}', $5)",
    )
    .bind(account_id)
    .bind(user_ref)
    .bind(format!("provider-{account_id}"))
    .bind(account_type_wire)
    .bind(OffsetDateTime::UNIX_EPOCH)
    .execute(test.database.pool())
    .await
    .expect("account inserts");
    sqlx::query(
        "insert into instagram_archive.raw_records
         (raw_record_id, record_kind, blob_ref, content_hash, byte_size, body, observed_at)
         values ($1, 'api_response', $2, $3, 2, $4, $5)",
    )
    .bind(raw_record_id)
    .bind(format!("{:064x}", raw_record_id.as_u128()))
    .bind(vec![7_u8; 32])
    .bind(b"{}".to_vec())
    .bind(OffsetDateTime::UNIX_EPOCH)
    .execute(test.database.pool())
    .await
    .expect("raw evidence inserts");
    let generation_id = test
        .database
        .reconcile_account_capabilities(
            account_id,
            &AccountObservation {
                provider_account_id: format!("provider-{account_id}"),
                account_type,
                permissions: BTreeMap::from([("instagram_business_basic".to_owned(), permission)]),
                external_write_consent: false,
                observed_at: OffsetDateTime::UNIX_EPOCH,
                raw_record_id,
            },
        )
        .await
        .expect("capability generation persists");
    (account_id, user_ref, generation_id)
}

async fn store_credential(test: &TestDatabase, account_id: Uuid, keyring: &CredentialKeyring) {
    let envelope = keyring
        .seal(
            TokenBinding {
                subject_id: account_id,
                kind: TokenKind::Access,
            },
            &SecretString::from("synthetic-own-media-token"),
        )
        .expect("synthetic token seals");
    sqlx::query(
        "insert into instagram_archive.credentials
         (credential_id, account_id, access_token_envelope, key_version, granted_permissions)
         values ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(account_id)
    .bind(envelope)
    .bind(i32::try_from(keyring.current_version()).expect("test key version fits"))
    .bind(vec!["instagram_business_basic"])
    .execute(test.database.pool())
    .await
    .expect("credential inserts");
}

fn media_item(account_id: Uuid, provider_media_id: &str, caption: &str) -> ProviderOwnMediaItem {
    ProviderOwnMediaItem {
        provider_media_id: provider_media_id.to_owned(),
        owner_provider_account_id: format!("provider-{account_id}"),
        media_type: "IMAGE".to_owned(),
        media_product_type: "FEED".to_owned(),
        permalink: format!("https://www.instagram.com/p/{provider_media_id}/"),
        caption: Some(caption.to_owned()),
        published_at: OffsetDateTime::parse(
            "2026-08-27T08:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .expect("synthetic provider time parses"),
        media_url: Some(format!("https://cdn.example.test/{provider_media_id}.jpg")),
        thumbnail_url: None,
    }
}

fn media_page(items: Vec<ProviderOwnMediaItem>, next_cursor: Option<&str>) -> ProviderOwnMediaPage {
    ProviderOwnMediaPage {
        items,
        next_cursor: next_cursor.map(str::to_owned),
        raw_body: br#"{"data":"synthetic-own-media-page"}"#.to_vec(),
    }
}

#[derive(Debug)]
struct CapabilityDowngradingProvider {
    database: ratatoskr_instagram_archive::Database,
    account_id: Uuid,
    page: Mutex<Option<ProviderOwnMediaPage>>,
}

impl InstagramProvider for CapabilityDowngradingProvider {
    fn exchange_code<'a>(&'a self, _code: &'a SecretString) -> ProviderFuture<'a, ExchangedToken> {
        Box::pin(async { Err(unsupported_provider_error()) })
    }

    fn discover_account<'a>(
        &'a self,
        _access_token: &'a SecretString,
    ) -> ProviderFuture<'a, ProviderAccount> {
        Box::pin(async { Err(unsupported_provider_error()) })
    }

    fn discover_permissions<'a>(
        &'a self,
        _access_token: &'a SecretString,
    ) -> ProviderFuture<'a, ProviderPermissions> {
        Box::pin(async { Err(unsupported_provider_error()) })
    }

    fn refresh_token<'a>(
        &'a self,
        _access_token: &'a SecretString,
    ) -> ProviderFuture<'a, ExchangedToken> {
        Box::pin(async { Err(unsupported_provider_error()) })
    }

    fn revoke_token<'a>(&'a self, _access_token: &'a SecretString) -> ProviderFuture<'a, ()> {
        Box::pin(async { Err(unsupported_provider_error()) })
    }

    fn list_own_media_page<'a>(
        &'a self,
        _provider_account_id: &'a str,
        _access_token: &'a SecretString,
        _after: Option<&'a str>,
    ) -> ProviderFuture<'a, ProviderOwnMediaPage> {
        Box::pin(async move {
            sqlx::query(
                "update instagram_archive.account_capabilities
                 set generation_id = $2, capability_state = 'unavailable',
                     reason = 'permission_declined', observed_at = now()
                 where account_id = $1 and capability = 'own_media_read'",
            )
            .bind(self.account_id)
            .bind(Uuid::now_v7())
            .execute(self.database.pool())
            .await
            .map_err(|_| unsupported_provider_error())?;
            self.page
                .lock()
                .map_err(|_| unsupported_provider_error())?
                .take()
                .ok_or_else(unsupported_provider_error)
        })
    }
}

fn unsupported_provider_error() -> ProviderError {
    ProviderError {
        class: ProviderFailureClass::Unsupported,
        http_status: None,
    }
}

#[tokio::test]
async fn unsupported_account_job_records_noop_without_provider_contact() {
    let test = TestDatabase::create().await.expect("fresh database");
    let (account_id, _, _) =
        account_with_capability(&test, AccountType::Personal, PermissionStatus::Granted).await;
    let provider = FakeInstagramProvider::new([]);
    let keyring = keyring();
    let executor = OwnMediaSyncExecutor::new(&test.database, &keyring, &provider, config());

    let outcome = executor
        .run_account_once(account_id, OffsetDateTime::UNIX_EPOCH)
        .await
        .expect("unsupported account is a truthful terminal outcome");

    assert!(matches!(
        outcome,
        OwnMediaSyncOutcome::CapabilityNoop { ref reason, .. }
            if reason == "account_type_unsupported"
    ));
    assert_eq!(
        provider.calls(),
        0,
        "capability gate precedes provider contact"
    );
    let (runs, authority, state): (i64, i64, i64) = (
        sqlx::query_scalar(
            "select count(*) from instagram_archive.own_media_sync_runs
             where account_id = $1 and status = 'capability_noop'",
        )
        .bind(account_id)
        .fetch_one(test.database.pool())
        .await
        .expect("run count loads"),
        sqlx::query_scalar(
            "select count(*) from instagram_archive.own_media_authority where account_id = $1",
        )
        .bind(account_id)
        .fetch_one(test.database.pool())
        .await
        .expect("authority count loads"),
        sqlx::query_scalar(
            "select count(*) from instagram_archive.own_media_sync_state where account_id = $1",
        )
        .bind(account_id)
        .fetch_one(test.database.pool())
        .await
        .expect("state count loads"),
    );
    assert_eq!((runs, authority, state), (1, 0, 1));
    let watermark: Option<String> = sqlx::query_scalar(
        "select watermark_provider_media_id from instagram_archive.own_media_sync_state
         where account_id = $1",
    )
    .bind(account_id)
    .fetch_one(test.database.pool())
    .await
    .expect("no-op watermark loads");
    assert!(watermark.is_none());

    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn permission_downgrade_job_uses_current_generation_and_noops() {
    let test = TestDatabase::create().await.expect("fresh database");
    let (account_id, _, _) =
        account_with_capability(&test, AccountType::Business, PermissionStatus::Granted).await;
    let raw_record_id: Uuid = sqlx::query_scalar(
        "select raw_record_id from instagram_archive.raw_records order by observed_at limit 1",
    )
    .fetch_one(test.database.pool())
    .await
    .expect("raw evidence id loads");
    let current_generation = test
        .database
        .reconcile_account_capabilities(
            account_id,
            &AccountObservation {
                provider_account_id: format!("provider-{account_id}"),
                account_type: AccountType::Business,
                permissions: BTreeMap::from([(
                    "instagram_business_basic".to_owned(),
                    PermissionStatus::Declined,
                )]),
                external_write_consent: false,
                observed_at: OffsetDateTime::UNIX_EPOCH + Duration::seconds(1),
                raw_record_id,
            },
        )
        .await
        .expect("downgrade generation persists");
    let provider = FakeInstagramProvider::new([]);
    let keyring = keyring();
    let executor = OwnMediaSyncExecutor::new(&test.database, &keyring, &provider, config());

    executor
        .run_account_once(
            account_id,
            OffsetDateTime::UNIX_EPOCH + Duration::seconds(2),
        )
        .await
        .expect("downgrade becomes a no-op");

    let stored: (Uuid, String) = sqlx::query_as(
        "select capability_generation_id, outcome_reason
         from instagram_archive.own_media_sync_runs where account_id = $1",
    )
    .bind(account_id)
    .fetch_one(test.database.pool())
    .await
    .expect("no-op run loads");
    assert_eq!(
        stored,
        (current_generation, "permission_declined".to_owned())
    );
    assert_eq!(provider.calls(), 0);

    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn permission_downgrade_closes_a_stale_resumable_generation() {
    let test = TestDatabase::create().await.expect("fresh database");
    let (account_id, user_ref, stale_generation) =
        account_with_capability(&test, AccountType::Business, PermissionStatus::Granted).await;
    let stale_run_id = Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.own_media_sync_runs
         (run_id, account_id, user_ref, capability_generation_id, status,
          outcome_reason, started_at, updated_at)
         values ($1, $2, $3, $4, 'retryable', 'provider_retryable', $5, $5)",
    )
    .bind(stale_run_id)
    .bind(account_id)
    .bind(user_ref)
    .bind(stale_generation)
    .bind(OffsetDateTime::UNIX_EPOCH)
    .execute(test.database.pool())
    .await
    .expect("stale resumable run inserts");
    let raw_record_id: Uuid = sqlx::query_scalar(
        "select raw_record_id from instagram_archive.raw_records order by observed_at limit 1",
    )
    .fetch_one(test.database.pool())
    .await
    .expect("raw evidence id loads");
    let current_generation = test
        .database
        .reconcile_account_capabilities(
            account_id,
            &AccountObservation {
                provider_account_id: format!("provider-{account_id}"),
                account_type: AccountType::Business,
                permissions: BTreeMap::from([(
                    "instagram_business_basic".to_owned(),
                    PermissionStatus::Declined,
                )]),
                external_write_consent: false,
                observed_at: OffsetDateTime::UNIX_EPOCH + Duration::seconds(1),
                raw_record_id,
            },
        )
        .await
        .expect("downgrade generation persists");
    let provider = FakeInstagramProvider::new([]);
    let keyring = keyring();

    OwnMediaSyncExecutor::new(&test.database, &keyring, &provider, config())
        .run_account_once(
            account_id,
            OffsetDateTime::UNIX_EPOCH + Duration::seconds(2),
        )
        .await
        .expect("downgrade becomes a no-op");

    let active_count: i64 = sqlx::query_scalar(
        "select count(*) from instagram_archive.own_media_sync_runs
         where account_id = $1 and status in ('running', 'retryable')",
    )
    .bind(account_id)
    .fetch_one(test.database.pool())
    .await
    .expect("active count loads");
    let no_op_generation: Uuid = sqlx::query_scalar(
        "select capability_generation_id from instagram_archive.own_media_sync_runs
         where account_id = $1 and status = 'capability_noop'",
    )
    .bind(account_id)
    .fetch_one(test.database.pool())
    .await
    .expect("no-op generation loads");
    let stale_status: (String, String) = sqlx::query_as(
        "select status, outcome_reason from instagram_archive.own_media_sync_runs
         where run_id = $1",
    )
    .bind(stale_run_id)
    .fetch_one(test.database.pool())
    .await
    .expect("stale run loads");
    assert_eq!(active_count, 0, "stale generation cannot remain active");
    assert_eq!(no_op_generation, current_generation);
    assert_eq!(
        stale_status,
        ("failed".to_owned(), "capability_changed".to_owned())
    );
    assert_eq!(provider.calls(), 0);

    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn completed_incremental_scan_advances_watermark_after_reaching_prior_media() {
    let test = TestDatabase::create().await.expect("fresh database");
    let (account_id, _, _) =
        account_with_capability(&test, AccountType::Business, PermissionStatus::Granted).await;
    let keyring = keyring();
    store_credential(&test, account_id, &keyring).await;
    sqlx::query(
        "insert into instagram_archive.own_media_sync_state
         (account_id, watermark_provider_media_id, next_due_at, updated_at)
         values ($1, 'old-media', $2, $2)",
    )
    .bind(account_id)
    .bind(OffsetDateTime::UNIX_EPOCH)
    .execute(test.database.pool())
    .await
    .expect("prior watermark inserts");
    let provider = FakeInstagramProvider::new([FakeProviderStep::OwnMedia(Ok(media_page(
        vec![
            media_item(account_id, "new-media", "new"),
            media_item(account_id, "old-media", "old"),
        ],
        Some("unneeded-after-watermark"),
    )))]);
    let executor = OwnMediaSyncExecutor::new(&test.database, &keyring, &provider, config());

    let outcome = executor
        .run_account_once(
            account_id,
            OffsetDateTime::UNIX_EPOCH + Duration::seconds(3),
        )
        .await
        .expect("incremental traversal completes");
    assert!(matches!(
        outcome,
        OwnMediaSyncOutcome::Completed {
            ref watermark_provider_media_id,
            ..
        } if watermark_provider_media_id.as_deref() == Some("new-media")
    ));
    let watermark: Option<String> = sqlx::query_scalar(
        "select watermark_provider_media_id from instagram_archive.own_media_sync_state
         where account_id = $1",
    )
    .bind(account_id)
    .fetch_one(test.database.pool())
    .await
    .expect("watermark loads");
    assert_eq!(watermark.as_deref(), Some("new-media"));
    assert_eq!(provider.calls(), 1, "old watermark stops pagination");

    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn failed_scan_retains_watermark_and_resumes_committed_cursor() {
    let test = TestDatabase::create().await.expect("fresh database");
    let (account_id, _, _) =
        account_with_capability(&test, AccountType::Business, PermissionStatus::Granted).await;
    let keyring = keyring();
    store_credential(&test, account_id, &keyring).await;
    sqlx::query(
        "insert into instagram_archive.own_media_sync_state
         (account_id, watermark_provider_media_id, next_due_at, updated_at)
         values ($1, 'old-media', $2, $2)",
    )
    .bind(account_id)
    .bind(OffsetDateTime::UNIX_EPOCH)
    .execute(test.database.pool())
    .await
    .expect("prior watermark inserts");
    let failing = FakeInstagramProvider::new([
        FakeProviderStep::OwnMedia(Ok(media_page(
            vec![media_item(account_id, "new-media", "new")],
            Some("cursor-2"),
        ))),
        FakeProviderStep::OwnMedia(Err(ProviderError {
            class: ProviderFailureClass::Network,
            http_status: None,
        })),
    ]);
    let first = OwnMediaSyncExecutor::new(&test.database, &keyring, &failing, config())
        .run_account_once(
            account_id,
            OffsetDateTime::UNIX_EPOCH + Duration::seconds(3),
        )
        .await
        .expect("network failure is retryable");
    assert!(matches!(
        first,
        OwnMediaSyncOutcome::Retryable {
            ref next_cursor,
            ..
        } if next_cursor.as_deref() == Some("cursor-2")
    ));
    let watermark: Option<String> = sqlx::query_scalar(
        "select watermark_provider_media_id from instagram_archive.own_media_sync_state
         where account_id = $1",
    )
    .bind(account_id)
    .fetch_one(test.database.pool())
    .await
    .expect("watermark loads");
    assert_eq!(watermark.as_deref(), Some("old-media"));

    let resumed = FakeInstagramProvider::new([FakeProviderStep::OwnMedia(Ok(media_page(
        vec![media_item(account_id, "old-media", "old")],
        None,
    )))]);
    let second = OwnMediaSyncExecutor::new(&test.database, &keyring, &resumed, config())
        .run_account_once(
            account_id,
            OffsetDateTime::UNIX_EPOCH + Duration::seconds(4),
        )
        .await
        .expect("retry resumes the active run");
    assert!(matches!(second, OwnMediaSyncOutcome::Completed { .. }));
    assert_eq!(
        resumed.own_media_cursors(),
        vec![Some("cursor-2".to_owned())]
    );
    let active: i64 = sqlx::query_scalar(
        "select count(*) from instagram_archive.own_media_sync_runs
         where account_id = $1 and status in ('running', 'retryable')",
    )
    .bind(account_id)
    .fetch_one(test.database.pool())
    .await
    .expect("active run count loads");
    assert_eq!(active, 0);

    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn completion_atomically_swaps_retained_refreshed_and_new_media() {
    let test = TestDatabase::create().await.expect("fresh database");
    let (account_id, _, _) =
        account_with_capability(&test, AccountType::Creator, PermissionStatus::Granted).await;
    let keyring = keyring();
    store_credential(&test, account_id, &keyring).await;
    let initial = FakeInstagramProvider::new([FakeProviderStep::OwnMedia(Ok(media_page(
        vec![
            media_item(account_id, "watermark-old", "watermark v1"),
            media_item(account_id, "retained", "retained v1"),
            media_item(account_id, "refreshed", "refresh v1"),
        ],
        None,
    )))]);
    OwnMediaSyncExecutor::new(&test.database, &keyring, &initial, config())
        .run_account_once(
            account_id,
            OffsetDateTime::UNIX_EPOCH + Duration::seconds(10),
        )
        .await
        .expect("initial full traversal completes");
    let prior_run: Uuid = sqlx::query_scalar(
        "select run_id from instagram_archive.own_media_authority where account_id = $1",
    )
    .bind(account_id)
    .fetch_one(test.database.pool())
    .await
    .expect("prior authority loads");

    let incremental = FakeInstagramProvider::new([FakeProviderStep::OwnMedia(Ok(media_page(
        vec![
            media_item(account_id, "new", "new v1"),
            media_item(account_id, "refreshed", "refresh v2"),
            media_item(account_id, "watermark-old", "watermark v1"),
        ],
        Some("unneeded"),
    )))]);
    OwnMediaSyncExecutor::new(&test.database, &keyring, &incremental, config())
        .run_account_once(
            account_id,
            OffsetDateTime::UNIX_EPOCH + Duration::seconds(20),
        )
        .await
        .expect("incremental traversal completes");

    let current_run: Uuid = sqlx::query_scalar(
        "select run_id from instagram_archive.own_media_authority where account_id = $1",
    )
    .bind(account_id)
    .fetch_one(test.database.pool())
    .await
    .expect("current authority loads");
    assert_ne!(current_run, prior_run);
    let current: Vec<(String, Option<String>)> = sqlx::query_as(
        "select provider_media_id, caption from instagram_archive.own_media_sync_items
         where run_id = $1 order by provider_media_id",
    )
    .bind(current_run)
    .fetch_all(test.database.pool())
    .await
    .expect("current set loads");
    assert_eq!(
        current,
        vec![
            ("new".to_owned(), Some("new v1".to_owned())),
            ("refreshed".to_owned(), Some("refresh v2".to_owned())),
            ("retained".to_owned(), Some("retained v1".to_owned())),
            ("watermark-old".to_owned(), Some("watermark v1".to_owned())),
        ]
    );

    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn capability_generation_change_before_completion_preserves_prior_authority() {
    let test = TestDatabase::create().await.expect("fresh database");
    let (account_id, _, _) =
        account_with_capability(&test, AccountType::Business, PermissionStatus::Granted).await;
    let keyring = keyring();
    store_credential(&test, account_id, &keyring).await;
    let initial = FakeInstagramProvider::new([FakeProviderStep::OwnMedia(Ok(media_page(
        vec![media_item(account_id, "old-media", "old")],
        None,
    )))]);
    OwnMediaSyncExecutor::new(&test.database, &keyring, &initial, config())
        .run_account_once(
            account_id,
            OffsetDateTime::UNIX_EPOCH + Duration::seconds(30),
        )
        .await
        .expect("initial authority completes");
    let prior_authority: Uuid = sqlx::query_scalar(
        "select run_id from instagram_archive.own_media_authority where account_id = $1",
    )
    .bind(account_id)
    .fetch_one(test.database.pool())
    .await
    .expect("prior authority loads");
    let provider = CapabilityDowngradingProvider {
        database: test.database.clone(),
        account_id,
        page: Mutex::new(Some(media_page(
            vec![
                media_item(account_id, "new-media", "new"),
                media_item(account_id, "old-media", "old"),
            ],
            None,
        ))),
    };

    let result = OwnMediaSyncExecutor::new(&test.database, &keyring, &provider, config())
        .run_account_once(
            account_id,
            OffsetDateTime::UNIX_EPOCH + Duration::seconds(40),
        )
        .await;
    assert!(result.is_err(), "capability drift refuses completion");
    let authority: Uuid = sqlx::query_scalar(
        "select run_id from instagram_archive.own_media_authority where account_id = $1",
    )
    .bind(account_id)
    .fetch_one(test.database.pool())
    .await
    .expect("authority loads");
    assert_eq!(authority, prior_authority);
    let watermark: Option<String> = sqlx::query_scalar(
        "select watermark_provider_media_id from instagram_archive.own_media_sync_state
         where account_id = $1",
    )
    .bind(account_id)
    .fetch_one(test.database.pool())
    .await
    .expect("watermark loads");
    assert_eq!(watermark.as_deref(), Some("old-media"));

    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn metadata_only_completion_publishes_official_fact_with_raw_blob_and_warning() {
    let test = TestDatabase::create().await.expect("fresh database");
    let (account_id, _, _) =
        account_with_capability(&test, AccountType::Business, PermissionStatus::Granted).await;
    let keyring = keyring();
    store_credential(&test, account_id, &keyring).await;
    let provider = FakeInstagramProvider::new([FakeProviderStep::OwnMedia(Ok(media_page(
        vec![media_item(account_id, "publish-media", "published caption")],
        None,
    )))]);
    OwnMediaSyncExecutor::new(&test.database, &keyring, &provider, config())
        .run_account_once(
            account_id,
            OffsetDateTime::UNIX_EPOCH + Duration::seconds(50),
        )
        .await
        .expect("metadata-only run completes");

    let payload: serde_json::Value = sqlx::query_scalar(
        "select payload from instagram_archive.outbox_events
         where aggregate_type = 'media' and event_type = 'social.source.captured.v1'",
    )
    .fetch_one(test.database.pool())
    .await
    .expect("own-media captured fact loads");
    let source = &payload["payload"]["source"];
    assert_eq!(source["acquisition"], "official_api");
    assert_eq!(source["saved_authority"], "authoritative_platform_state");
    assert_eq!(source["external_post_id"], "publish-media");
    assert_eq!(source["completeness"], "partial");
    assert_eq!(source["checkpoint"], "publish-media");
    assert!(source["raw_blob"].is_object());
    assert!(source["media"].as_array().is_none_or(Vec::is_empty));
    assert_eq!(
        source["warnings"][0]["code"],
        "social.source.media_not_archived"
    );
    let serialized = payload.to_string();
    assert!(!serialized.contains("cdn.example.test"));
    assert!(!serialized.contains("synthetic-own-media-token"));

    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn unchanged_generation_emits_no_duplicate_and_changed_metadata_emits_one_update() {
    let test = TestDatabase::create().await.expect("fresh database");
    let (account_id, _, _) =
        account_with_capability(&test, AccountType::Business, PermissionStatus::Granted).await;
    let keyring = keyring();
    store_credential(&test, account_id, &keyring).await;
    for (second, caption) in [
        (60_i64, "caption v1"),
        (70, "caption v1"),
        (80, "caption v2"),
    ] {
        let provider = FakeInstagramProvider::new([FakeProviderStep::OwnMedia(Ok(media_page(
            vec![media_item(account_id, "stable-media", caption)],
            None,
        )))]);
        OwnMediaSyncExecutor::new(&test.database, &keyring, &provider, config())
            .run_account_once(
                account_id,
                OffsetDateTime::UNIX_EPOCH + Duration::seconds(second),
            )
            .await
            .expect("generation completes");
    }

    let counts: Vec<(String, i64)> = sqlx::query_as(
        "select event_type, count(*) from instagram_archive.outbox_events
         where aggregate_type = 'media' group by event_type order by event_type",
    )
    .fetch_all(test.database.pool())
    .await
    .expect("event counts load");
    assert_eq!(
        counts,
        vec![
            ("social.source.captured.v1".to_owned(), 1),
            ("social.source.updated.v1".to_owned(), 1),
        ]
    );
    let source_ids: Vec<String> = sqlx::query_scalar(
        "select payload #>> '{payload,source,social_source_id}'
         from instagram_archive.outbox_events where aggregate_type = 'media'
         order by occurred_at",
    )
    .fetch_all(test.database.pool())
    .await
    .expect("source identities load");
    assert_eq!(source_ids.len(), 2);
    assert_eq!(source_ids.first(), source_ids.get(1));

    test.cleanup().await.expect("cleanup must drop");
}
