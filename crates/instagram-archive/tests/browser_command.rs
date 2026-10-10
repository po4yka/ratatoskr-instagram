//! Contract coverage for Platform's Instagram browser-capture command.

use ratatoskr_instagram_archive::{
    BrowserCaptureIngested, CommandCaptureError, decode_browser_capture_command,
    test_support::TestDatabase,
};
use serde_json::json;

const COMMAND_ID: &str = "01991000-0000-7000-8000-000000000001";
const OPERATION_ID: &str = "01991000-0000-7000-8000-000000000002";
const USER_ID: &str = "01991000-0000-7000-8000-000000000003";
const SECOND_COMMAND_ID: &str = "01991000-0000-7000-8000-000000000004";
const SECOND_OPERATION_ID: &str = "01991000-0000-7000-8000-000000000005";

fn instagram_command(provider: &str, permalink: &str) -> Vec<u8> {
    command_with_ids(COMMAND_ID, OPERATION_ID, provider, permalink)
}

#[expect(
    clippy::expect_used,
    reason = "the fixed JSON fixture is authored in this test and failure is a test setup error"
)]
fn command_with_ids(
    command_id: &str,
    operation_id: &str,
    provider: &str,
    permalink: &str,
) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "command_id": command_id,
        "command_type": "social.capture.requested.v1",
        "issued_at": "2026-08-27T12:00:00Z",
        "producer": "ratatoskr-platform",
        "aggregate_id": format!("operation:{operation_id}"),
        "correlation_id": format!("operation:{operation_id}"),
        "tenant_id": format!("user:{USER_ID}"),
        "schema_version": 1,
        "payload": {
            "operation_id": operation_id,
            "idempotency_key": {
                "algorithm": "sha256",
                "hex": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            },
            "original_permalink": permalink,
            "captured_at": "2026-08-27T11:59:00Z",
            "provider": provider,
            "acquisition": "browser_extension",
            "saved_authority": "explicit_user_capture"
        }
    }))
    .expect("the fixed synthetic command serializes")
}

#[test]
fn instagram_command_retains_closed_browser_provenance() {
    let command = decode_browser_capture_command(
        "cmd.instagram.capture.requested.v1",
        &instagram_command("instagram", "https://www.instagram.com/p/Capture123/"),
    )
    .expect("the Instagram command is accepted");

    assert_eq!(command.user_ref.to_string(), USER_ID);
    assert_eq!(command.operation_id.to_string(), OPERATION_ID);
    assert_eq!(
        command.original_permalink,
        "https://www.instagram.com/p/Capture123/"
    );
    assert_eq!(command.client_source.wire_value(), "browser_extension");
}

#[test]
fn instagram_consumer_rejects_a_command_for_another_provider() {
    let error = decode_browser_capture_command(
        "cmd.instagram.capture.requested.v1",
        &instagram_command("threads", "https://www.instagram.com/p/Capture123/"),
    )
    .expect_err("a Threads command must not reach Instagram");

    assert!(error.to_string().contains("Instagram"));
}

#[tokio::test]
async fn failed_capture_rolls_back_its_inbox_claim_for_redelivery() {
    let test = TestDatabase::create()
        .await
        .expect("a disposable Instagram archive database");
    let valid = instagram_command("instagram", "https://www.instagram.com/p/Capture123/");
    // A storage fault, not a bad command: the capture table is unreachable.
    sqlx::query("alter table instagram_archive.captures rename to captures_unreachable")
        .execute(test.database.pool())
        .await
        .expect("the fault is injected");
    let error = test
        .database
        .ingest_browser_capture_command("cmd.instagram.capture.requested.v1", &valid)
        .await
        .expect_err("a storage fault must refuse capture persistence");
    assert!(
        matches!(error, CommandCaptureError::Capture(_)),
        "the source must fail during capture persistence: {error:?}"
    );

    let inbox_count: i64 = sqlx::query_scalar(
        "select count(*) from instagram_archive.inbox_events \
         where consumer_name = 'ratatoskr-instagram-browser-capture' and event_id = $1",
    )
    .bind(uuid::Uuid::parse_str(COMMAND_ID).expect("fixed command UUID"))
    .fetch_one(test.database.pool())
    .await
    .expect("the inbox count query answers");
    assert_eq!(
        inbox_count, 0,
        "a failed capture must not consume redelivery"
    );
    assert!(
        report_rows(&test).await.is_empty(),
        "a failed capture queues no report"
    );

    sqlx::query("alter table instagram_archive.captures_unreachable rename to captures")
        .execute(test.database.pool())
        .await
        .expect("the fault is cleared");
    let outcome = test
        .database
        .ingest_browser_capture_command("cmd.instagram.capture.requested.v1", &valid)
        .await
        .expect("the same delivery can be retried after rollback");
    assert!(matches!(outcome, BrowserCaptureIngested::Preserved(_)));
    test.cleanup()
        .await
        .expect("the disposable database is removed");
}

