//! Atomic repair contract for rows falsely credited by the retired logging transport.

use ratatoskr_instagram_archive::outbox_repair::repair_logging_outbox;
use ratatoskr_instagram_archive::test_support::TestDatabase;
use time::OffsetDateTime;
use uuid::Uuid;

#[expect(clippy::expect_used, reason = "repair fixture setup must succeed")]
async fn seed_published(test: &TestDatabase, event_type: &str) -> Uuid {
    let event_id = Uuid::now_v7();
    let aggregate_id = Uuid::now_v7();
    let owner = Uuid::now_v7();
    let payload = serde_json::json!({
        "event_id": event_id,
        "event_type": event_type,
        "occurred_at": "1970-01-01T00:00:00Z",
        "producer": "ratatoskr-instagram",
        "aggregate_id": format!("social_source:{aggregate_id}"),
        "correlation_id": format!("user:{owner}"),
        "tenant_id": format!("user:{owner}"),
        "schema_version": 1,
        "payload": {}
    });
    sqlx::query(
        "insert into instagram_archive.outbox_events \
         (event_id, event_type, aggregate_type, aggregate_id, payload, occurred_at, \
          published_at, attempt_count, next_attempt_at) \
         values ($1, $2, 'capture', $3, $4, now(), now(), 7, null)",
    )
    .bind(event_id)
    .bind(event_type)
    .bind(aggregate_id)
    .bind(payload)
    .execute(test.database.pool())
    .await
    .expect("the published fixture row is seeded");
    event_id
}

#[tokio::test]
async fn logging_era_social_facts_are_requeued_atomically() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let mut before = Vec::new();
    for event_type in [
        "social.source.captured.v1",
        "social.source.updated.v1",
        "social.source.removed.v1",
    ] {
        let event_id = seed_published(&test, event_type).await;
        let (payload, attempt_count): (serde_json::Value, i32) = sqlx::query_as(
            "select payload, attempt_count from instagram_archive.outbox_events where event_id = $1",
        )
        .bind(event_id)
        .fetch_one(test.database.pool())
        .await
        .expect("the fixture row is readable");
        before.push((event_id, payload, attempt_count));
    }

    let repaired = repair_logging_outbox(test.database.pool())
        .await
        .expect("the repair transaction succeeds");

    assert_eq!(repaired, 3, "all logging-era SocialSource facts requeue");
    for (event_id, payload, attempt_count) in before {
        let (stored_id, stored_payload, stored_attempts, published_at, next_attempt_at): (
            Uuid,
            serde_json::Value,
            i32,
            Option<OffsetDateTime>,
            Option<OffsetDateTime>,
        ) = sqlx::query_as(
            "select event_id, payload, attempt_count, published_at, next_attempt_at \
             from instagram_archive.outbox_events where event_id = $1",
        )
        .bind(event_id)
        .fetch_one(test.database.pool())
        .await
        .expect("the repaired row remains readable");
        assert_eq!(stored_id, event_id, "event identity is unchanged");
        assert_eq!(stored_payload, payload, "envelope bytes are unchanged");
        assert_eq!(
            stored_attempts, attempt_count,
            "attempt history is unchanged"
        );
        assert!(
            published_at.is_none(),
            "false publication credit is cleared"
        );
        assert!(
            next_attempt_at.is_some(),
            "the repaired fact is immediately due"
        );
    }
    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn second_pre_cutover_repair_changes_zero_rows() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    seed_published(&test, "social.source.captured.v1").await;

    assert_eq!(
        repair_logging_outbox(test.database.pool())
            .await
            .expect("the first repair succeeds"),
        1
    );
    assert_eq!(
        repair_logging_outbox(test.database.pool())
            .await
            .expect("the repeated repair succeeds"),
        0,
        "a repeated stopped-service repair is idempotent"
    );
    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn foreign_event_types_are_untouched() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    seed_published(&test, "social.source.captured.v1").await;
    let foreign_id = seed_published(&test, "platform.audit.recorded.v1").await;

    let repaired = repair_logging_outbox(test.database.pool())
        .await
        .expect("the repair succeeds");

    assert_eq!(repaired, 1, "only the owned SocialSource fact is repaired");
    let (published_at, next_attempt_at): (Option<OffsetDateTime>, Option<OffsetDateTime>) =
        sqlx::query_as(
            "select published_at, next_attempt_at \
         from instagram_archive.outbox_events where event_id = $1",
        )
        .bind(foreign_id)
        .fetch_one(test.database.pool())
        .await
        .expect("the foreign row remains readable");
    assert!(
        published_at.is_some(),
        "foreign publication state is untouched"
    );
    assert!(
        next_attempt_at.is_none(),
        "foreign retry state is untouched"
    );
    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn failed_transaction_changes_no_subset() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let captured_id = seed_published(&test, "social.source.captured.v1").await;
    let updated_id = seed_published(&test, "social.source.updated.v1").await;
    sqlx::query(
        "create function instagram_archive.refuse_updated_repair() returns trigger \
         language plpgsql as $$ begin raise exception 'repair fixture refusal'; end $$",
    )
    .execute(test.database.pool())
    .await
    .expect("the disposable failure function is installed");
    sqlx::query(
        "create trigger refuse_updated_repair \
         before update of published_at on instagram_archive.outbox_events \
         for each row when (old.event_type = 'social.source.updated.v1') \
         execute function instagram_archive.refuse_updated_repair()",
    )
    .execute(test.database.pool())
    .await
    .expect("the disposable failure trigger is installed");

    repair_logging_outbox(test.database.pool())
        .await
        .expect_err("the trigger must abort the repair transaction");

    for event_id in [captured_id, updated_id] {
        let (published_at, next_attempt_at): (Option<OffsetDateTime>, Option<OffsetDateTime>) =
            sqlx::query_as(
                "select published_at, next_attempt_at \
             from instagram_archive.outbox_events where event_id = $1",
            )
            .bind(event_id)
            .fetch_one(test.database.pool())
            .await
            .expect("the original row remains readable");
        assert!(published_at.is_some(), "no subset loses publication credit");
        assert!(next_attempt_at.is_none(), "no subset gains a retry time");
    }
    test.cleanup().await.expect("cleanup");
}
