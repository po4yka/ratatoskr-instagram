//! The `JetStream` transport publishes stored envelopes and acknowledges them honestly
//! (XR-021 CONTRACTS.md S02 rules 1 to 4, S03).

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "the isolated broker assertions are the integration contract; the envelopes are JSON documents asserted field by field"
)]

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_nats::jetstream;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use ratatoskr_instagram_archive::UndeliverableClass;
use ratatoskr_instagram_archive::publishing::{EventTransport as _, TransportError};
use ratatoskr_instagram_archive::test_support::TestDatabase;
use ratatoskr_instagram_archive_service::nats_transport::NatsEventTransport;
use ratatoskr_instagram_archive_service::relay::relay_outbox;
use ratatoskr_instagram_archive_service::{RuntimeState, admin_router};
use serde_json::{Value, json};
use support::{AuthBroker, FIXTURE_USER, INSTAGRAM_USER};
use tower::ServiceExt as _;
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

/// A canonical envelope whose serialized form is exactly `length` bytes.
fn envelope_of_length(event_id: Uuid, length: usize) -> String {
    let mut value: Value = serde_json::from_str(&envelope(event_id, "social.source.captured.v1"))
        .expect("the envelope parses");
    value["payload"]["padding"] = Value::String(String::new());
    let base = serde_json::to_string(&value)
        .expect("the body serializes")
        .len();
    value["payload"]["padding"] = Value::String("x".repeat(length - base));
    let body = serde_json::to_string(&value).expect("the body serializes");
    assert_eq!(body.len(), length, "the padding fixes the length exactly");
    body
}

/// The server limit minus the 1 KiB the transport keeps free for headers (R2-04).
const HEADROOM: usize = 1024;

#[tokio::test]
async fn deliver_marks_a_body_over_the_server_limit_undeliverable() {
    let (context, stream) = events_stream().await;
    let limit = context.client().max_payload() - HEADROOM;
    let transport = NatsEventTransport::new(context);
    let event_id = Uuid::now_v7();

    let error = transport
        .deliver(event_id, &envelope_of_length(event_id, limit + 1))
        .await
        .expect_err("a body over the limit cannot be delivered");

    assert!(
        matches!(
            error,
            TransportError::Undeliverable(UndeliverableClass::PayloadTooLarge)
        ),
        "{error:?}"
    );
    assert!(
        messages_with_id(&stream, "evt.social.source.captured.v1", event_id)
            .await
            .is_empty(),
        "nothing was published"
    );
}

#[tokio::test]
async fn deliver_accepts_a_body_exactly_at_the_limit_minus_the_headroom() {
    let (context, stream) = events_stream().await;
    let limit = context.client().max_payload() - HEADROOM;
    let transport = NatsEventTransport::new(context);
    let event_id = Uuid::now_v7();

    transport
        .deliver(event_id, &envelope_of_length(event_id, limit))
        .await
        .expect("a body at the limit is published and acknowledged");

    assert_eq!(
        messages_with_id(&stream, "evt.social.source.captured.v1", event_id)
            .await
            .len(),
        1
    );
}

async fn ready_status(runtime: &Arc<RuntimeState>) -> (StatusCode, Value) {
    let response = admin_router(Arc::clone(runtime), String::new)
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .expect("a valid request"),
        )
        .await
        .expect("the router answers");
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("a collectible body")
        .to_bytes();
    (status, serde_json::from_slice(&body).expect("a JSON body"))
}

