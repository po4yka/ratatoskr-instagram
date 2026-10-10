//! The `JetStream` transport publishes stored envelopes and acknowledges them honestly
//! (XR-021 CONTRACTS.md S02 rules 1 to 4, S03).

#![allow(
    clippy::expect_used,
    reason = "the isolated broker assertions are the integration contract"
)]

use async_nats::jetstream;
use ratatoskr_instagram_archive::publishing::EventTransport as _;
use ratatoskr_instagram_archive_service::nats_transport::NatsEventTransport;
use serde_json::{Value, json};
use uuid::Uuid;

#[expect(
    clippy::disallowed_methods,
    reason = "the integration binary chooses its isolated JetStream endpoint"
)]
async fn events_stream() -> (jetstream::Context, jetstream::stream::Stream) {
    let url = std::env::var("INSTAGRAM_ARCHIVE_TEST_NATS_URL")
        .expect("an isolated JetStream endpoint is required");
    let client = async_nats::connect(url)
        .await
        .expect("the isolated broker connects");
    let context = jetstream::new(client);
    let stream = context
        .get_or_create_stream(jetstream::stream::Config {
            name: "ratatoskr_events".to_owned(),
            subjects: vec!["evt.>".to_owned()],
            ..jetstream::stream::Config::default()
        })
        .await
        .expect("the privileged fixture creates the events stream");
    (context, stream)
}

fn envelope(event_id: Uuid, event_type: &str) -> String {
    let operation = Uuid::now_v7();
    serde_json::to_string(&json!({
        "event_id": event_id.to_string(),
        "event_type": event_type,
        "occurred_at": "2026-08-27T12:00:00Z",
        "producer": "ratatoskr-instagram",
        "aggregate_id": format!("capture:{}", Uuid::now_v7()),
        "correlation_id": format!("operation:{operation}"),
        "causation_id": format!("command:{}", Uuid::now_v7()),
        "tenant_id": format!("user:{}", Uuid::now_v7()),
        "schema_version": 1,
        "payload": {
            "operation_id": operation.to_string(),
            "status": "queued",
            "stage": "capture_queued"
        }
    }))
    .expect("the fixed envelope serializes")
}

/// Every message the stream holds on a subject whose `Nats-Msg-Id` is `event_id`.
async fn messages_with_id(
    stream: &jetstream::stream::Stream,
    subject: &str,
    event_id: Uuid,
) -> Vec<Value> {
    let mut found = Vec::new();
    let mut sequence = 1_u64;
    while let Ok(raw) = stream.get_raw_message(sequence).await {
        sequence += 1;
        if raw.subject.as_str() != subject {
            continue;
        }
        let id = raw
            .headers
            .get(async_nats::header::NATS_MESSAGE_ID)
            .map(std::string::ToString::to_string);
        if id.as_deref() == Some(event_id.to_string().as_str()) {
            found.push(serde_json::from_slice(&raw.payload).expect("a JSON payload"));
        }
    }
    found
}

#[tokio::test]
async fn deliver_publishes_to_the_event_subject_with_the_event_id_as_message_id() {
    let (context, stream) = events_stream().await;
    let transport = NatsEventTransport::new(context);
    let event_id = Uuid::now_v7();
    let body = envelope(event_id, "platform.operation.reported.v1");

    transport
        .deliver(event_id, &body)
        .await
        .expect("a stored envelope is published and acknowledged");

    let stored = messages_with_id(&stream, "evt.platform.operation.reported.v1", event_id).await;
    assert_eq!(stored.len(), 1, "the broker holds the published envelope");
    assert_eq!(stored[0]["event_id"], event_id.to_string());

    transport
        .deliver(event_id, &body)
        .await
        .expect("a redelivery is acknowledged as a duplicate");
    assert_eq!(
        messages_with_id(&stream, "evt.platform.operation.reported.v1", event_id)
            .await
            .len(),
        1,
        "the message id deduplicates a redelivery"
    );
}

#[tokio::test]
async fn deliver_refuses_an_event_type_outside_the_allowlist() {
    let (context, stream) = events_stream().await;
    let transport = NatsEventTransport::new(context);
    let event_id = Uuid::now_v7();

    let error = transport
        .deliver(
            event_id,
            &envelope(event_id, "knowledge.analysis.completed.v1"),
        )
        .await
        .expect_err("an unlisted type has no subject");

    assert!(error.to_string().contains("allowed"), "{error}");
    assert!(
        messages_with_id(&stream, "evt.knowledge.analysis.completed.v1", event_id)
            .await
            .is_empty(),
        "nothing was published"
    );
}

#[tokio::test]
async fn deliver_refuses_an_envelope_whose_id_differs_from_the_row() {
    let (context, _stream) = events_stream().await;
    let transport = NatsEventTransport::new(context);
    let row_id = Uuid::now_v7();

    let error = transport
        .deliver(
            row_id,
            &envelope(Uuid::now_v7(), "social.source.captured.v1"),
        )
        .await
        .expect_err("the message id must be the envelope id");

    assert!(error.to_string().contains("id"), "{error}");
}

#[tokio::test]
async fn deliver_refuses_a_body_that_is_not_an_envelope() {
    let (context, _stream) = events_stream().await;
    let transport = NatsEventTransport::new(context);

    let error = transport
        .deliver(Uuid::now_v7(), "{\"hello\":\"world\"}")
        .await
        .expect_err("a bare payload is not a canonical event");

    assert!(error.to_string().contains("canonical"), "{error}");
}
