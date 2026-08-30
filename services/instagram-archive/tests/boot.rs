//! Process boot contract: the real binary starts against a disposable
//! database, serves the operator plane, validates configuration, and stops
//! cleanly on SIGTERM.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use async_nats::jetstream;
use ratatoskr_instagram_archive::test_support::TestDatabase;

const BIN: &str = env!("CARGO_BIN_EXE_ratatoskr-instagram-archive");
const READY_TIMEOUT: Duration = Duration::from_mins(1);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(20);

/// Reserves a free loopback port for the operator listener.
#[expect(
    clippy::expect_used,
    reason = "boot-test helper: an unreservable port is the failure under test"
)]
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free port exists");
    let port = listener.local_addr().expect("a bound address").port();
    drop(listener);
    port
}

/// One minimal HTTP/1.1 GET over raw TCP, closing after one response.
fn http_get(port: u16, path: &str) -> Option<(u16, String)> {
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    let status_line = response.lines().next()?;
    let status = status_line.split_whitespace().nth(1)?.parse::<u16>().ok()?;
    Some((status, response))
}

#[expect(clippy::expect_used, reason = "boot-test helper; see free_port")]
fn spawn_service(database_url: &str, admin_port: u16) -> Child {
    Command::new(BIN)
        .env("RATATOSKR__STORAGE__DATABASE_URL", database_url)
        .env("RATATOSKR__BUS__URL", test_nats_url())
        .env("RATATOSKR__PUBLISHER__POLL_INTERVAL_MS", "25")
        .env(
            "RATATOSKR__ADMIN__LISTEN_ADDRESS",
            format!("127.0.0.1:{admin_port}"),
        )
        .env(
            "RATATOSKR__API__LISTEN_ADDRESS",
            format!("127.0.0.1:{}", free_port()),
        )
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the service binary spawns")
}

#[expect(clippy::expect_used, reason = "boot-test helper; see spawn_service")]
fn spawn_service_without_bus(database_url: &str, admin_port: u16) -> Child {
    Command::new(BIN)
        .env("RATATOSKR__STORAGE__DATABASE_URL", database_url)
        .env_remove("RATATOSKR__BUS__URL")
        .env_remove("RATATOSKR__BUS__NKEY_SEED_PATH")
        .env("RATATOSKR__PUBLISHER__POLL_INTERVAL_MS", "25")
        .env(
            "RATATOSKR__ADMIN__LISTEN_ADDRESS",
            format!("127.0.0.1:{admin_port}"),
        )
        .env(
            "RATATOSKR__API__LISTEN_ADDRESS",
            format!("127.0.0.1:{}", free_port()),
        )
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the standalone service binary spawns")
}

