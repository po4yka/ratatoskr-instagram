//! A removal Instagram published is kept and published again until Knowledge acknowledges it
//! (XR-021 CONTRACTS.md R5-11 rule 4): the pass sends the same bytes again under the same
//! `Nats-Msg-Id`, the consumer of the Edge-provisioned durable `ratatoskr_instagram_deletion_acks`
//! ends the resend, and a message that cannot be attributed to Knowledge is terminated.
//!
//! The round trip and the identity tests run a private broker that enforces the shipped stanza;
//! the message-handling tests use the shared `JetStream` fixture with a fresh durable each.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "assertions in a test binary"
)]

#[path = "support/stanza.rs"]
mod stanza;

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use async_nats::jetstream;
use futures_util::StreamExt as _;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use ratatoskr_instagram_archive::publishing::{ResendPolicy, run_once};
use ratatoskr_instagram_archive::test_support::TestDatabase;
use ratatoskr_instagram_archive::{BusConfig, Database};
use ratatoskr_instagram_archive_service::RuntimeState;
use ratatoskr_instagram_archive_service::deletion_acks::{
    AcksError, AcksSummary, DURABLE, EVENTS_STREAM, FILTER_SUBJECT, consume_deletion_acks,
    ensure_acks_consumer,
};
use ratatoskr_instagram_archive_service::nats_transport::{NatsEventTransport, connect_bus};
use ratatoskr_instagram_archive_service::relay::relay_outbox;
use ratatoskr_social_contracts::RemovalReason;
use stanza::{ACK_SUBJECT, KNOWLEDGE_INBOX_PREFIX, StanzaBroker};
use tokio::task::JoinHandle;
use uuid::Uuid;

const OWNER: &str = "user:018f0000-0000-7000-8000-000000000005";
const OTHER_OWNER: &str = "user:018f0000-0000-7000-8000-000000000006";

/// Tests that share the fixture broker's `evt.knowledge.deletion.acknowledged.v1` subject, the
/// process-wide metrics recorder and the log buffer run one at a time.
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[expect(
    clippy::disallowed_methods,
    reason = "the integration binary chooses its isolated JetStream endpoint"
)]
fn nats_url() -> String {
    std::env::var("INSTAGRAM_ARCHIVE_TEST_NATS_URL")
        .unwrap_or_else(|_| "nats://127.0.0.1:14225".to_owned())
}

/// Captures the log lines of the process-global subscriber.
#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("not poisoned")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl LogBuffer {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("not poisoned")).into_owned()
    }
}

impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for LogBuffer {
    type Writer = Self;

    fn make_writer(&'writer self) -> Self::Writer {
        self.clone()
    }
}

/// The metrics recorder and the log buffer this test binary installs once.
static TELEMETRY: OnceLock<(LogBuffer, PrometheusHandle)> = OnceLock::new();

fn telemetry() -> &'static (LogBuffer, PrometheusHandle) {
    TELEMETRY.get_or_init(|| {
        let buffer = LogBuffer::default();
        tracing::subscriber::set_global_default(
            tracing_subscriber::fmt()
                .json()
                .with_max_level(tracing::Level::INFO)
                .with_writer(buffer.clone())
                .finish(),
        )
        .expect("this test binary installs the log subscriber once");
        let recorder = PrometheusBuilder::new()
            .install_recorder()
            .expect("this test binary installs the only metrics recorder");
        (buffer, recorder)
    })
}

/// A capture that is tombstoned, so the outbox holds a real removal envelope of `owner`.
async fn removal_of(database: &Database, owner: Uuid) -> Uuid {
    let capture_id = Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.captures \
         (capture_id, user_ref, canonical_url, acquisition_method, saved_authority, \
          client_source, status, captured_at) \
         values ($1, $2, 'https://www.instagram.com/p/ACKME001/', 'share_extension', \
          'explicit_user_capture', 'ios_share_extension', 'accepted', now())",
    )
    .bind(capture_id)
    .bind(owner)
    .execute(database.pool())
    .await
    .expect("the capture is stored");
    database
        .tombstone_capture(
            capture_id,
            RemovalReason::UserRequested,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("the tombstone is recorded");
    sqlx::query_scalar(
        "select event_id from instagram_archive.outbox_events \
         where aggregate_id = $1 and event_type = 'social.source.removed.v1'",
    )
    .bind(capture_id)
    .fetch_one(database.pool())
    .await
    .expect("the removal row exists")
}

