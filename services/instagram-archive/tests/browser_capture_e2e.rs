//! One Instagram permalink from a Platform command to the events a consumer sees: the real
//! command consumer, the capture resolver over a fake public surface, the outbox pass and the
//! `JetStream` transport (XR-021 CONTRACTS.md S10 CD1, CD2, CD7, S02).

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "the isolated broker and database assertions are the integration contract; the envelopes are JSON documents asserted field by field"
)]

use std::time::Duration;

use async_nats::jetstream;
use futures_util::StreamExt as _;
use ratatoskr_instagram_archive::capture_resolution::{CaptureResolver, ResolutionPolicy};
use ratatoskr_instagram_archive::permalink::CanonicalPermalink;
use ratatoskr_instagram_archive::publishing::run_once;
use ratatoskr_instagram_archive::test_support::TestDatabase;
use ratatoskr_instagram_archive::{PublicSurface, SurfaceOutcome};
use ratatoskr_instagram_archive_service::command_consumer::consume_one;
use ratatoskr_instagram_archive_service::nats_transport::NatsEventTransport;
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

const REEL_FIXTURE: &str =
    include_str!("../../../crates/instagram-archive/tests/fixtures/oembed/reel_public.json");

struct FixtureSurface;

impl PublicSurface for FixtureSurface {
    async fn fetch(&self, _permalink: &CanonicalPermalink) -> SurfaceOutcome {
        SurfaceOutcome::Payload {
            body: REEL_FIXTURE.to_owned(),
        }
    }
}

fn command(user: Uuid, operation: Uuid) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "command_id": Uuid::now_v7().to_string(),
        "command_type": "social.capture.requested.v1",
        "issued_at": "2026-08-27T12:00:00Z",
        "producer": "ratatoskr-platform",
        "aggregate_id": format!("operation:{operation}"),
        "correlation_id": format!("operation:{operation}"),
        "tenant_id": format!("user:{user}"),
        "schema_version": 1,
        "payload": {
            "operation_id": operation.to_string(),
            "idempotency_key": {
                "algorithm": "sha256",
                "hex": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            },
            "original_permalink": "https://www.instagram.com/reel/Cabc123/",
            "captured_at": "2026-08-27T11:59:00Z",
            "provider": "instagram",
            "acquisition": "browser_extension",
            "saved_authority": "explicit_user_capture"
        }
    }))
    .expect("the fixed command serializes")
}

/// `(subject, envelope)` of every events-stream message of one tenant, in stream order.
async fn tenant_events(stream: &jetstream::stream::Stream, tenant: &str) -> Vec<(String, Value)> {
    let mut found = Vec::new();
    let mut sequence = 1_u64;
    while let Ok(raw) = stream.get_raw_message(sequence).await {
        sequence += 1;
        let Ok(envelope) = serde_json::from_slice::<Value>(&raw.payload) else {
            continue;
        };
        if envelope["tenant_id"] == tenant {
            found.push((raw.subject.to_string(), envelope));
        }
    }
    found
}

/// The command and events streams a Platform deployment provisions, plus a context to publish
/// through. `ratatoskr_*` streams are created here only because the fixture broker is bare.
#[expect(
    clippy::disallowed_methods,
    reason = "the integration binary chooses its isolated JetStream endpoint"
)]
async fn provisioned_streams() -> (
    jetstream::Context,
    jetstream::stream::Stream,
    jetstream::stream::Stream,
) {
    let url = std::env::var("INSTAGRAM_ARCHIVE_TEST_NATS_URL")
        .expect("an isolated JetStream endpoint is required");
    let client = async_nats::connect(url)
        .await
        .expect("the isolated broker connects");
    let context = jetstream::new(client);
    let commands = context
        .get_or_create_stream(jetstream::stream::Config {
            name: "ratatoskr_commands".to_owned(),
            subjects: vec!["cmd.>".to_owned()],
            ..jetstream::stream::Config::default()
        })
        .await
        .expect("the privileged fixture creates the command stream");
    let events = context
        .get_or_create_stream(jetstream::stream::Config {
            name: "ratatoskr_events".to_owned(),
            subjects: vec!["evt.>".to_owned()],
            ..jetstream::stream::Config::default()
        })
        .await
        .expect("the privileged fixture creates the events stream");
    (context, commands, events)
}

