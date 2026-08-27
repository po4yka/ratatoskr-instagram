//! Provider attempts reserve durable budget before transport.

use std::sync::atomic::{AtomicUsize, Ordering};

use ratatoskr_instagram_archive::provider_budget::{
    MetaUsage, ProviderBudget, RequestClass, UsageOutcome,
};
use ratatoskr_instagram_archive::test_support::TestDatabase;
use sqlx::Row as _;
use time::OffsetDateTime;
use uuid::Uuid;

#[tokio::test]
async fn admission_reserves_started_usage_before_transport() {
    let test = TestDatabase::create().await.expect("fresh database");
    let operation_id = Uuid::now_v7();
    let mut budget = ProviderBudget::new(test.database.clone(), operation_id, None, 3);
    let reservation = budget
        .reserve(RequestClass::AccountDiscovery, OffsetDateTime::UNIX_EPOCH)
        .await
        .expect("first attempt is admitted");
    let state: String = sqlx::query_scalar(
        "select state from instagram_archive.provider_api_usage where usage_id = $1",
    )
    .bind(reservation.usage_id)
    .fetch_one(test.database.pool())
    .await
    .expect("reservation is committed before transport");
    assert_eq!(state, "started");
    assert_eq!(reservation.attempt_ordinal, 1);
    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn completed_attempt_records_redacted_outcome_and_bounded_usage_percentages() {
    let test = TestDatabase::create().await.expect("fresh database");
    let mut budget = ProviderBudget::new(test.database.clone(), Uuid::now_v7(), None, 1);
    let reservation = budget
        .reserve(
            RequestClass::PermissionDiscovery,
            OffsetDateTime::UNIX_EPOCH,
        )
        .await
        .expect("attempt admitted");
    budget
        .complete(
            reservation,
            UsageOutcome::Succeeded,
            Some(200),
            MetaUsage {
                call_count_percent: Some(17),
                cpu_time_percent: Some(4),
                total_time_percent: Some(9),
            },
            OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(1),
        )
        .await
        .expect("terminal update succeeds");
    let row = sqlx::query(
        "select state, outcome, http_status, call_count_percent, cpu_time_percent,
                total_time_percent
         from instagram_archive.provider_api_usage where usage_id = $1",
    )
    .bind(reservation.usage_id)
    .fetch_one(test.database.pool())
    .await
    .expect("usage row exists");
    assert_eq!(row.get::<String, _>("state"), "completed");
    assert_eq!(row.get::<String, _>("outcome"), "succeeded");
    assert_eq!(row.get::<i16, _>("http_status"), 200);
    assert_eq!(row.get::<i16, _>("call_count_percent"), 17);
    assert_eq!(row.get::<i16, _>("cpu_time_percent"), 4);
    assert_eq!(row.get::<i16, _>("total_time_percent"), 9);
    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn failed_retry_consumes_a_second_ordinal() {
    let test = TestDatabase::create().await.expect("fresh database");
    let operation_id = Uuid::now_v7();
    let mut budget = ProviderBudget::new(test.database.clone(), operation_id, None, 2);
    for outcome in [UsageOutcome::Network, UsageOutcome::Succeeded] {
        let reservation = budget
            .reserve(RequestClass::AccountDiscovery, OffsetDateTime::UNIX_EPOCH)
            .await
            .expect("attempt is admitted");
        budget
            .complete(
                reservation,
                outcome,
                None,
                MetaUsage::default(),
                OffsetDateTime::UNIX_EPOCH,
            )
            .await
            .expect("terminal update succeeds");
    }
    let ordinals: Vec<i32> = sqlx::query_scalar(
        "select attempt_ordinal from instagram_archive.provider_api_usage
         where operation_id = $1 order by attempt_ordinal",
    )
    .bind(operation_id)
    .fetch_all(test.database.pool())
    .await
    .expect("usage rows load");
    assert_eq!(ordinals, [1, 2]);
    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn exhausted_budget_does_not_invoke_transport() {
    let test = TestDatabase::create().await.expect("fresh database");
    let calls = AtomicUsize::new(0);
    let mut budget = ProviderBudget::new(test.database.clone(), Uuid::now_v7(), None, 1);
    budget
        .reserve(RequestClass::CodeExchange, OffsetDateTime::UNIX_EPOCH)
        .await
        .expect("first attempt admitted");
    let denied = budget
        .reserve(RequestClass::CodeExchange, OffsetDateTime::UNIX_EPOCH)
        .await;
    if denied.is_ok() {
        calls.fetch_add(1, Ordering::Relaxed);
    }
    assert!(denied.is_err());
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn crash_left_started_attempt_remains_consumed() {
    let test = TestDatabase::create().await.expect("fresh database");
    let operation_id = Uuid::now_v7();
    let mut budget = ProviderBudget::new(test.database.clone(), operation_id, None, 1);
    budget
        .reserve(RequestClass::TokenRefresh, OffsetDateTime::UNIX_EPOCH)
        .await
        .expect("attempt admitted");
    drop(budget);
    let state: String = sqlx::query_scalar(
        "select state from instagram_archive.provider_api_usage where operation_id = $1",
    )
    .bind(operation_id)
    .fetch_one(test.database.pool())
    .await
    .expect("reservation remains after owner drop");
    assert_eq!(state, "started");
    test.cleanup().await.expect("cleanup drops");
}