fn owner_uuid() -> Uuid {
    OWNER
        .trim_start_matches("user:")
        .parse()
        .expect("the fixture owner is a UUID")
}

/// What Knowledge publishes: the envelope of CONTRACTS.md R5-11 rule 1.
struct Answer {
    acknowledged: Uuid,
    owner: &'static str,
    family: &'static str,
    outcome: &'static str,
    refusal: Option<&'static str>,
    producer: &'static str,
}

impl Answer {
    fn applied(acknowledged: Uuid) -> Self {
        Self {
            acknowledged,
            owner: OWNER,
            family: "social",
            outcome: "applied",
            refusal: None,
            producer: "ratatoskr-knowledge",
        }
    }

    fn refused(acknowledged: Uuid) -> Self {
        Self {
            outcome: "refused",
            refusal: Some("foreign_subject"),
            ..Self::applied(acknowledged)
        }
    }

    fn document(&self) -> String {
        let mut payload = serde_json::json!({
            "acknowledged_event_id": self.acknowledged,
            "family": self.family,
            "owner": self.owner,
            "subject": format!("social_source:{}", self.acknowledged),
            "outcome": self.outcome,
            "acknowledged_at": "2026-10-12T08:00:02Z",
        });
        if let (Some(refusal), Some(object)) = (self.refusal, payload.as_object_mut()) {
            object.insert("refusal".to_owned(), serde_json::Value::from(refusal));
        }
        serde_json::json!({
            "event_id": Uuid::now_v7(),
            "event_type": "knowledge.deletion.acknowledged.v1",
            "occurred_at": "2026-10-12T08:00:02Z",
            "producer": self.producer,
            "aggregate_id": format!("social_source:{}", self.acknowledged),
            "correlation_id": format!("event:{}", self.acknowledged),
            "causation_id": format!("event:{}", self.acknowledged),
            "tenant_id": self.owner,
            "schema_version": 1,
            "payload": payload,
        })
        .to_string()
    }
}

/// A running consumer whose summary is read when it stops.
struct Running {
    stop: tokio::sync::watch::Sender<bool>,
    handle: JoinHandle<Result<AcksSummary, AcksError>>,
}

impl Running {
    fn start(context: &jetstream::Context, database: &Database, durable: &str) -> Self {
        let (stop, mut stopped) = tokio::sync::watch::channel(false);
        let (context, database, durable) = (context.clone(), database.clone(), durable.to_owned());
        let handle = tokio::spawn(async move {
            consume_deletion_acks(&context, &database, EVENTS_STREAM, &durable, async move {
                let _ = stopped.wait_for(|requested| *requested).await;
            })
            .await
        });
        Self { stop, handle }
    }

    async fn finish(self) -> AcksSummary {
        let _ = self.stop.send(true);
        self.handle
            .await
            .expect("the consumer task does not panic")
            .expect("the consumer stops cleanly")
    }
}

/// `(acknowledged, outcome, resend_count)` of a row.
async fn row_state(database: &Database, id: Uuid) -> (bool, Option<String>, i32) {
    sqlx::query_as(
        "select acknowledged_at is not null, ack_outcome, resend_count \
         from instagram_archive.outbox_events where event_id = $1",
    )
    .bind(id)
    .fetch_one(database.pool())
    .await
    .expect("the row is readable")
}

