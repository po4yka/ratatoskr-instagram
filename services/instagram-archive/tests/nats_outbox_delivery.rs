//! Real PostgreSQL/JetStream proof for the deployable outbox transport.

use std::sync::OnceLock;
use std::time::Duration;
use std::{fs, process::Command};

use async_nats::jetstream;
use futures_util::StreamExt as _;
use ratatoskr_instagram_archive::publishing::run_once;
use ratatoskr_instagram_archive::test_support::TestDatabase;
use ratatoskr_instagram_archive_service::outbox_transport::JetStreamTransport;
use time::OffsetDateTime;
use uuid::Uuid;

const STREAM: &str = "ratatoskr_instagram_outbox_test";
const NATS_IMAGE: &str =
    "nats:2-alpine@sha256:d4ac35882ac65aff236cd65b9d3fa4d24332c681e1a85f94eedccd3cdd65b1da";

fn broker_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

#[expect(
    clippy::disallowed_methods,
    reason = "integration fixture selection uses an explicit test-only environment variable"
)]
fn actual_policy_fixture_configured() -> bool {
    std::env::var("INSTAGRAM_ARCHIVE_TEST_NATS_NKEY_SEED_PATH").is_ok()
}

#[expect(
    clippy::expect_used,
    clippy::disallowed_methods,
    reason = "integration fixture: the explicitly configured broker is mandatory"
)]
async fn broker() -> async_nats::Client {
    let url = std::env::var("INSTAGRAM_ARCHIVE_TEST_NATS_URL")
        .expect("an isolated JetStream endpoint is required");
    if let Ok(seed_path) = std::env::var("INSTAGRAM_ARCHIVE_TEST_NATS_NKEY_SEED_PATH") {
        let seed = fs::read_to_string(seed_path).expect("the publisher seed file is readable");
        async_nats::ConnectOptions::with_nkey(seed.trim().to_owned())
            .connect(url)
            .await
            .expect("the isolated publisher identity connects")
    } else {
        async_nats::connect(url)
            .await
            .expect("the isolated JetStream endpoint connects")
    }
}

#[expect(
    clippy::expect_used,
    clippy::disallowed_methods,
    reason = "integration fixture: the optional privileged identity is explicitly isolated"
)]
async fn admin_broker(fallback: async_nats::Client) -> async_nats::Client {
    let Ok(seed_path) = std::env::var("INSTAGRAM_ARCHIVE_TEST_NATS_ADMIN_NKEY_SEED_PATH") else {
        return fallback;
    };
    let url = std::env::var("INSTAGRAM_ARCHIVE_TEST_NATS_URL")
        .expect("an isolated JetStream endpoint is required");
    let seed = fs::read_to_string(seed_path).expect("the admin seed file is readable");
    async_nats::ConnectOptions::with_nkey(seed.trim().to_owned())
        .connect(url)
        .await
        .expect("the isolated admin identity connects")
}

#[expect(clippy::expect_used, reason = "integration fixture setup must succeed")]
async fn prepare_stream(client: async_nats::Client) -> jetstream::stream::Stream {
    let context = jetstream::new(client);
    let _ = context.delete_stream(STREAM).await;
    context
        .create_stream(jetstream::stream::Config {
            name: STREAM.to_owned(),
            subjects: vec!["evt.social.source.>".to_owned()],
            ..jetstream::stream::Config::default()
        })
        .await
        .expect("the isolated event stream is created")
}

async fn seed_captured(test: &TestDatabase) -> (Uuid, String) {
    seed_event(test, "social.source.captured.v1").await
}

#[expect(clippy::expect_used, reason = "integration fixture setup must succeed")]
async fn seed_event(test: &TestDatabase, event_type: &str) -> (Uuid, String) {
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
         (event_id, event_type, aggregate_type, aggregate_id, payload, occurred_at) \
         values ($1, $2, 'capture', $3, $4, now())",
    )
    .bind(event_id)
    .bind(event_type)
    .bind(aggregate_id)
    .bind(&payload)
    .execute(test.database.pool())
    .await
    .expect("the outbox row is seeded");
    let (stored_payload,): (serde_json::Value,) =
        sqlx::query_as("select payload from instagram_archive.outbox_events where event_id = $1")
            .bind(event_id)
            .fetch_one(test.database.pool())
            .await
            .expect("the stored JSONB payload is readable");
    (
        event_id,
        serde_json::to_string(&stored_payload).expect("the JSONB payload renders"),
    )
}

struct OpenBroker {
    container: String,
    client: async_nats::Client,
}

