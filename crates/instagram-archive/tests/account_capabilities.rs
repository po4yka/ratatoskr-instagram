//! Capability reconciliation from provider-observed facts only.

#![expect(
    clippy::expect_used,
    reason = "integration-test setup failures are assertions"
)]

use std::collections::BTreeMap;

use ratatoskr_instagram_archive::capability::{AcquisitionMode, SupportStatus};
use ratatoskr_instagram_archive::capability_reconciliation::{
    AccountCapability, AccountObservation, AccountType, CapabilityReason, CapabilityState,
    PermissionStatus, ReconciledCapability, reconcile,
};
use ratatoskr_instagram_archive::test_support::TestDatabase;
use time::OffsetDateTime;
use uuid::Uuid;

fn observation(account_type: AccountType, basic: PermissionStatus) -> AccountObservation {
    AccountObservation {
        provider_account_id: "17841400000000000".to_owned(),
        account_type,
        permissions: BTreeMap::from([("instagram_business_basic".to_owned(), basic)]),
        external_write_consent: false,
        observed_at: OffsetDateTime::UNIX_EPOCH,
        raw_record_id: Uuid::from_u128(0x018f_1a2b_3c4d_7e6f_8a9b_0c1d_2e3f_4a5b),
    }
}

fn row(rows: &[ReconciledCapability], capability: AccountCapability) -> ReconciledCapability {
    rows.iter()
        .copied()
        .find(|row| row.capability == capability)
        .expect("every closed capability has one row")
}

fn assert_read_only_professional(account_type: AccountType) {
    let rows = reconcile(&observation(account_type, PermissionStatus::Granted));
    assert_eq!(rows.len(), AccountCapability::ALL.len());
    for capability in [
        AccountCapability::AccountIdentityRead,
        AccountCapability::OwnMediaRead,
    ] {
        let result = row(&rows, capability);
        assert_eq!(result.state, CapabilityState::Available);
        assert_eq!(result.reason, CapabilityReason::Granted);
    }
    for capability in [
        AccountCapability::ContentPublish,
        AccountCapability::CommentManagement,
        AccountCapability::MessageManagement,
    ] {
        assert_eq!(row(&rows, capability).state, CapabilityState::Unavailable);
    }
}

#[test]
fn business_basic_grant_exposes_only_read_capabilities() {
    assert_read_only_professional(AccountType::Business);
}

#[test]
fn creator_basic_grant_exposes_only_read_capabilities() {
    assert_read_only_professional(AccountType::Creator);
}

#[test]
fn declined_expired_absent_and_unknown_permissions_are_unavailable() {
    for (status, reason) in [
        (
            PermissionStatus::Declined,
            CapabilityReason::PermissionDeclined,
        ),
        (
            PermissionStatus::Expired,
            CapabilityReason::PermissionExpired,
        ),
        (PermissionStatus::Absent, CapabilityReason::PermissionAbsent),
        (
            PermissionStatus::Unknown,
            CapabilityReason::PermissionUnknown,
        ),
    ] {
        let rows = reconcile(&observation(AccountType::Business, status));
        for capability in [
            AccountCapability::AccountIdentityRead,
            AccountCapability::OwnMediaRead,
        ] {
            let result = row(&rows, capability);
            assert_eq!(result.state, CapabilityState::Unavailable);
            assert_eq!(result.reason, reason);
        }
    }
}

#[test]
fn personal_and_unknown_accounts_grant_nothing() {
    for account_type in [AccountType::Personal, AccountType::Unknown] {
        let rows = reconcile(&observation(account_type, PermissionStatus::Granted));
        for result in rows {
            if result.capability != AccountCapability::NativeSavedRead {
                assert_eq!(result.state, CapabilityState::Unavailable);
                assert_eq!(result.reason, CapabilityReason::AccountTypeUnsupported);
            }
        }
    }
}

#[test]
fn native_saved_is_always_not_supported() {
    for account_type in [
        AccountType::Business,
        AccountType::Creator,
        AccountType::Personal,
        AccountType::Unknown,
    ] {
        let rows = reconcile(&observation(account_type, PermissionStatus::Granted));
        let result = row(&rows, AccountCapability::NativeSavedRead);
        assert_eq!(result.state, CapabilityState::NotSupported);
        assert_eq!(result.reason, CapabilityReason::ProviderNotSupported);
    }
}

#[test]
fn own_account_sync_lane_remains_planned() {
    assert_eq!(
        AcquisitionMode::OwnAccountSync.capability().status,
        SupportStatus::Planned
    );
}

