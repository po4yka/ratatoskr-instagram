//! Official account lifecycle safety, especially unconditional local revoke.

#![expect(
    clippy::expect_used,
    reason = "integration-test setup failures are assertions"
)]

use ratatoskr_instagram_archive::account::ProviderRevokeOutcome;
use ratatoskr_instagram_archive::capability_reconciliation::{
    AccountCapability, AccountObservation, AccountType, CapabilityState, PermissionStatus,
};
use ratatoskr_instagram_archive::credentials::crypto::{
    CredentialKeyring, KEY_LEN, TokenBinding, TokenKind,
};
use ratatoskr_instagram_archive::provider::{
    BASIC_READ_SCOPE, ExchangedToken, ProviderAccount, ProviderError, ProviderFailureClass,
    ProviderPermissions,
};
use ratatoskr_instagram_archive::test_support::{
    FakeInstagramProvider, FakeProviderStep, TestDatabase,
};
use secrecy::SecretString;
use sqlx::Row as _;
use time::OffsetDateTime;
use uuid::Uuid;

const SENTINEL: &str = "SYNTHETIC_REVOKE_SECRET_SENTINEL";

fn keyring() -> CredentialKeyring {
    CredentialKeyring::new(7, std::collections::BTreeMap::from([(7, [0x42; KEY_LEN])]))
        .expect("test key exists")
}

async fn refreshable_account(test: &TestDatabase) -> (Uuid, Uuid, Vec<u8>) {
    let account_id = Uuid::now_v7();
    let user_ref = Uuid::now_v7();
    let old_envelope = keyring()
        .seal(
            TokenBinding {
                subject_id: account_id,
                kind: TokenKind::Access,
            },
            &SecretString::from("SYNTHETIC_OLD_ACCESS_TOKEN"),
        )
        .expect("old token seals");
    sqlx::query(
        "insert into instagram_archive.accounts
         (account_id, user_ref, provider_account_id, username, account_type,
          connection_status, scopes, connected_at)
         values ($1, $2, $3, 'old_name', 'business', 'connected',
                 array['instagram_business_basic'], now())",
    )
    .bind(account_id)
    .bind(user_ref)
    .bind(format!("provider-{account_id}"))
    .execute(test.database.pool())
    .await
    .expect("account inserts");
    sqlx::query(
        "insert into instagram_archive.credentials
         (credential_id, account_id, access_token_envelope, key_version,
          granted_permissions, expires_at)
         values ($1, $2, $3, 7, array['instagram_business_basic'], now())",
    )
    .bind(Uuid::now_v7())
    .bind(account_id)
    .bind(&old_envelope)
    .execute(test.database.pool())
    .await
    .expect("credential inserts");
    let raw_id = Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.raw_records
         (raw_record_id, record_kind, blob_ref, content_hash, byte_size, body, observed_at)
         values ($1, 'api_response', $2, $3, 2, $4, now())",
    )
    .bind(raw_id)
    .bind(format!("{:064x}", raw_id.as_u128()))
    .bind(vec![3_u8; 32])
    .bind(b"{}".to_vec())
    .execute(test.database.pool())
    .await
    .expect("raw inserts");
    test.database
        .reconcile_account_capabilities(
            account_id,
            &AccountObservation {
                provider_account_id: format!("provider-{account_id}"),
                account_type: AccountType::Business,
                permissions: std::collections::BTreeMap::from([(
                    BASIC_READ_SCOPE.to_owned(),
                    PermissionStatus::Granted,
                )]),
                external_write_consent: false,
                observed_at: OffsetDateTime::UNIX_EPOCH,
                raw_record_id: raw_id,
            },
        )
        .await
        .expect("initial capabilities reconcile");
    (account_id, user_ref, old_envelope)
}

