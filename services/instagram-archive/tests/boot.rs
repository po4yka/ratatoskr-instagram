//! Process boot contract: the real binary starts against a disposable
//! database, serves the operator plane, validates configuration, and stops
//! cleanly on SIGTERM.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use async_nats::jetstream;
use ratatoskr_instagram_archive::Config;
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
            "RATATOSKR__PUBLIC_RESOLUTION__ACCESS_TOKEN_PATH",
            token_file(),
        )
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
        .env(
            "RATATOSKR__PUBLIC_RESOLUTION__ACCESS_TOKEN_PATH",
            token_file(),
        )
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
        .env(
            "RATATOSKR__PUBLIC_RESOLUTION__ACCESS_TOKEN_PATH",
            token_file(),
        )
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
            "RATATOSKR__PUBLIC_RESOLUTION__ACCESS_TOKEN_PATH",
            token_file(),
        )
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

/// An access-token file the booting service can read; its content is a synthetic placeholder.
#[expect(clippy::expect_used, reason = "boot-test helper; see free_port")]
fn token_file() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("instagram-boot-token-{}", std::process::id()));
    std::fs::write(&path, "boot-test-token\n").expect("the token file is written");
    path
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

/// The `KEY=VALUE` lines of a systemd environment file, comments and blank lines dropped.
#[expect(clippy::expect_used, reason = "boot-test helper; see free_port")]
fn environment_file_entries(path: &std::path::Path) -> Vec<(String, String)> {
    std::fs::read_to_string(path)
        .expect("the shipped example exists (XR-021 CONTRACTS.md R2-10)")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let (key, value) = line.split_once('=').expect("every line is KEY=VALUE");
            (key.to_owned(), value.to_owned())
        })
        .collect()
}

#[test]
fn the_shipped_example_loads() {
    let example = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../deploy/systemd/instagram.conf.example");
    let entries = environment_file_entries(&example);
    let value_of = |key: &str| {
        entries
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value.clone())
    };

    // The operator-facing values the example promises, as written.
    assert_eq!(
        value_of("RATATOSKR__ADMIN__LISTEN_ADDRESS").as_deref(),
        Some("127.0.0.1:9082")
    );
    assert_eq!(
        value_of("RATATOSKR__API__LISTEN_ADDRESS").as_deref(),
        Some("127.0.0.1:9083")
    );
    assert_eq!(
        value_of("RATATOSKR__BUS__URL").as_deref(),
        Some("nats://127.0.0.1:4222")
    );
    assert_eq!(
        value_of("RATATOSKR__BUS__NKEY_SEED_PATH").as_deref(),
        Some("/etc/ratatoskr/instagram.nkey")
    );
    assert!(
        value_of("RATATOSKR__PUBLIC_RESOLUTION__ENDPOINT")
            .is_some_and(|endpoint| endpoint.starts_with("https://graph.facebook.com/")),
        "the Meta oEmbed endpoint is stated"
    );

    // Loaded through the real loader, with temporary files in place of the absolute secret paths.
    let directory = std::env::temp_dir().join(format!("instagram-example-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    let substituted: Vec<(String, String)> = entries
        .iter()
        .map(|(key, value)| {
            if key.ends_with("_PATH") && value.starts_with('/') {
                let file = directory.join(key);
                std::fs::write(&file, "placeholder-secret\n").expect("a scratch secret file");
                (key.clone(), file.display().to_string())
            } else {
                (key.clone(), value.clone())
            }
        })
        .collect();
    let config = Config::from_environment(substituted.clone())
        .expect("the shipped example is a valid configuration");

    assert_eq!(config.admin.listen_address.to_string(), "127.0.0.1:9082");
    assert_eq!(config.api.listen_address.to_string(), "127.0.0.1:9083");
    let bus = config.bus.as_ref().expect("the example configures the bus");
    assert_eq!(bus.url, "nats://127.0.0.1:4222");
    assert_eq!(
        bus.nkey_seed_path.as_deref(),
        Some(directory.join("RATATOSKR__BUS__NKEY_SEED_PATH").as_path())
    );
    assert!(
        config
            .public_resolution
            .endpoint
            .starts_with("https://graph.facebook.com/"),
        "the Meta oEmbed endpoint is configured"
    );
    config
        .public_resolution
        .load_access_token()
        .expect("the access-token path names a readable token file");
    std::fs::remove_dir_all(&directory).expect("the scratch directory is removed");
}