async fn wait_for_acknowledgement(database: &Database, id: Uuid) -> (bool, Option<String>, i32) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let state = row_state(database, id).await;
        if state.0 {
            return state;
        }
        assert!(
            Instant::now() < deadline,
            "the removal was never acknowledged"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Waits until the durable has acknowledged every message up to stream sequence `sequence`.
async fn wait_until_consumed(
    context: &jetstream::Context,
    durable: &str,
    sequence: u64,
) -> jetstream::consumer::Info {
    let consumer: jetstream::consumer::Consumer<jetstream::consumer::pull::Config> = context
        .get_consumer_from_stream(durable, EVENTS_STREAM)
        .await
        .expect("the durable exists");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let mut handle = consumer.clone();
        let info = handle.info().await.expect("the durable has info").clone();
        if info.ack_floor.stream_sequence >= sequence && info.num_ack_pending == 0 {
            return info;
        }
        assert!(
            Instant::now() < deadline,
            "the consumer never reached stream sequence {sequence}: {info:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn publish(context: &jetstream::Context, subject: &str, body: String) -> u64 {
    context
        .publish(subject.to_owned(), body.into())
        .await
        .expect("the message publishes")
        .await
        .expect("the broker stores the message")
        .sequence
}

fn durable_config(
    durable: &str,
    filter: &str,
    ack_wait: Duration,
) -> jetstream::consumer::pull::Config {
    jetstream::consumer::pull::Config {
        durable_name: Some(durable.to_owned()),
        filter_subject: filter.to_owned(),
        ack_policy: jetstream::consumer::AckPolicy::Explicit,
        ack_wait,
        deliver_policy: jetstream::consumer::DeliverPolicy::New,
        ..jetstream::consumer::pull::Config::default()
    }
}

/// Edge's side on the shared broker: the stream and one fresh durable of the right shape.
struct Fixture {
    test: TestDatabase,
    context: jetstream::Context,
    durable: String,
}

async fn fixture() -> Fixture {
    let test = TestDatabase::create().await.expect("a disposable database");
    let context = jetstream::new(
        async_nats::connect(nats_url())
            .await
            .expect("the test broker connects"),
    );
    let events = context
        .get_or_create_stream(jetstream::stream::Config {
            name: EVENTS_STREAM.to_owned(),
            subjects: vec!["evt.>".to_owned()],
            ..jetstream::stream::Config::default()
        })
        .await
        .expect("the events stream exists");
    let durable = format!("instagram_deletion_acks_{}", Uuid::now_v7().simple());
    events
        .create_consumer(durable_config(
            &durable,
            FILTER_SUBJECT,
            Duration::from_secs(30),
        ))
        .await
        .expect("Edge provisions the durable");
    Fixture {
        test,
        context,
        durable,
    }
}

/// Edge's and the service's contexts on a private broker that runs the shipped stanza, whose
/// events stream has a duplicate window of 200 ms so a test can see the second copy of a message
/// id. The production window is 120 s, which is why the production resend delay defaults to 600 s.
async fn private_contexts(broker: &StanzaBroker) -> (jetstream::Context, jetstream::Context) {
    let administrator =
        async_nats::ConnectOptions::with_nkey(broker.administrator_seed().to_owned())
            .connect(broker.url())
            .await
            .expect("Edge connects");
    let edge = jetstream::new(administrator);
    let events = edge
        .get_or_create_stream(jetstream::stream::Config {
            name: EVENTS_STREAM.to_owned(),
            subjects: vec!["evt.>".to_owned()],
            duplicate_window: Duration::from_millis(200),
            ..jetstream::stream::Config::default()
        })
        .await
        .expect("the events stream exists");
    events
        .create_consumer(jetstream::consumer::pull::Config {
            deliver_policy: jetstream::consumer::DeliverPolicy::All,
            ..durable_config(DURABLE, FILTER_SUBJECT, Duration::from_secs(30))
        })
        .await
        .expect("Edge provisions the durable");
    let client = connect_bus(&BusConfig {
        url: broker.url(),
        nkey_seed_path: Some(broker.instagram_seed_path().to_path_buf()),
    })
    .await
    .expect("the service connects under the shipped stanza");
    (edge, jetstream::new(client))
}

async fn knowledge_context(broker: &StanzaBroker) -> jetstream::Context {
    let client = async_nats::ConnectOptions::with_nkey(broker.knowledge_seed().to_owned())
        .custom_inbox_prefix(KNOWLEDGE_INBOX_PREFIX)
        .connect(broker.url())
        .await
        .expect("Knowledge connects");
    jetstream::new(client)
}

/// The `Nats-Msg-Id` and bytes of the first `count` messages stored in the stream.
async fn stored_copies(stream: &jetstream::stream::Stream, count: u64) -> Vec<(String, Vec<u8>)> {
    let mut copies = Vec::new();
    for sequence in 1..=count {
        let raw = stream
            .get_raw_message(sequence)
            .await
            .expect("the stored copy is readable");
        let message_id = raw
            .headers
            .get(async_nats::header::NATS_MESSAGE_ID)
            .expect("the copy carries a Nats-Msg-Id")
            .to_string();
        copies.push((message_id, raw.payload.to_vec()));
    }
    copies
}

fn gauge(rendered: &str, name: &str) -> f64 {
    rendered
        .lines()
        .find(|line| line.starts_with(name))
        .and_then(|line| line.rsplit(' ').next())
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("the gauge {name} is rendered:\n{rendered}"))
}

#[tokio::test]
async fn an_unacknowledged_removal_is_published_again_until_knowledge_acknowledges_it() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let (_, metrics) = telemetry();
    let broker = StanzaBroker::start();
    let (edge, context) = private_contexts(&broker).await;
    let test = TestDatabase::create().await.expect("a disposable database");
    let id = removal_of(&test.database, owner_uuid()).await;
    let transport = NatsEventTransport::new(context.clone());
    let policy = ResendPolicy::new(Duration::from_millis(400), Duration::from_secs(4));
    let consumer = Running::start(&context, &test.database, DURABLE);

    eprintln!("transcript: pass 1, the removal {id} is published");
    let first = run_once(test.database.pool(), &transport, 16, policy)
        .await
        .expect("the first pass");
    assert_eq!((first.delivered, first.resent), (1, 0));
    assert_eq!(
        row_state(&test.database, id).await,
        (false, None, 0),
        "the first publish is not an acknowledgement"
    );
    assert!(
        (gauge(&metrics.render(), "instagram_unacknowledged_tombstones") - 1.0).abs() < 0.5,
        "the unacknowledged removal is visible to an operator"
    );

    eprintln!("transcript: no acknowledgement; waiting past resend_after and the duplicate window");
    tokio::time::sleep(Duration::from_millis(600)).await;
    let again = run_once(test.database.pool(), &transport, 16, policy)
        .await
        .expect("the resend pass");
    assert_eq!((again.delivered, again.resent), (0, 1));
    eprintln!("transcript: pass 2 published the stored bytes again");

    let mut stream = edge
        .get_stream(EVENTS_STREAM)
        .await
        .expect("the events stream is readable");
    let info = stream.info().await.expect("the stream has info");
    assert_eq!(info.state.messages, 2, "the stream holds both copies");
    let copies = stored_copies(&stream, 2).await;
    assert_eq!(copies[0].0, id.to_string(), "the message id is the row id");
    assert_eq!(copies[0], copies[1], "same Nats-Msg-Id, same bytes");
    for (sequence, (message_id, _)) in (1..).zip(&copies) {
        eprintln!("transcript: stream sequence {sequence} Nats-Msg-Id {message_id}");
    }

    let sequence = publish(
        &knowledge_context(&broker).await,
        ACK_SUBJECT,
        Answer::applied(id).document(),
    )
    .await;
    eprintln!("transcript: Knowledge acknowledged at stream sequence {sequence}");
    let (acknowledged, outcome, resends) = wait_for_acknowledgement(&test.database, id).await;
    assert!(acknowledged);
    assert_eq!(outcome.as_deref(), Some("applied"));
    assert_eq!(resends, 1);

    tokio::time::sleep(Duration::from_millis(600)).await;
    sqlx::query(
        "update instagram_archive.outbox_events set next_resend_at = now() where event_id = $1",
    )
    .bind(id)
    .execute(test.database.pool())
    .await
    .expect("the resend is made due");
    let after = run_once(test.database.pool(), &transport, 16, policy)
        .await
        .expect("the pass after the answer");
    assert_eq!(after.resent, 0, "the acknowledgement ended the resend");
    eprintln!("transcript: pass 3 resent nothing");
    let info = stream.info().await.expect("the stream has info");
    assert_eq!(info.state.messages, 3, "two copies and the acknowledgement");
    assert!(
        gauge(&metrics.render(), "instagram_unacknowledged_tombstones") < 0.5,
        "the gauge moves down once Knowledge answered"
    );

    let summary = consumer.finish().await;
    assert_eq!(summary.applied, 1);
    test.cleanup().await.expect("cleanup drops the database");
}