fn successful_refresh_steps(
    account_id: Uuid,
    basic_status: PermissionStatus,
) -> [FakeProviderStep; 3] {
    [
        FakeProviderStep::Refresh(Ok(ExchangedToken {
            access_token: SecretString::from("SYNTHETIC_NEW_ACCESS_TOKEN"),
            user_id: String::new(),
            permissions: Vec::new(),
            expires_in_seconds: Some(3_600),
        })),
        FakeProviderStep::Account(Ok(ProviderAccount {
            provider_account_id: format!("provider-{account_id}"),
            username: "new_name".to_owned(),
            account_type: AccountType::Business,
        })),
        FakeProviderStep::Permissions(Ok(ProviderPermissions {
            statuses: std::collections::BTreeMap::from([(
                BASIC_READ_SCOPE.to_owned(),
                basic_status,
            )]),
        })),
    ]
}

async fn seeded_account(test: &TestDatabase, status: &str) -> (Uuid, Uuid) {
    let account_id = Uuid::now_v7();
    let user_ref = Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.accounts
         (account_id, user_ref, provider_account_id, username, account_type,
          connection_status, scopes, connected_at)
         values ($1, $2, $3, 'synthetic', 'business', $4,
                 array['instagram_business_basic'], now())",
    )
    .bind(account_id)
    .bind(user_ref)
    .bind(format!("provider-{account_id}"))
    .bind(status)
    .execute(test.database.pool())
    .await
    .expect("account inserts");
    sqlx::query(
        "insert into instagram_archive.credentials
         (credential_id, account_id, access_token_envelope, refresh_token_envelope,
          key_version, granted_permissions, expires_at)
         values ($1, $2, $3, $4, 7, array['instagram_business_basic'], now())",
    )
    .bind(Uuid::now_v7())
    .bind(account_id)
    .bind(SENTINEL.as_bytes().to_vec())
    .bind(format!("refresh-{SENTINEL}").into_bytes())
    .execute(test.database.pool())
    .await
    .expect("credential inserts");
    sqlx::query(
        "insert into instagram_archive.oauth_flows
         (flow_id, user_ref, account_id, state_hash, redirect_uri_hash,
          pkce_verifier_envelope, key_version, expires_at)
         values ($1, $2, $3, $4, $5, $6, 7, now() + interval '10 minutes')",
    )
    .bind(Uuid::now_v7())
    .bind(user_ref)
    .bind(account_id)
    .bind(vec![1_u8; 32])
    .bind(vec![2_u8; 32])
    .bind(format!("pkce-{SENTINEL}").into_bytes())
    .execute(test.database.pool())
    .await
    .expect("flow inserts");
    let generation = Uuid::now_v7();
    for capability in AccountCapability::ALL {
        sqlx::query(
            "insert into instagram_archive.account_capabilities
             (account_id, generation_id, capability, capability_state, reason, observed_at)
             values ($1, $2, $3, 'available', 'granted', now())",
        )
        .bind(account_id)
        .bind(generation)
        .bind(capability.wire_value())
        .execute(test.database.pool())
        .await
        .expect("capability inserts");
    }
    (account_id, user_ref)
}

async fn assert_fully_scrubbed(test: &TestDatabase, account_id: Uuid, user_ref: Uuid) {
    let credentials: i64 = sqlx::query_scalar(
        "select count(*) from instagram_archive.credentials where account_id = $1",
    )
    .bind(account_id)
    .fetch_one(test.database.pool())
    .await
    .expect("credential count answers");
    assert_eq!(credentials, 0);
    let flows: i64 = sqlx::query_scalar(
        "select count(*) from instagram_archive.oauth_flows where user_ref = $1",
    )
    .bind(user_ref)
    .fetch_one(test.database.pool())
    .await
    .expect("flow count answers");
    assert_eq!(flows, 0);
    let account = sqlx::query(
        "select connection_status, scopes from instagram_archive.accounts where account_id = $1",
    )
    .bind(account_id)
    .fetch_one(test.database.pool())
    .await
    .expect("account loads");
    assert_eq!(account.get::<String, _>("connection_status"), "revoked");
    assert!(account.get::<Vec<String>, _>("scopes").is_empty());
    let capabilities: Vec<(String, String)> = sqlx::query_as(
        "select capability_state, reason from instagram_archive.account_capabilities
         where account_id = $1",
    )
    .bind(account_id)
    .fetch_all(test.database.pool())
    .await
    .expect("capabilities load");
    assert_eq!(capabilities.len(), AccountCapability::ALL.len());
    assert!(
        capabilities
            .iter()
            .all(|(state, reason)| { state == "unavailable" && reason == "revoked" })
    );
}

