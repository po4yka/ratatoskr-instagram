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

#[cfg(unix)]
#[tokio::test]
async fn boots_serves_and_stops_cleanly_on_sigterm() {
    preprovision_browser_capture_consumer().await;
    let test = TestDatabase::create().await.expect("a prepared database");
    let url = test_url(test.name());
    let port = free_port();

    let mut child = spawn_service(&url, port);

    // Readiness arrives only after connect + schema apply + bind.
    let deadline = Instant::now() + READY_TIMEOUT;
    let mut ready_status = None;
    while Instant::now() < deadline {
        if let Some((status, _)) = http_get(port, "/health/ready") {
            ready_status = Some(status);
            if status == 200 {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
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
async fn preprovision_browser_capture_consumer() {
    let client = async_nats::connect(test_nats_url())
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
    let _: jetstream::consumer::PullConsumer = stream
        .create_consumer(jetstream::consumer::pull::Config {
            durable_name: Some("ratatoskr_instagram_browser_capture".to_owned()),
            filter_subject: "cmd.instagram.capture.requested.v1".to_owned(),
            ack_policy: jetstream::consumer::AckPolicy::Explicit,
            ..jetstream::consumer::pull::Config::default()
        })
        .await
        .expect("the privileged fixture preprovisions the fixed durable");
}