fn wait_until_ready(port: u16) -> Option<u16> {
    let deadline = Instant::now() + READY_TIMEOUT;
    let mut ready_status = None;
    while Instant::now() < deadline {
        if let Some((status, _)) = http_get(port, "/health/ready") {
            ready_status = Some(status);
            if status == 200 {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    ready_status
}

#[expect(clippy::expect_used, reason = "boot fixture outbox setup must succeed")]
async fn seed_outbox(test: &TestDatabase) -> uuid::Uuid {
    let event_id = uuid::Uuid::now_v7();
    let aggregate_id = uuid::Uuid::now_v7();
    let owner = uuid::Uuid::now_v7();
    let payload = serde_json::json!({
        "event_id": event_id,
        "event_type": "social.source.captured.v1",
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
         values ($1, 'social.source.captured.v1', 'capture', $2, $3, now())",
    )
    .bind(event_id)
    .bind(aggregate_id)
    .bind(payload)
    .execute(test.database.pool())
    .await
    .expect("the boot outbox row is seeded");
    event_id
}

#[cfg(unix)]
#[tokio::test]
async fn boots_serves_and_stops_cleanly_on_sigterm() {
    preprovision_browser_capture_consumer().await;
    let test = TestDatabase::create().await.expect("a prepared database");
    let url = test_url(test.name());
    let port = free_port();

    let mut child = spawn_service(&url, port);

    // Readiness arrives only after connect + schema apply + bind.
    let ready_status = wait_until_ready(port);
    assert_eq!(
        ready_status,
        Some(200),
        "readiness did not arrive within {READY_TIMEOUT:?}"
    );

    let (live_status, live_body) = http_get(port, "/health/live").expect("live answers");
    assert_eq!(live_status, 200);
    assert!(live_body.contains("live"));

    let (_, metrics_body) = http_get(port, "/metrics").expect("metrics answers");
    assert!(
        metrics_body.contains("instagram_build_info"),
        "build info must be exported: {metrics_body}"
    );

    let (_, version_body) = http_get(port, "/version").expect("version answers");
    assert!(
        version_body.contains("ratatoskr-instagram-archive"),
        "{version_body}"
    );

    let (unknown_status, _) = http_get(port, "/definitely/not/here").expect("404 answers");
    assert_eq!(unknown_status, 404);

    send_sigterm(&child);
    let exited = wait_with_timeout(&mut child, SHUTDOWN_TIMEOUT).expect("no spawn error");
    assert_eq!(
        exited,
        Some(0),
        "SIGTERM must produce a clean exit within the shutdown bound"
    );

    test.cleanup().await.expect("cleanup drops");
}

#[cfg(unix)]
#[tokio::test]
async fn configured_bus_wires_consumer_and_publisher_before_readiness() {
    let stream = preprovision_browser_capture_consumer().await;
    let test = TestDatabase::create().await.expect("a prepared database");
    let event_id = seed_outbox(&test).await;
    let port = free_port();
    let mut child = spawn_service(&test_url(test.name()), port);

    assert_eq!(
        wait_until_ready(port),
        Some(200),
        "startup must reach ready"
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let message = loop {
        if let Ok(message) = stream.get_raw_message(1).await {
            break message;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "readiness was exposed without a working publisher"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert_eq!(message.subject.as_str(), "evt.social.source.captured.v1");
    let (_, metrics) = http_get(port, "/metrics").expect("configured metrics answer");
    assert!(
        metrics.contains("instagram_broker_delivery_enabled 1"),
        "configured broker state must be explicit: {metrics}"
    );
    let (published_at,): (Option<time::OffsetDateTime>,) = sqlx::query_as(
        "select published_at from instagram_archive.outbox_events where event_id = $1",
    )
    .bind(event_id)
    .fetch_one(test.database.pool())
    .await
    .expect("the publisher mark is readable");
    assert!(published_at.is_some(), "the acknowledged row is marked");

    send_sigterm(&child);
    assert_eq!(
        wait_with_timeout(&mut child, SHUTDOWN_TIMEOUT).expect("wait succeeds"),
        Some(0)
    );
    test.cleanup().await.expect("cleanup drops");
}

#[cfg(unix)]
#[tokio::test]
async fn missing_bus_starts_no_success_transport_and_changes_no_outbox_row() {
    let test = TestDatabase::create().await.expect("a prepared database");
    let event_id = seed_outbox(&test).await;
    let port = free_port();
    let mut child = spawn_service_without_bus(&test_url(test.name()), port);

    assert_eq!(
        wait_until_ready(port),
        Some(200),
        "standalone startup must remain available"
    );
    let (_, metrics) = http_get(port, "/metrics").expect("standalone metrics answer");
    assert!(
        metrics.contains("instagram_broker_delivery_enabled 0"),
        "disabled broker state must be explicit: {metrics}"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (published_at, attempt_count): (Option<time::OffsetDateTime>, i32) = sqlx::query_as(
        "select published_at, attempt_count \
         from instagram_archive.outbox_events where event_id = $1",
    )
    .bind(event_id)
    .fetch_one(test.database.pool())
    .await
    .expect("the standalone outbox row remains readable");
    assert!(published_at.is_none(), "no bus means no success transport");
    assert_eq!(attempt_count, 0, "no publisher means no attempted delivery");

    send_sigterm(&child);
    assert_eq!(
        wait_with_timeout(&mut child, SHUTDOWN_TIMEOUT).expect("wait succeeds"),
        Some(0)
    );
    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn check_config_accepts_valid_configuration_without_binding() {
    let test = TestDatabase::create().await.expect("a prepared database");
    let output = Command::new(BIN)
        .arg("check-config")
        .env("RATATOSKR__STORAGE__DATABASE_URL", test_url(test.name()))
        .env("RATATOSKR__BUS__URL", test_nats_url())
        .output()
        .expect("check-config runs");

    assert_eq!(output.status.code(), Some(0));
    let rendered = String::from_utf8_lossy(&output.stderr);
    assert!(rendered.contains("configuration is valid"), "{rendered}");

    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn check_config_refuses_invalid_configuration_without_echoing_values() {
    let output = Command::new(BIN)
        .arg("check-config")
        .env("RATATOSKR__ADMIN__LISTEN_ADDRESS", "10.9.8.7:9082")
        .env("RATATOSKR__LIMITS__SHUTDOWN_TIMEOUT_MS", "0")
        .env("RATATOSKR__BUS__URL", test_nats_url())
        .output()
        .expect("check-config runs");

    assert_eq!(
        output.status.code(),
        Some(78),
        "invalid configuration is EX_CONFIG"
    );
    let rendered = String::from_utf8_lossy(&output.stderr);
    assert!(rendered.contains("RATATOSKR__ADMIN__LISTEN_ADDRESS"));
    assert!(rendered.contains("RATATOSKR__LIMITS__SHUTDOWN_TIMEOUT_MS"));
    assert!(!rendered.contains("10.9.8.7"), "values never render");
}

#[tokio::test]
async fn missing_database_url_refuses_startup() {
    let port = free_port();
    let mut child = Command::new(BIN)
        .env("RATATOSKR__BUS__URL", test_nats_url())
        .env(
            "RATATOSKR__ADMIN__LISTEN_ADDRESS",
            format!("127.0.0.1:{port}"),
        )
        .env_remove("RATATOSKR__STORAGE__DATABASE_URL")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the service binary spawns");

    let exited = wait_with_timeout(&mut child, READY_TIMEOUT).expect("no spawn error");
    assert_ne!(
        exited,
        Some(0),
        "a process without its database must refuse"
    );
    let _ = child.kill();
}

#[cfg(unix)]
#[expect(clippy::expect_used, reason = "boot-test helper; see free_port")]
fn send_sigterm(child: &Child) {
    Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .output()
        .expect("SIGTERM is deliverable");
}

fn wait_with_timeout(child: &mut Child, limit: Duration) -> std::io::Result<Option<i32>> {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status.code());
        }
        if Instant::now() > deadline {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn test_url(name: &str) -> String {
    let base = ratatoskr_instagram_archive::test_support::admin_url();
    let (prefix, _) = base.rsplit_once('/').unwrap_or((base.as_str(), ""));
    format!("{prefix}/{name}")
}

#[expect(
    clippy::disallowed_methods,
    clippy::expect_used,
    reason = "the integration binary must use the explicitly isolated test broker"
)]
fn test_nats_url() -> String {
    std::env::var("INSTAGRAM_ARCHIVE_TEST_NATS_URL")
        .expect("an isolated JetStream endpoint is required")
}

#[expect(
    clippy::expect_used,
    reason = "the isolated broker fixture is part of the boot contract"
)]
async fn preprovision_browser_capture_consumer() -> jetstream::stream::Stream {
    let client = async_nats::connect(test_nats_url())
        .await
        .expect("the isolated broker connects");
    let context = jetstream::new(client);
    let _ = context.delete_stream("ratatoskr_commands").await;
    let _ = context
        .delete_stream("ratatoskr_social_events_boot_test")
        .await;
    let _ = context
        .delete_stream("ratatoskr_instagram_outbox_test")
        .await;
    let command_stream = context
        .create_stream(jetstream::stream::Config {
            name: "ratatoskr_commands".to_owned(),
            subjects: vec!["cmd.>".to_owned()],
            ..jetstream::stream::Config::default()
        })
        .await
        .expect("the privileged fixture creates the command stream");
    let _: jetstream::consumer::PullConsumer = command_stream
        .create_consumer(jetstream::consumer::pull::Config {
            durable_name: Some("ratatoskr_instagram_browser_capture".to_owned()),
            filter_subject: "cmd.instagram.capture.requested.v1".to_owned(),
            ack_policy: jetstream::consumer::AckPolicy::Explicit,
            ..jetstream::consumer::pull::Config::default()
        })
        .await
        .expect("the privileged fixture preprovisions the fixed durable");
    context
        .create_stream(jetstream::stream::Config {
            name: "ratatoskr_social_events_boot_test".to_owned(),
            subjects: vec!["evt.social.source.>".to_owned()],
            ..jetstream::stream::Config::default()
        })
        .await
        .expect("the privileged fixture creates the event stream")
}
