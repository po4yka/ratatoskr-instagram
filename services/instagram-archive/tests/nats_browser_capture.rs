//! Real `JetStream` delivery coverage for the Instagram browser-capture worker.

#![allow(
    clippy::expect_used,
    reason = "the isolated broker and database assertions are the integration contract"
)]

use std::time::Duration;

use async_nats::jetstream;
use futures_util::StreamExt as _;
use ratatoskr_instagram_archive::test_support::TestDatabase;
use ratatoskr_instagram_archive_service::command_consumer::consume_one;
use serde_json::json;

const COMMAND_ID: &str = "01991000-0000-7000-8000-000000000011";
const OPERATION_ID: &str = "01991000-0000-7000-8000-000000000012";
const USER_ID: &str = "01991000-0000-7000-8000-000000000013";

fn command() -> Vec<u8> {
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
            "idempotency_key": {"algorithm":"sha256","hex":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
            "original_permalink": "https://www.instagram.com/p/Capture123/",
            "captured_at": "2026-08-27T11:59:00Z",
            "provider": "instagram",
            "acquisition": "browser_extension",
            "saved_authority": "explicit_user_capture"
        }
    }))
    .expect("the fixed command fixture serializes")
}

#[tokio::test]
#[expect(
    clippy::disallowed_methods,
    reason = "the integration binary chooses its isolated JetStream endpoint"
)]
async fn preprovisioned_durable_persists_and_acknowledges_one_browser_capture() {
    let url = std::env::var("INSTAGRAM_ARCHIVE_TEST_NATS_URL")
        .expect("an isolated JetStream endpoint is required");
    let client = async_nats::connect(url)
        .await
        .expect("the isolated broker connects");
    let context = jetstream::new(client);
    let stream = context
        .create_stream(jetstream::stream::Config {
            name: "ratatoskr_commands".to_owned(),
            subjects: vec!["cmd.>".to_owned()],
            ..jetstream::stream::Config::default()
        })
        .await
        .expect("the privileged fixture creates the command stream");
    let consumer: jetstream::consumer::PullConsumer = stream
        .create_consumer(jetstream::consumer::pull::Config {
            durable_name: Some("ratatoskr_instagram_browser_capture".to_owned()),
            filter_subject: "cmd.instagram.capture.requested.v1".to_owned(),
            ack_policy: jetstream::consumer::AckPolicy::Explicit,
            ..jetstream::consumer::pull::Config::default()
        })
        .await
        .expect("the privileged fixture preprovisions the fixed durable");
    context
        .publish("cmd.instagram.capture.requested.v1", command().into())
        .await
        .expect("the canonical command is accepted")
        .await
        .expect("the broker persists the command");

    let test = TestDatabase::create()
        .await
        .expect("a disposable archive database");
    let mut messages = consumer
        .messages()
        .await
        .expect("the durable receives deliveries");
    let message = tokio::time::timeout(Duration::from_secs(2), messages.next())
        .await
        .expect("the preprovisioned durable delivers promptly")
        .expect("the durable has one message")
        .expect("the delivery is valid");
    consume_one(&test.database, &message).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let captures: i64 = sqlx::query_scalar("select count(*) from instagram_archive.captures")
        .fetch_one(test.database.pool())
        .await
        .expect("capture count answers");
    assert_eq!(captures, 1);
    let info = consumer.get_info().await.expect("consumer info answers");
    assert_eq!(
        info.num_ack_pending, 0,
        "the durable has no unacknowledged delivery"
    );
    test.cleanup()
        .await
        .expect("the disposable database is removed");
}