#[tokio::test]
async fn a_refused_acknowledgement_stops_the_resend_raises_an_error_and_counts() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let (logs, metrics) = telemetry();
    let fixture = fixture().await;
    let id = removal_of(&fixture.test.database, owner_uuid()).await;
    let transport = NatsEventTransport::new(fixture.context.clone());
    run_once(
        fixture.test.database.pool(),
        &transport,
        16,
        ResendPolicy::default(),
    )
    .await
    .expect("the removal is published");
    let consumer = Running::start(&fixture.context, &fixture.test.database, &fixture.durable);

    let sequence = publish(
        &fixture.context,
        ACK_SUBJECT,
        Answer::refused(id).document(),
    )
    .await;
    let (acknowledged, outcome, _) = wait_for_acknowledgement(&fixture.test.database, id).await;
    wait_until_consumed(&fixture.context, &fixture.durable, sequence).await;
    let summary = consumer.finish().await;

    assert!(acknowledged, "a refusal ends the resend too");
    assert_eq!(outcome.as_deref(), Some("refused"));
    assert_eq!(summary.refused, 1);
    let refusal = logs
        .text()
        .lines()
        .find(|line| line.contains(&id.to_string()) && line.contains("\"ERROR\""))
        .expect("an ERROR line names the refused removal")
        .to_owned();
    assert!(
        !refusal.contains(OWNER) && !refusal.contains("social_source:"),
        "the line names ids only, never the owner or the subject: {refusal}"
    );
    assert!(
        metrics
            .render()
            .lines()
            .any(|line| line.trim() == "instagram_tombstone_refused_total 1"),
        "the refusal is counted once:\n{}",
        metrics.render()
    );
    fixture
        .test
        .cleanup()
        .await
        .expect("cleanup drops the database");
}

