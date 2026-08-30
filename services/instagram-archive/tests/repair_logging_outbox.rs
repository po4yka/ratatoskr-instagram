//! Closed operator CLI for the stopped-service logging-era outbox repair.

use std::process::Command;

use ratatoskr_instagram_archive::test_support::TestDatabase;
use uuid::Uuid;

const BIN: &str = env!("CARGO_BIN_EXE_ratatoskr-instagram-archive");
const CONFIRMATION: &str = "logging-transport-never-delivered";

fn test_url(name: &str) -> String {
    let base = ratatoskr_instagram_archive::test_support::admin_url();
    let (prefix, _) = base.rsplit_once('/').unwrap_or((base.as_str(), ""));
    format!("{prefix}/{name}")
}

#[expect(clippy::expect_used, reason = "CLI repair fixture setup must succeed")]
async fn seed_published(test: &TestDatabase) {
    let event_id = Uuid::now_v7();
    let aggregate_id = Uuid::now_v7();
    let owner = Uuid::now_v7();
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
         (event_id, event_type, aggregate_type, aggregate_id, payload, occurred_at, published_at) \
         values ($1, 'social.source.captured.v1', 'capture', $2, $3, now(), now())",
    )
    .bind(event_id)
    .bind(aggregate_id)
    .bind(payload)
    .execute(test.database.pool())
    .await
    .expect("the logging-era row is seeded");
}

#[test]
fn repair_requires_exact_confirmation() {
    for arguments in [
        vec!["repair-logging-outbox"],
        vec!["repair-logging-outbox", "--confirm", "yes"],
        vec![
            "repair-logging-outbox",
            "--confirm",
            CONFIRMATION,
            "unexpected",
        ],
    ] {
        let output = Command::new(BIN)
            .args(arguments)
            .env_remove("RATATOSKR__STORAGE__DATABASE_URL")
            .output()
            .expect("the operator command runs");
        assert_eq!(
            output.status.code(),
            Some(2),
            "only the exact confirmation grammar is accepted"
        );
        assert!(output.stdout.is_empty(), "grammar refusal prints no count");
    }
}

#[tokio::test]
async fn repair_prints_only_count() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    seed_published(&test).await;
    let database_url = test_url(test.name());

    let first = Command::new(BIN)
        .args(["repair-logging-outbox", "--confirm", CONFIRMATION])
        .env("RATATOSKR__STORAGE__DATABASE_URL", &database_url)
        .output()
        .expect("the confirmed repair runs");
    assert_eq!(first.status.code(), Some(0));
    assert_eq!(first.stdout, b"1\n");
    assert!(first.stderr.is_empty(), "success emits no diagnostics");

    let repeated = Command::new(BIN)
        .args(["repair-logging-outbox", "--confirm", CONFIRMATION])
        .env("RATATOSKR__STORAGE__DATABASE_URL", database_url)
        .output()
        .expect("the repeated repair runs");
    assert_eq!(repeated.status.code(), Some(0));
    assert_eq!(repeated.stdout, b"0\n");
    assert!(repeated.stderr.is_empty());
    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn repair_never_starts_network_or_http_planes() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    seed_published(&test).await;
    let admin = std::net::TcpListener::bind("127.0.0.1:0").expect("admin port is reserved");
    let api = std::net::TcpListener::bind("127.0.0.1:0").expect("API port is reserved");

    let output = Command::new(BIN)
        .args(["repair-logging-outbox", "--confirm", CONFIRMATION])
        .env("RATATOSKR__STORAGE__DATABASE_URL", test_url(test.name()))
        .env("RATATOSKR__BUS__URL", "nats://127.0.0.1:1")
        .env(
            "RATATOSKR__ADMIN__LISTEN_ADDRESS",
            admin.local_addr().expect("admin address").to_string(),
        )
        .env(
            "RATATOSKR__API__LISTEN_ADDRESS",
            api.local_addr().expect("API address").to_string(),
        )
        .output()
        .expect("the stopped-service repair runs");

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"1\n");
    assert!(output.stderr.is_empty());
    test.cleanup().await.expect("cleanup");
}