impl OpenBroker {
    #[expect(
        clippy::expect_used,
        clippy::panic,
        reason = "integration fixture: disposable broker setup must succeed"
    )]
    async fn start() -> Self {
        let id = Uuid::now_v7().simple().to_string();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port exists");
        let port = listener.local_addr().expect("the port is readable").port();
        drop(listener);
        let container = format!("ratatoskr-instagram-outbox-{id}");
        let output = Command::new("docker")
            .args([
                "run",
                "--detach",
                "--name",
                &container,
                "--publish",
                &format!("127.0.0.1:{port}:4222"),
                NATS_IMAGE,
                "-js",
            ])
            .output()
            .expect("docker starts the isolated broker");
        assert!(
            output.status.success(),
            "isolated broker failed to start: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let url = format!("nats://127.0.0.1:{port}");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            match async_nats::connect(&url).await {
                Ok(client) => return Self { container, client },
                Err(error) if tokio::time::Instant::now() < deadline => {
                    let _ = error;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => panic!("the isolated broker did not become ready: {error}"),
            }
        }
    }
}

impl Drop for OpenBroker {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "--force", &self.container])
            .output();
    }
}

struct RestrictedBroker {
    container: String,
    directory: std::path::PathBuf,
    url: String,
}

impl RestrictedBroker {
    #[expect(
        clippy::expect_used,
        clippy::panic,
        reason = "integration fixture: disposable broker setup must succeed"
    )]
    async fn start() -> Self {
        let id = Uuid::now_v7().simple().to_string();
        let directory = std::env::current_dir()
            .expect("the repository directory is readable")
            .join("target")
            .join(format!("instagram-nats-deny-{id}"));
        fs::create_dir_all(&directory).expect("the private fixture directory is created");
        let config_path = directory.join("nats.conf");
        fs::write(
            &config_path,
            r#"
listen: 0.0.0.0:4222
jetstream: { store_dir: "/tmp/jetstream" }
authorization {
  users: [
    { user: "admin", password: "admin", permissions: { publish: ">", subscribe: ">" } }
    { user: "instagram", password: "instagram", permissions: {
        publish: ["evt.social.source.updated.v1"], subscribe: ["_INBOX.>"]
    } }
  ]
}
"#,
        )
        .expect("the broker policy is written");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port exists");
        let port = listener.local_addr().expect("the port is readable").port();
        drop(listener);
        let container = format!("ratatoskr-instagram-deny-{id}");
        let output = Command::new("docker")
            .args([
                "run",
                "--detach",
                "--name",
                &container,
                "--publish",
                &format!("127.0.0.1:{port}:4222"),
                "--volume",
                &format!("{}:/etc/nats:ro", directory.display()),
                NATS_IMAGE,
                "-c",
                "/etc/nats/nats.conf",
            ])
            .output()
            .expect("docker starts the restricted broker");
        assert!(
            output.status.success(),
            "restricted broker failed to start: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let broker = Self {
            container,
            directory,
            url: format!("nats://127.0.0.1:{port}"),
        };
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if async_nats::ConnectOptions::with_user_and_password(
                "admin".to_owned(),
                "admin".to_owned(),
            )
            .connect(&broker.url)
            .await
            .is_ok()
            {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                let logs = Command::new("docker")
                    .args(["logs", &broker.container])
                    .output()
                    .expect("restricted broker logs are readable");
                panic!(
                    "restricted broker did not accept connections: {}{}",
                    String::from_utf8_lossy(&logs.stdout),
                    String::from_utf8_lossy(&logs.stderr)
                );
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        broker
    }

    #[expect(
        clippy::expect_used,
        reason = "timeout fixture must stop broker responses"
    )]
    fn pause(&self) {
        let output = Command::new("docker")
            .args(["pause", &self.container])
            .output()
            .expect("docker can pause the fixture");
        assert!(
            output.status.success(),
            "the broker fixture is paused: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[expect(
        clippy::expect_used,
        reason = "timeout fixture must resume for recovery"
    )]
    fn unpause(&self) {
        let output = Command::new("docker")
            .args(["unpause", &self.container])
            .output()
            .expect("docker can unpause the fixture");
        assert!(
            output.status.success(),
            "the broker fixture is resumed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

impl Drop for RestrictedBroker {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "--force", &self.container])
            .output();
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[tokio::test]
async fn publish_ack_marks_only_acknowledged_row() {
    let _guard = broker_lock().lock().await;
    let broker = OpenBroker::start().await;
    let client = broker.client.clone();
    let stream = prepare_stream(client.clone()).await;
    let test = TestDatabase::create().await.expect("a fresh test database");
    let (event_id, expected_body) = seed_captured(&test).await;
    let transport = JetStreamTransport::new(client, Duration::from_secs(5));

    let summary = run_once(test.database.pool(), &transport, 8)
        .await
        .expect("the publisher pass completes");

    assert_eq!(summary.delivered, 1, "an acknowledged row is delivered");
    assert_eq!(summary.failed, 0);
    let message = stream
        .get_raw_message(1)
        .await
        .expect("the acknowledged message is persisted");
    assert_eq!(message.subject.as_str(), "evt.social.source.captured.v1");
    assert_eq!(message.payload.as_ref(), expected_body.as_bytes());
    let (stored_id, published_at): (Uuid, Option<OffsetDateTime>) = sqlx::query_as(
        "select event_id, published_at from instagram_archive.outbox_events where event_id = $1",
    )
    .bind(event_id)
    .fetch_one(test.database.pool())
    .await
    .expect("the outbox mark is readable");
    assert_eq!(stored_id, event_id);
    assert!(
        published_at.is_some(),
        "only the acknowledged row is marked"
    );
    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn permission_denial_retains_row() {
    let _guard = broker_lock().lock().await;
    let broker = RestrictedBroker::start().await;
    let admin =
        async_nats::ConnectOptions::with_user_and_password("admin".to_owned(), "admin".to_owned())
            .connect(&broker.url)
            .await
            .expect("the admin connects");
    jetstream::new(admin)
        .create_stream(jetstream::stream::Config {
            name: STREAM.to_owned(),
            subjects: vec!["evt.social.source.>".to_owned()],
            ..jetstream::stream::Config::default()
        })
        .await
        .expect("the privileged fixture creates the event stream");
    let instagram = async_nats::ConnectOptions::with_user_and_password(
        "instagram".to_owned(),
        "instagram".to_owned(),
    )
    .connect(&broker.url)
    .await
    .expect("the restricted publisher connects");
    let test = TestDatabase::create().await.expect("a fresh test database");
    let (event_id, original_body) = seed_captured(&test).await;
    let transport = JetStreamTransport::new(instagram, Duration::from_millis(500));

    let summary = run_once(test.database.pool(), &transport, 8)
        .await
        .expect("the denied pass completes");

    assert_eq!(summary.delivered, 0, "a denied publish cannot be credited");
    assert_eq!(summary.failed, 1);
    let (payload, published_at, attempt_count): (serde_json::Value, Option<OffsetDateTime>, i32) =
        sqlx::query_as(
            "select payload, published_at, attempt_count \
             from instagram_archive.outbox_events where event_id = $1",
        )
        .bind(event_id)
        .fetch_one(test.database.pool())
        .await
        .expect("the denied row remains readable");
    assert_eq!(
        serde_json::to_string(&payload).expect("payload renders"),
        original_body,
        "a denial cannot mutate the envelope"
    );
    assert!(published_at.is_none(), "a denied row remains unpublished");
    assert_eq!(attempt_count, 1, "the denial records one failed attempt");
    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn ack_timeout_retains_identical_bytes() {
    let _guard = broker_lock().lock().await;
    let broker = RestrictedBroker::start().await;
    let client =
        async_nats::ConnectOptions::with_user_and_password("admin".to_owned(), "admin".to_owned())
            .connect(&broker.url)
            .await
            .expect("the acknowledged publisher connects before the pause");
    let stream = jetstream::new(client.clone())
        .create_stream(jetstream::stream::Config {
            name: STREAM.to_owned(),
            subjects: vec!["evt.social.source.>".to_owned()],
            ..jetstream::stream::Config::default()
        })
        .await
        .expect("the event stream is created before the pause");
    let test = TestDatabase::create().await.expect("a fresh test database");
    let (event_id, original_body) = seed_captured(&test).await;
    let timing_out = JetStreamTransport::new(client, Duration::from_millis(100));
    broker.pause();

    let failed = run_once(test.database.pool(), &timing_out, 8)
        .await
        .expect("the timed-out pass completes");
    broker.unpause();

    assert_eq!(failed.delivered, 0, "a timed-out ack cannot be credited");
    assert_eq!(failed.failed, 1);
    let (stored_id, payload, published_at): (Uuid, serde_json::Value, Option<OffsetDateTime>) =
        sqlx::query_as(
            "select event_id, payload, published_at \
             from instagram_archive.outbox_events where event_id = $1",
        )
        .bind(event_id)
        .fetch_one(test.database.pool())
        .await
        .expect("the timed-out row remains readable");
    assert_eq!(stored_id, event_id, "event identity remains stable");
    assert_eq!(
        serde_json::to_string(&payload).expect("payload renders"),
        original_body,
        "the timeout cannot mutate stored bytes"
    );
    assert!(published_at.is_none(), "the timed-out row is unpublished");

    sqlx::query(
        "update instagram_archive.outbox_events set next_attempt_at = now() where event_id = $1",
    )
    .bind(event_id)
    .execute(test.database.pool())
    .await
    .expect("the retry is made due");
    let recovering_client =
        async_nats::ConnectOptions::with_user_and_password("admin".to_owned(), "admin".to_owned())
            .connect(&broker.url)
            .await
            .expect("the recovering publisher reconnects");
    let recovering = JetStreamTransport::new(recovering_client, Duration::from_secs(5));
    let succeeded = run_once(test.database.pool(), &recovering, 8)
        .await
        .expect("the recovering pass completes");
    assert_eq!(succeeded.delivered, 1);
    let message = stream
        .get_raw_message(1)
        .await
        .expect("the recovered message is persisted");
    assert_eq!(message.payload.as_ref(), original_body.as_bytes());
    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn all_three_fact_types_use_exact_subjects() {
    let _guard = broker_lock().lock().await;
    let local_broker = if actual_policy_fixture_configured() {
        None
    } else {
        Some(OpenBroker::start().await)
    };
    let client = match local_broker.as_ref() {
        Some(local) => local.client.clone(),
        None => broker().await,
    };
    let stream = prepare_stream(admin_broker(client.clone()).await).await;
    let test = TestDatabase::create().await.expect("a fresh test database");
    let event_types = [
        "social.source.captured.v1",
        "social.source.updated.v1",
        "social.source.removed.v1",
    ];
    let mut expected = std::collections::BTreeMap::new();
    let mut event_ids = Vec::with_capacity(event_types.len());
    for event_type in event_types {
        let (event_id, body) = seed_event(&test, event_type).await;
        event_ids.push(event_id);
        expected.insert(format!("evt.{event_type}"), body);
    }
    let transport = JetStreamTransport::new(client, Duration::from_secs(5));

    let summary = run_once(test.database.pool(), &transport, 8)
        .await
        .expect("the three-fact pass completes");

    assert_eq!(summary.delivered, 3, "all owned facts are acknowledged");
    assert_eq!(summary.failed, 0);
    let (published_count,): (i64,) = sqlx::query_as(
        "select count(*) from instagram_archive.outbox_events \
         where event_id = any($1) and published_at is not null",
    )
    .bind(&event_ids)
    .fetch_one(test.database.pool())
    .await
    .expect("the publication marks are readable");
    assert_eq!(
        published_count, 3,
        "each row is marked only after its acknowledged delivery"
    );
    let mut observed = std::collections::BTreeMap::new();
    for sequence in 1..=3 {
        let message = stream
            .get_raw_message(sequence)
            .await
            .expect("each acknowledged message is persisted");
        observed.insert(
            message.subject.to_string(),
            String::from_utf8(message.payload.to_vec()).expect("the envelope is UTF-8"),
        );
    }
    assert_eq!(observed, expected, "subject and body mapping is exact");
    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
#[ignore = "requires the workspace actual Platform policy fixture"]
async fn actual_platform_policy_denies_foreign_publish_and_direct_subscription() {
    let _guard = broker_lock().lock().await;
    let publisher = broker().await;
    let admin = admin_broker(publisher.clone()).await;
    prepare_stream(admin).await;

    let foreign = tokio::time::timeout(Duration::from_millis(500), async {
        let acknowledgement = jetstream::new(publisher.clone())
            .publish("evt.platform.operation.reported.v1", b"{}".to_vec().into())
            .await;
        match acknowledgement {
            Ok(acknowledgement) => acknowledgement.await.map(|_| ()),
            Err(error) => Err(error),
        }
    })
    .await;
    assert!(
        !matches!(foreign, Ok(Ok(()))),
        "the Instagram identity must not receive a foreign event acknowledgement"
    );

    let mut subscription = publisher
        .subscribe("evt.>")
        .await
        .expect("the local client can issue the denied subscription request");
    jetstream::new(publisher)
        .publish(
            "evt.social.source.captured.v1",
            b"subscription-probe".to_vec().into(),
        )
        .await
        .expect("the Instagram identity sends the subscription probe")
        .await
        .expect("the subscription probe is persisted");
    assert!(
        tokio::time::timeout(Duration::from_millis(250), subscription.next())
            .await
            .is_err(),
        "the Instagram identity must not receive a direct event delivery"
    );
}