#[tokio::test]
async fn acknowledgements_that_name_no_open_removal_are_acknowledged_and_ignored() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let fixture = fixture().await;
    let id = removal_of(&fixture.test.database, owner_uuid()).await;
    let consumer = Running::start(&fixture.context, &fixture.test.database, &fixture.durable);

    let mut last = 0;
    for answer in [
        Answer::applied(Uuid::now_v7()),
        Answer {
            owner: OTHER_OWNER,
            ..Answer::applied(id)
        },
        Answer {
            family: "ai_archive",
            ..Answer::applied(id)
        },
    ] {
        last = publish(&fixture.context, ACK_SUBJECT, answer.document()).await;
    }
    let info = wait_until_consumed(&fixture.context, &fixture.durable, last).await;
    let summary = consumer.finish().await;

    assert_eq!(summary.ignored, 3, "{summary:?}");
    assert_eq!(
        (summary.applied, summary.refused, summary.terminated),
        (0, 0, 0)
    );
    assert_eq!(
        info.num_redelivered, 0,
        "each message was acknowledged once"
    );
    assert_eq!(
        row_state(&fixture.test.database, id).await,
        (false, None, 0),
        "no foreign acknowledgement changed the removal"
    );

    // The right tenant, family and id do end it, and a repeat changes nothing.
    let consumer = Running::start(&fixture.context, &fixture.test.database, &fixture.durable);
    publish(
        &fixture.context,
        ACK_SUBJECT,
        Answer::applied(id).document(),
    )
    .await;
    let last = publish(
        &fixture.context,
        ACK_SUBJECT,
        Answer::refused(id).document(),
    )
    .await;
    wait_until_consumed(&fixture.context, &fixture.durable, last).await;
    let summary = consumer.finish().await;
    assert_eq!((summary.applied, summary.ignored), (1, 1), "{summary:?}");
    assert_eq!(
        row_state(&fixture.test.database, id).await.1.as_deref(),
        Some("applied"),
        "the first answer stands"
    );
    fixture
        .test
        .cleanup()
        .await
        .expect("cleanup drops the database");
}