#[tokio::test]
async fn revoke_scrubs_every_credential_and_live_owner_flow_marks_account_and_disables_capabilities()
 {
    let test = TestDatabase::create().await.expect("fresh database");
    let (account_id, user_ref) = seeded_account(&test, "connected").await;
    test.database
        .scrub_revoked_account(
            account_id,
            user_ref,
            ProviderRevokeOutcome::Succeeded,
            OffsetDateTime::UNIX_EPOCH,
        )
        .await
        .expect("local revoke succeeds");
    assert_fully_scrubbed(&test, account_id, user_ref).await;
    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn provider_revoke_failure_still_scrubs_locally() {
    let test = TestDatabase::create().await.expect("fresh database");
    let (account_id, user_ref) = seeded_account(&test, "connected").await;
    test.database
        .scrub_revoked_account(
            account_id,
            user_ref,
            ProviderRevokeOutcome::Failed,
            OffsetDateTime::UNIX_EPOCH,
        )
        .await
        .expect("local revoke succeeds");
    assert_fully_scrubbed(&test, account_id, user_ref).await;
    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn unsupported_provider_revoke_still_scrubs_locally() {
    let test = TestDatabase::create().await.expect("fresh database");
    let (account_id, user_ref) = seeded_account(&test, "connected").await;
    test.database
        .scrub_revoked_account(
            account_id,
            user_ref,
            ProviderRevokeOutcome::Unsupported,
            OffsetDateTime::UNIX_EPOCH,
        )
        .await
        .expect("local revoke succeeds");
    assert_fully_scrubbed(&test, account_id, user_ref).await;
    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn revoke_audit_and_usage_are_redacted() {
    let test = TestDatabase::create().await.expect("fresh database");
    let (account_id, user_ref) = seeded_account(&test, "connected").await;
    test.database
        .scrub_revoked_account(
            account_id,
            user_ref,
            ProviderRevokeOutcome::Failed,
            OffsetDateTime::UNIX_EPOCH,
        )
        .await
        .expect("local revoke succeeds");
    let audit: Vec<String> = sqlx::query_scalar(
        "select detail::text from instagram_archive.account_credential_audit
         where account_id = $1 and change_kind = 'revoked'",
    )
    .bind(account_id)
    .fetch_all(test.database.pool())
    .await
    .expect("audit loads");
    assert_eq!(audit.len(), 1);
    assert!(!audit.join("").contains(SENTINEL));
    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn startup_scrubs_account_stranded_in_revoking() {
    let test = TestDatabase::create().await.expect("fresh database");
    let (account_id, user_ref) = seeded_account(&test, "revoking").await;
    let scrubbed = test
        .database
        .scrub_stranded_revocations(OffsetDateTime::UNIX_EPOCH)
        .await
        .expect("startup sweep succeeds");
    assert_eq!(scrubbed, 1);
    assert_fully_scrubbed(&test, account_id, user_ref).await;
    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn refresh_replaces_encrypted_material_expiry_permissions_and_capabilities_together() {
    let test = TestDatabase::create().await.expect("fresh database");
    let (account_id, user_ref, old_envelope) = refreshable_account(&test).await;
    let provider = FakeInstagramProvider::new(successful_refresh_steps(
        account_id,
        PermissionStatus::Granted,
    ));
    let capabilities = test
        .database
        .refresh_official_account(
            &keyring(),
            &provider,
            account_id,
            user_ref,
            4,
            1,
            true,
            OffsetDateTime::UNIX_EPOCH,
        )
        .await
        .expect("refresh succeeds");
    let envelope: Vec<u8> = sqlx::query_scalar(
        "select access_token_envelope from instagram_archive.credentials where account_id = $1",
    )
    .bind(account_id)
    .fetch_one(test.database.pool())
    .await
    .expect("credential loads");
    assert_ne!(envelope, old_envelope);
    assert_eq!(
        capabilities
            .iter()
            .find(|row| row.reconciled.capability == AccountCapability::OwnMediaRead)
            .expect("own-media row")
            .reconciled
            .state,
        CapabilityState::Available
    );
    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn refresh_permission_downgrade_removes_stale_capability() {
    let test = TestDatabase::create().await.expect("fresh database");
    let (account_id, user_ref, _) = refreshable_account(&test).await;
    let provider = FakeInstagramProvider::new(successful_refresh_steps(
        account_id,
        PermissionStatus::Declined,
    ));
    let capabilities = test
        .database
        .refresh_official_account(
            &keyring(),
            &provider,
            account_id,
            user_ref,
            4,
            1,
            true,
            OffsetDateTime::UNIX_EPOCH,
        )
        .await
        .expect("refresh succeeds with downgrade evidence");
    assert_eq!(
        capabilities
            .iter()
            .find(|row| row.reconciled.capability == AccountCapability::OwnMediaRead)
            .expect("own-media row")
            .reconciled
            .state,
        CapabilityState::Unavailable
    );
    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn refresh_auth_failure_marks_reauthorization_and_disables_all_capabilities() {
    let test = TestDatabase::create().await.expect("fresh database");
    let (account_id, user_ref, _) = refreshable_account(&test).await;
    let provider = FakeInstagramProvider::new([FakeProviderStep::Refresh(Err(ProviderError {
        class: ProviderFailureClass::Authentication,
        http_status: Some(401),
    }))]);
    let result = test
        .database
        .refresh_official_account(
            &keyring(),
            &provider,
            account_id,
            user_ref,
            4,
            1,
            true,
            OffsetDateTime::UNIX_EPOCH,
        )
        .await;
    assert!(result.is_err());
    let status: String = sqlx::query_scalar(
        "select connection_status from instagram_archive.accounts where account_id = $1",
    )
    .bind(account_id)
    .fetch_one(test.database.pool())
    .await
    .expect("status loads");
    assert_eq!(status, "reauthorization_required");
    let available: i64 = sqlx::query_scalar(
        "select count(*) from instagram_archive.account_capabilities
         where account_id = $1 and capability_state = 'available'",
    )
    .bind(account_id)
    .fetch_one(test.database.pool())
    .await
    .expect("capability count answers");
    assert_eq!(available, 0);
    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn refresh_transient_failure_preserves_prior_connection() {
    let test = TestDatabase::create().await.expect("fresh database");
    let (account_id, user_ref, old_envelope) = refreshable_account(&test).await;
    let provider = FakeInstagramProvider::new([FakeProviderStep::Refresh(Err(ProviderError {
        class: ProviderFailureClass::Network,
        http_status: None,
    }))]);
    let result = test
        .database
        .refresh_official_account(
            &keyring(),
            &provider,
            account_id,
            user_ref,
            4,
            1,
            true,
            OffsetDateTime::UNIX_EPOCH,
        )
        .await;
    assert!(result.is_err());
    let row: (String, Vec<u8>) = sqlx::query_as(
        "select a.connection_status, c.access_token_envelope
         from instagram_archive.accounts a
         join instagram_archive.credentials c using (account_id)
         where a.account_id = $1",
    )
    .bind(account_id)
    .fetch_one(test.database.pool())
    .await
    .expect("prior connection loads");
    assert_eq!(row.0, "connected");
    assert_eq!(row.1, old_envelope);
    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn refresh_without_provider_supported_strategy_is_typed() {
    let test = TestDatabase::create().await.expect("fresh database");
    let (account_id, user_ref, _) = refreshable_account(&test).await;
    let provider = FakeInstagramProvider::new([]);
    let error = test
        .database
        .refresh_official_account(
            &keyring(),
            &provider,
            account_id,
            user_ref,
            4,
            1,
            false,
            OffsetDateTime::UNIX_EPOCH,
        )
        .await
        .expect_err("unsupported strategy is typed");
    assert!(matches!(
        error,
        ratatoskr_instagram_archive::account::AccountLifecycleError::UnsupportedRefresh
    ));
    assert_eq!(provider.remaining(), 0);
    test.cleanup().await.expect("cleanup drops");
}