#[tokio::test]
async fn an_unmappable_permalink_is_reported_inaccessible_instead_of_dropped() {
    let test = TestDatabase::create()
        .await
        .expect("a disposable Instagram archive database");
    let invalid = instagram_command("instagram", "https://www.instagram.com/example");
    let outcome = test
        .database
        .ingest_browser_capture_command("cmd.instagram.capture.requested.v1", &invalid)
        .await
        .expect("an attributable command is answered, not failed");
    assert!(
        matches!(outcome, BrowserCaptureIngested::Rejected),
        "the permalink cannot be mapped: {outcome:?}"
    );

    let reports = report_rows(&test).await;
    assert_eq!(reports.len(), 1, "exactly one terminal report");
    let envelope = &reports[0];
    assert_eq!(envelope["tenant_id"], format!("user:{USER_ID}"));
    assert_eq!(
        envelope["correlation_id"],
        format!("operation:{OPERATION_ID}")
    );
    assert_eq!(envelope["causation_id"], format!("command:{COMMAND_ID}"));
    assert_eq!(
        envelope["aggregate_id"],
        format!("operation:{OPERATION_ID}")
    );
    assert_eq!(envelope["payload"]["status"], "failed");
    assert_eq!(envelope["payload"]["stage"], "capture_unavailable");
    assert_eq!(
        envelope["payload"]["error"]["code"],
        "social.source.unavailable"
    );
    assert_eq!(envelope["payload"]["error"]["retryable"], false);
    assert!(operation_rows(&test).await.is_empty(), "no capture exists");

    let replay = test
        .database
        .ingest_browser_capture_command("cmd.instagram.capture.requested.v1", &invalid)
        .await
        .expect("the redelivery is recognized");
    assert!(matches!(replay, BrowserCaptureIngested::Duplicate));
    assert_eq!(
        report_rows(&test).await.len(),
        1,
        "redelivery adds no report"
    );
    test.cleanup()
        .await
        .expect("the disposable database is removed");
}

#[tokio::test]
async fn duplicate_delivery_reuses_one_capture_and_one_inbox_claim() {
    let test = TestDatabase::create()
        .await
        .expect("a disposable Instagram archive database");
    let command = instagram_command("instagram", "https://www.instagram.com/p/Capture123/");
    let first = test
        .database
        .ingest_browser_capture_command("cmd.instagram.capture.requested.v1", &command)
        .await
        .expect("the first delivery persists the capture");
    assert!(matches!(first, BrowserCaptureIngested::Preserved(_)));
    let replay = test
        .database
        .ingest_browser_capture_command("cmd.instagram.capture.requested.v1", &command)
        .await
        .expect("the redelivery is recognized by the inbox");
    assert!(matches!(replay, BrowserCaptureIngested::Duplicate));

    let captures: i64 = sqlx::query_scalar("select count(*) from instagram_archive.captures")
        .fetch_one(test.database.pool())
        .await
        .expect("the capture count query answers");
    assert_eq!(captures, 1);
    test.cleanup()
        .await
        .expect("the disposable database is removed");
}

/// One stored operation row: `(operation_id, command_id, capture_id, user_ref, reported)`.
type OperationRow = (uuid::Uuid, uuid::Uuid, uuid::Uuid, uuid::Uuid, bool);

#[expect(
    clippy::expect_used,
    reason = "a database that cannot answer is the failure under test"
)]
async fn operation_rows(test: &TestDatabase) -> Vec<OperationRow> {
    sqlx::query_as(
        "select operation_id, command_id, capture_id, user_ref, reported_at is not null \
         from instagram_archive.capture_operations order by created_at, operation_id",
    )
    .fetch_all(test.database.pool())
    .await
    .expect("the operation rows are readable")
}

#[expect(
    clippy::expect_used,
    reason = "a database that cannot answer is the failure under test"
)]
async fn report_rows(test: &TestDatabase) -> Vec<serde_json::Value> {
    sqlx::query_scalar(
        "select payload from instagram_archive.outbox_events \
         where event_type = 'platform.operation.reported.v1' order by occurred_at, event_id",
    )
    .fetch_all(test.database.pool())
    .await
    .expect("the report rows are readable")
}

