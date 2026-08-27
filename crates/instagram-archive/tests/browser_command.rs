//! Contract coverage for Platform's Instagram browser-capture command.

use ratatoskr_instagram_archive::{
    BrowserCaptureIngested, CommandCaptureError, decode_browser_capture_command,
    test_support::TestDatabase,
};
use serde_json::json;

const COMMAND_ID: &str = "01991000-0000-7000-8000-000000000001";
const OPERATION_ID: &str = "01991000-0000-7000-8000-000000000002";
const USER_ID: &str = "01991000-0000-7000-8000-000000000003";

#[expect(
    clippy::expect_used,
    reason = "the fixed JSON fixture is authored in this test and failure is a test setup error"
)]
fn instagram_command(provider: &str, permalink: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "command_id": COMMAND_ID,
        "command_type": "social.capture.requested.v1",
        "issued_at": "2026-08-27T12:00:00Z",
        "producer": "ratatoskr-platform",
        "aggregate_id": format!("operation:{OPERATION_ID}"),
        "correlation_id": format!("operation:{OPERATION_ID}"),
        "tenant_id": format!("user:{USER_ID}"),
        "schema_version": 1,
        "payload": {
            "operation_id": OPERATION_ID,
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
    let invalid = instagram_command("instagram", "https://www.instagram.com/example");
    let error = test
        .database
        .ingest_browser_capture_command("cmd.instagram.capture.requested.v1", &invalid)
        .await
        .expect_err("a non-permalink must refuse capture persistence");
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

    let valid = instagram_command("instagram", "https://www.instagram.com/p/Capture123/");
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