async fn stored_account(test: &TestDatabase, account_type: &str) -> (Uuid, Uuid) {
    let account_id = Uuid::now_v7();
    let raw_record_id = Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.accounts
         (account_id, user_ref, provider_account_id, username, account_type,
          connection_status, scopes, connected_at)
         values ($1, $2, $3, 'synthetic', $4, 'connected', '{}', now())",
    )
    .bind(account_id)
    .bind(Uuid::now_v7())
    .bind(format!("provider-{account_id}"))
    .bind(account_type)
    .execute(test.database.pool())
    .await
    .expect("account inserts");
    sqlx::query(
        "insert into instagram_archive.raw_records
         (raw_record_id, record_kind, blob_ref, content_hash, byte_size, body, observed_at)
         values ($1, 'api_response', $2, $3, 2, $4, now())",
    )
    .bind(raw_record_id)
    .bind(format!("{:064x}", account_id.as_u128()))
    .bind(vec![7_u8; 32])
    .bind(b"{}".to_vec())
    .execute(test.database.pool())
    .await
    .expect("raw evidence inserts");
    (account_id, raw_record_id)
}

#[tokio::test]
async fn reconciliation_replaces_the_whole_prior_generation() {
    let test = TestDatabase::create().await.expect("fresh database");
    let (account_id, raw_record_id) = stored_account(&test, "business").await;
    let mut granted = observation(AccountType::Business, PermissionStatus::Granted);
    granted.raw_record_id = raw_record_id;
    test.database
        .reconcile_account_capabilities(account_id, &granted)
        .await
        .expect("first reconciliation succeeds");

    let later_raw = Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.raw_records
         (raw_record_id, record_kind, blob_ref, content_hash, byte_size, body, observed_at)
         values ($1, 'api_response', $2, $3, 2, $4, now())",
    )
    .bind(later_raw)
    .bind(format!("{:064x}", later_raw.as_u128()))
    .bind(vec![8_u8; 32])
    .bind(b"{}".to_vec())
    .execute(test.database.pool())
    .await
    .expect("later raw evidence inserts");
    let mut declined = observation(AccountType::Business, PermissionStatus::Declined);
    declined.raw_record_id = later_raw;
    let latest = test
        .database
        .reconcile_account_capabilities(account_id, &declined)
        .await
        .expect("second reconciliation succeeds");

    let rows = test
        .database
        .load_account_capabilities(account_id)
        .await
        .expect("projection loads");
    assert_eq!(rows.len(), AccountCapability::ALL.len());
    assert!(rows.iter().all(|row| row.generation_id == latest));
    let identity = rows
        .iter()
        .find(|row| row.reconciled.capability == AccountCapability::AccountIdentityRead)
        .expect("identity row exists");
    assert_eq!(identity.reconciled.state, CapabilityState::Unavailable);
    assert_eq!(
        identity.reconciled.reason,
        CapabilityReason::PermissionDeclined
    );
    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn two_accounts_never_share_observations() {
    let test = TestDatabase::create().await.expect("fresh database");
    let (business_id, business_raw) = stored_account(&test, "business").await;
    let (personal_id, personal_raw) = stored_account(&test, "personal").await;
    let mut business = observation(AccountType::Business, PermissionStatus::Granted);
    business.raw_record_id = business_raw;
    let mut personal = observation(AccountType::Personal, PermissionStatus::Granted);
    personal.raw_record_id = personal_raw;
    test.database
        .reconcile_account_capabilities(business_id, &business)
        .await
        .expect("business reconciles");
    test.database
        .reconcile_account_capabilities(personal_id, &personal)
        .await
        .expect("personal reconciles");
    let business_rows = test
        .database
        .load_account_capabilities(business_id)
        .await
        .expect("business projection loads");
    let personal_rows = test
        .database
        .load_account_capabilities(personal_id)
        .await
        .expect("personal projection loads");
    assert_eq!(business_rows.len(), AccountCapability::ALL.len());
    assert_eq!(personal_rows.len(), AccountCapability::ALL.len());
    assert_eq!(
        business_rows
            .iter()
            .find(|row| row.reconciled.capability == AccountCapability::OwnMediaRead)
            .expect("business own-media row")
            .reconciled
            .state,
        CapabilityState::Available
    );
    assert_eq!(
        personal_rows
            .iter()
            .find(|row| row.reconciled.capability == AccountCapability::OwnMediaRead)
            .expect("personal own-media row")
            .reconciled
            .state,
        CapabilityState::Unavailable
    );
    test.cleanup().await.expect("cleanup drops");
}