async fn wait_for_ready(
    runtime: &Arc<RuntimeState>,
    want: StatusCode,
    mut nudge: impl AsyncFnMut(),
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        nudge().await;
        let (status, body) = ready_status(runtime).await;
        if status == want {
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "readiness never became {want}: {body}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn connect_as(url: &str, (user, password): (&str, &str)) -> async_nats::Client {
    async_nats::ConnectOptions::with_user_and_password(user.to_owned(), password.to_owned())
        .connect(url)
        .await
        .expect("the private broker accepts the configured user")
}

#[tokio::test]
async fn a_refused_publish_flips_readiness_until_a_clean_pass() {
    const PERMITTED: [&str; 3] = [
        "evt.platform.operation.reported.v1",
        "evt.social.source.updated.v1",
        "evt.social.source.removed.v1",
    ];
    let broker = AuthBroker::start(&PERMITTED);
    let privileged = jetstream::new(connect_as(&broker.url(), FIXTURE_USER).await);
    privileged
        .get_or_create_stream(jetstream::stream::Config {
            name: "ratatoskr_events".to_owned(),
            subjects: vec!["evt.>".to_owned()],
            ..jetstream::stream::Config::default()
        })
        .await
        .expect("the privileged identity creates the events stream");
    // The Instagram identity gets a short acknowledgement wait: a refused publish never answers.
    let context = jetstream::ContextBuilder::new()
        .timeout(Duration::from_millis(400))
        .build(connect_as(&broker.url(), INSTAGRAM_USER).await);

    let test = TestDatabase::create().await.expect("a disposable database");
    let event_id = Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.outbox_events \
         (event_id, event_type, aggregate_type, aggregate_id, payload, occurred_at) \
         values ($1, 'social.source.captured.v1', 'capture', $2, $3, now())",
    )
    .bind(event_id)
    .bind(Uuid::now_v7())
    .bind(
        serde_json::from_str::<Value>(&envelope(event_id, "social.source.captured.v1"))
            .expect("JSON"),
    )
    .execute(test.database.pool())
    .await
    .expect("the outbox row is inserted");

    let runtime = Arc::new(RuntimeState::new());
    runtime.mark_startup_complete();
    runtime.set_bus_running();
    let relay = tokio::spawn(relay_outbox(
        test.database.clone(),
        NatsEventTransport::new(context),
        Duration::from_millis(50),
        16,
        runtime.publisher_health(),
    ));
    // The outbox backoff is 60 seconds; the test moves the clock instead of waiting for it.
    let make_due = async || {
        sqlx::query(
            "update instagram_archive.outbox_events set next_attempt_at = now() \
             where event_id = $1 and published_at is null",
        )
        .bind(event_id)
        .execute(test.database.pool())
        .await
        .expect("the row is made due");
    };

    let failing = wait_for_ready(&runtime, StatusCode::SERVICE_UNAVAILABLE, make_due).await;
    let attempts: i32 = sqlx::query_scalar(
        "select attempt_count from instagram_archive.outbox_events where event_id = $1",
    )
    .bind(event_id)
    .fetch_one(test.database.pool())
    .await
    .expect("the attempt count is readable");
    assert!(
        attempts >= 3,
        "readiness flips only from the third failed attempt, saw {attempts}"
    );
    let check = failing["checks"]
        .as_array()
        .and_then(|checks| checks.iter().find(|check| check["name"] == "bus_publish"))
        .expect("a bus_publish check is reported");
    assert_eq!(check["state"], "fail", "{failing}");
    assert!(
        !relay.is_finished(),
        "the relay keeps running while it cannot publish"
    );

    broker.reload(&[
        "evt.platform.operation.reported.v1",
        "evt.social.source.captured.v1",
        "evt.social.source.updated.v1",
        "evt.social.source.removed.v1",
    ]);
    let healthy = wait_for_ready(&runtime, StatusCode::OK, make_due).await;
    let check = healthy["checks"]
        .as_array()
        .and_then(|checks| checks.iter().find(|check| check["name"] == "bus_publish"))
        .expect("the bus_publish check stays reported");
    assert_eq!(check["state"], "pass", "{healthy}");
    let published: bool = sqlx::query_scalar(
        "select published_at is not null from instagram_archive.outbox_events where event_id = $1",
    )
    .bind(event_id)
    .fetch_one(test.database.pool())
    .await
    .expect("the row is readable");
    assert!(published, "the row publishes once the subject is permitted");

    relay.abort();
    test.cleanup()
        .await
        .expect("the disposable database is removed");
}