#[tokio::test]
async fn ingest_records_the_operation_and_queues_a_report() {
    let test = TestDatabase::create()
        .await
        .expect("a disposable Instagram archive database");
    let command = instagram_command("instagram", "https://www.instagram.com/p/Capture123/");
    let outcome = test
        .database
        .ingest_browser_capture_command("cmd.instagram.capture.requested.v1", &command)
        .await
        .expect("the delivery is accepted");
    let submission = match outcome {
        BrowserCaptureIngested::Preserved(submission) => submission,
        other => unreachable!("the first delivery must be preserved: {other:?}"),
    };
    let capture_id = submission.record().capture_id;

    let operations = operation_rows(&test).await;
    assert_eq!(
        operations,
        vec![(
            uuid::Uuid::parse_str(OPERATION_ID).expect("fixed operation UUID"),
            uuid::Uuid::parse_str(COMMAND_ID).expect("fixed command UUID"),
            capture_id,
            uuid::Uuid::parse_str(USER_ID).expect("fixed user UUID"),
            false,
        )],
        "intake records the operation, unreported"
    );
    let due: bool = sqlx::query_scalar(
        "select next_resolution_at is not null from instagram_archive.captures \
         where capture_id = $1",
    )
    .bind(capture_id)
    .fetch_one(test.database.pool())
    .await
    .expect("the capture is readable");
    assert!(due, "intake makes the capture due for resolution");

    let reports = report_rows(&test).await;
    assert_eq!(reports.len(), 1, "exactly one queued report");
    let envelope = &reports[0];
    assert_eq!(envelope["tenant_id"], format!("user:{USER_ID}"));
    assert_eq!(
        envelope["correlation_id"],
        format!("operation:{OPERATION_ID}")
    );
    assert_eq!(envelope["causation_id"], format!("command:{COMMAND_ID}"));
    assert_eq!(envelope["aggregate_id"], format!("capture:{capture_id}"));
    assert_eq!(envelope["producer"], "ratatoskr-instagram");
    assert_eq!(envelope["payload"]["status"], "queued");
    assert_eq!(envelope["payload"]["stage"], "capture_queued");
    assert_eq!(envelope["payload"]["operation_id"], OPERATION_ID);

    let (aggregate_type, aggregate_id, correlation, causation): (
        String,
        uuid::Uuid,
        Option<uuid::Uuid>,
        Option<uuid::Uuid>,
    ) = sqlx::query_as(
        "select aggregate_type, aggregate_id, correlation_id, causation_id \
         from instagram_archive.outbox_events \
         where event_type = 'platform.operation.reported.v1'",
    )
    .fetch_one(test.database.pool())
    .await
    .expect("the outbox row is readable");
    assert_eq!(aggregate_type, "capture");
    assert_eq!(aggregate_id, capture_id);
    assert_eq!(
        correlation.map(|id| id.to_string()).as_deref(),
        Some(OPERATION_ID)
    );
    assert_eq!(
        causation.map(|id| id.to_string()).as_deref(),
        Some(COMMAND_ID)
    );

    let replay = test
        .database
        .ingest_browser_capture_command("cmd.instagram.capture.requested.v1", &command)
        .await
        .expect("the redelivery is recognized");
    assert!(matches!(replay, BrowserCaptureIngested::Duplicate));
    assert_eq!(
        operation_rows(&test).await.len(),
        1,
        "redelivery adds no operation"
    );
    assert_eq!(
        report_rows(&test).await.len(),
        1,
        "redelivery adds no report"
    );
    test.cleanup()
        .await
        .expect("the disposable database is removed");
}

#[tokio::test]
async fn a_second_operation_shares_the_capture_and_resets_an_unavailable_one() {
    let test = TestDatabase::create()
        .await
        .expect("a disposable Instagram archive database");
    let permalink = "https://www.instagram.com/p/Capture123/";
    test.database
        .ingest_browser_capture_command(
            "cmd.instagram.capture.requested.v1",
            &instagram_command("instagram", permalink),
        )
        .await
        .expect("the first delivery is accepted");
    sqlx::query(
        "update instagram_archive.captures \
         set status = 'unavailable', resolution_attempts = 3, next_resolution_at = null",
    )
    .execute(test.database.pool())
    .await
    .expect("the capture is forced unavailable");

    test.database
        .ingest_browser_capture_command(
            "cmd.instagram.capture.requested.v1",
            &command_with_ids(
                SECOND_COMMAND_ID,
                SECOND_OPERATION_ID,
                "instagram",
                permalink,
            ),
        )
        .await
        .expect("the second operation is accepted");

    let operations = operation_rows(&test).await;
    assert_eq!(operations.len(), 2, "one row per operation id");
    assert_eq!(operations[0].2, operations[1].2, "both bind to one capture");
    assert_eq!(
        operations[1].0.to_string(),
        SECOND_OPERATION_ID,
        "the second operation is recorded"
    );
    let (status, attempts, due): (String, i32, bool) = sqlx::query_as(
        "select status, resolution_attempts, next_resolution_at is not null \
         from instagram_archive.captures",
    )
    .fetch_one(test.database.pool())
    .await
    .expect("the capture is readable");
    assert_eq!(status, "accepted", "a new acquisition reopens the capture");
    assert_eq!(attempts, 0);
    assert!(due);
    assert_eq!(
        report_rows(&test).await.len(),
        2,
        "each operation is queued"
    );
    test.cleanup()
        .await
        .expect("the disposable database is removed");
}