/// Delivers the one published command to the real consumer entry point.
async fn consume_published_command(
    consumer: &jetstream::consumer::PullConsumer,
    database: &ratatoskr_instagram_archive::Database,
) {
    let mut messages = consumer
        .messages()
        .await
        .expect("the durable receives deliveries");
    let message = tokio::time::timeout(Duration::from_secs(5), messages.next())
        .await
        .expect("the durable delivers promptly")
        .expect("the durable has the command")
        .expect("the delivery is valid");
    consume_one(database, &message).await;
}

#[tokio::test]
async fn a_platform_command_ends_in_queued_captured_and_succeeded_events() {
    let (context, commands, events) = provisioned_streams().await;
    let durable = format!("e2e_{}", Uuid::now_v7().simple());
    let consumer: jetstream::consumer::PullConsumer = commands
        .create_consumer(jetstream::consumer::pull::Config {
            durable_name: Some(durable.clone()),
            filter_subject: "cmd.instagram.capture.requested.v1".to_owned(),
            ack_policy: jetstream::consumer::AckPolicy::Explicit,
            deliver_policy: jetstream::consumer::DeliverPolicy::New,
            ..jetstream::consumer::pull::Config::default()
        })
        .await
        .expect("a private durable for this test");
    let user = Uuid::now_v7();
    let operation = Uuid::now_v7();
    let published = context
        .publish(
            "cmd.instagram.capture.requested.v1",
            command(user, operation).into(),
        )
        .await
        .expect("the command is accepted")
        .await
        .expect("the broker persists the command");

    let test = TestDatabase::create().await.expect("a disposable database");
    consume_published_command(&consumer, &test.database).await;
    let policy = ResolutionPolicy {
        max_attempts: 5,
        batch_size: 8,
    };
    let resolved = CaptureResolver::new(test.database.clone())
        .run_due_once(
            &FixtureSurface,
            OffsetDateTime::now_utc() + time::Duration::minutes(1),
            policy,
        )
        .await
        .expect("the resolver pass runs");
    assert_eq!(
        (resolved.fetched, resolved.reported),
        (1, 1),
        "{resolved:?}"
    );
    let transport = NatsEventTransport::new(context.clone());
    let pass = run_once(test.database.pool(), &transport, 16)
        .await
        .expect("the outbox pass runs");
    assert_eq!((pass.delivered, pass.failed, pass.remaining), (3, 0, 0));

    let seen = tenant_events(&events, &format!("user:{user}")).await;
    assert_event_sequence(&seen, operation);

    commands
        .delete_consumer(&durable)
        .await
        .expect("the private durable is removed");
    // The command subject is also filtered by the fixed provider durable other tests share; this
    // command was consumed through the private durable above, so it must not linger for them.
    commands
        .delete_message(published.sequence)
        .await
        .expect("the consumed command is removed from the stream");
    test.cleanup()
        .await
        .expect("the disposable database is removed");
}

/// The broker holds, in order: the queued report, the captured fact and the terminal report,
/// all for the command tenant.
fn assert_event_sequence(seen: &[(String, Value)], operation: Uuid) {
    let summary: Vec<(String, Option<String>)> = seen
        .iter()
        .map(|(subject, envelope)| {
            (
                subject.clone(),
                envelope["payload"]["status"].as_str().map(str::to_owned),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (
                "evt.platform.operation.reported.v1".to_owned(),
                Some("queued".to_owned())
            ),
            ("evt.social.source.captured.v1".to_owned(), None),
            (
                "evt.platform.operation.reported.v1".to_owned(),
                Some("succeeded".to_owned())
            ),
        ],
        "queued, captured, then the terminal report, all for the command tenant"
    );
    let [queued, captured, terminal] = seen else {
        unreachable!("three events were asserted above, found {}", seen.len());
    };
    assert_eq!(queued.1["correlation_id"], format!("operation:{operation}"));
    assert_eq!(
        terminal.1["payload"]["results"][0]["target"], captured.1["aggregate_id"],
        "the report points at the published social source"
    );
}