#[tokio::test]
async fn messages_that_are_not_a_valid_knowledge_acknowledgement_are_terminated() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let fixture = fixture().await;
    let id = removal_of(&fixture.test.database, owner_uuid()).await;
    let consumer = Running::start(&fixture.context, &fixture.test.database, &fixture.durable);

    let mut wrong_type: serde_json::Value =
        serde_json::from_str(&Answer::applied(id).document()).expect("a document");
    wrong_type["event_type"] = serde_json::Value::from("platform.operation.reported.v1");
    let mut refused_without_reason: serde_json::Value =
        serde_json::from_str(&Answer::applied(id).document()).expect("a document");
    refused_without_reason["payload"]["outcome"] = serde_json::Value::from("refused");
    let mut other_tenant: serde_json::Value =
        serde_json::from_str(&Answer::applied(id).document()).expect("a document");
    other_tenant["tenant_id"] = serde_json::Value::from(OTHER_OWNER);
    let mut last = 0;
    for body in [
        "not json at all".to_owned(),
        Answer {
            producer: "ratatoskr-chatgpt",
            ..Answer::applied(id)
        }
        .document(),
        wrong_type.to_string(),
        refused_without_reason.to_string(),
        other_tenant.to_string(),
    ] {
        last = publish(&fixture.context, ACK_SUBJECT, body).await;
    }
    let info = wait_until_consumed(&fixture.context, &fixture.durable, last).await;
    let summary = consumer.finish().await;

    assert_eq!(summary.terminated, 5, "{summary:?}");
    assert_eq!((summary.applied, summary.ignored), (0, 0));
    assert_eq!(
        info.num_redelivered, 0,
        "a terminated message is never redelivered"
    );
    assert_eq!(
        row_state(&fixture.test.database, id).await,
        (false, None, 0),
        "nothing that is not Knowledge's changed the removal"
    );
    fixture
        .test
        .cleanup()
        .await
        .expect("cleanup drops the database");
}

#[tokio::test]
async fn a_durable_with_another_filter_or_ack_wait_refuses_to_become_ready() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let context = jetstream::new(
        async_nats::connect(nats_url())
            .await
            .expect("the test broker connects"),
    );
    let events = context
        .get_or_create_stream(jetstream::stream::Config {
            name: EVENTS_STREAM.to_owned(),
            subjects: vec!["evt.>".to_owned()],
            ..jetstream::stream::Config::default()
        })
        .await
        .expect("the events stream exists");
    let suffix = Uuid::now_v7().simple();
    let good = format!("instagram_deletion_acks_good_{suffix}");
    let other_filter = format!("instagram_deletion_acks_filter_{suffix}");
    let other_wait = format!("instagram_deletion_acks_wait_{suffix}");
    for config in [
        durable_config(&good, FILTER_SUBJECT, Duration::from_secs(30)),
        durable_config(&other_filter, "evt.knowledge.>", Duration::from_secs(30)),
        durable_config(&other_wait, FILTER_SUBJECT, Duration::from_secs(10)),
    ] {
        events
            .create_consumer(config)
            .await
            .expect("the durable exists");
    }

    ensure_acks_consumer(&context, EVENTS_STREAM, &good)
        .await
        .expect("a durable of the right shape verifies");
    for wrong in [&other_filter, &other_wait] {
        ensure_acks_consumer(&context, EVENTS_STREAM, wrong)
            .await
            .expect_err("a durable of another shape is refused");
    }
    ensure_acks_consumer(&context, EVENTS_STREAM, &format!("absent_{suffix}"))
        .await
        .expect_err("an absent durable is refused: the client never creates one");

    let test = TestDatabase::create().await.expect("a disposable database");
    consume_deletion_acks(
        &context,
        &test.database,
        EVENTS_STREAM,
        &other_wait,
        std::future::pending(),
    )
    .await
    .expect_err("the consumer verifies the durable before it reads");
    test.cleanup().await.expect("cleanup drops the database");
}

#[tokio::test]
async fn readiness_stays_green_while_knowledge_never_answers() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let broker = StanzaBroker::start();
    let (edge, context) = private_contexts(&broker).await;
    let test = TestDatabase::create().await.expect("a disposable database");
    let id = removal_of(&test.database, owner_uuid()).await;
    let runtime = Arc::new(RuntimeState::new());
    runtime.mark_startup_complete();
    runtime.set_bus_running();
    let relay = tokio::spawn(relay_outbox(
        test.database.clone(),
        NatsEventTransport::new(context),
        Duration::from_millis(50),
        16,
        ResendPolicy::new(Duration::from_millis(400), Duration::from_millis(800)),
        runtime.publisher_health(),
    ));

    tokio::time::sleep(Duration::from_secs(3)).await;

    let (acknowledged, _, resends) = row_state(&test.database, id).await;
    assert!(!acknowledged, "Knowledge never answered");
    assert!(
        resends >= 3,
        "the removal keeps being published again, the delay doubling to the cap: {resends}"
    );
    assert!(
        runtime.is_ready(),
        "a Knowledge outage never makes the service unready"
    );
    let stream = edge
        .get_stream(EVENTS_STREAM)
        .await
        .expect("the events stream is readable");
    let copies: Vec<_> = stored_copies(&stream, 3).await;
    assert!(
        copies.iter().all(|copy| copy.0 == id.to_string()),
        "every copy carries the same Nats-Msg-Id"
    );
    relay.abort();
    test.cleanup().await.expect("cleanup drops the database");
}

#[tokio::test]
async fn the_identity_can_fetch_from_its_own_durable_and_from_no_sibling() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let broker = StanzaBroker::start();
    let (edge, context) = private_contexts(&broker).await;
    for sibling in [
        "ratatoskr_x_deletion_acks",
        "ratatoskr_threads_deletion_acks",
    ] {
        edge.get_stream(EVENTS_STREAM)
            .await
            .expect("the events stream exists")
            .create_consumer(durable_config(
                sibling,
                FILTER_SUBJECT,
                Duration::from_secs(30),
            ))
            .await
            .expect("Edge provisions the sibling durable");
    }
    publish(
        &knowledge_context(&broker).await,
        ACK_SUBJECT,
        Answer::applied(Uuid::now_v7()).document(),
    )
    .await;

    let own: jetstream::consumer::PullConsumer = tokio::time::timeout(
        Duration::from_secs(10),
        context.get_consumer_from_stream(DURABLE, EVENTS_STREAM),
    )
    .await
    .expect("the own durable answers")
    .expect("the own durable verifies under the stanza");
    let mut batch = tokio::time::timeout(
        Duration::from_secs(10),
        own.fetch().max_messages(1).messages(),
    )
    .await
    .expect("the pull request is answered")
    .expect("the pull request is accepted");
    let message = tokio::time::timeout(Duration::from_secs(10), batch.next())
        .await
        .expect("the delivery arrives")
        .expect("the durable holds the acknowledgement")
        .expect("the delivery is valid");
    message
        .ack()
        .await
        .expect("the narrow $JS.ACK grant covers the acknowledgement");

    for sibling in [
        "ratatoskr_x_deletion_acks",
        "ratatoskr_threads_deletion_acks",
    ] {
        let attempt = tokio::time::timeout(
            Duration::from_secs(3),
            context.get_consumer_from_stream::<jetstream::consumer::pull::Config, _, _>(
                sibling,
                EVENTS_STREAM,
            ),
        )
        .await;
        assert!(
            attempt.is_err() || attempt.is_ok_and(|result| result.is_err()),
            "the identity must not reach the durable of {sibling}"
        );
    }
}
