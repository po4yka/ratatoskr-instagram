//! The capture resolution worker: bounded retry and exactly-once terminal reports
//! (XR-021 CONTRACTS.md S10 CD2, CD7).

#![allow(
    clippy::expect_used,
    reason = "the disposable database assertions are the integration contract"
)]

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use ratatoskr_instagram_archive::capture_resolution::{
    CaptureResolver, ResolutionPolicy, ResolutionSummary, retry_delay,
};
use ratatoskr_instagram_archive::permalink::CanonicalPermalink;
use ratatoskr_instagram_archive::test_support::TestDatabase;
use ratatoskr_instagram_archive::{PublicSurface, SurfaceOutcome, source_identity};
use serde_json::{Value, json};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const REEL_FIXTURE: &str = include_str!("fixtures/oembed/reel_public.json");
const USER_ID: &str = "01991000-0000-7000-8000-000000000003";
const PERMALINK: &str = "https://www.instagram.com/p/Capture123/";
const POLICY: ResolutionPolicy = ResolutionPolicy {
    max_attempts: 5,
    batch_size: 8,
};

/// A surface that answers from a script and counts its fetches.
struct ScriptedSurface {
    script: Mutex<VecDeque<SurfaceOutcome>>,
    calls: AtomicUsize,
}

impl ScriptedSurface {
    fn new(script: impl IntoIterator<Item = SurfaceOutcome>) -> Self {
        Self {
            script: Mutex::new(script.into_iter().collect()),
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl PublicSurface for ScriptedSurface {
    async fn fetch(&self, _permalink: &CanonicalPermalink) -> SurfaceOutcome {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.script
            .lock()
            .expect("the script lock")
            .pop_front()
            .expect("the surface was fetched more often than scripted")
    }
}

fn payload() -> SurfaceOutcome {
    SurfaceOutcome::Payload {
        body: REEL_FIXTURE.to_owned(),
    }
}

/// The base instant of a test, safely after the intake clock (`next_resolution_at = now()`).
fn base() -> OffsetDateTime {
    OffsetDateTime::now_utc() + Duration::minutes(1)
}

fn command(sequence: u8, permalink: &str) -> Vec<u8> {
    let command_id = format!("01991000-0000-7000-8000-0000000001{sequence:02}");
    let operation_id = format!("01991000-0000-7000-8000-0000000002{sequence:02}");
    serde_json::to_vec(&json!({
        "command_id": command_id,
        "command_type": "social.capture.requested.v1",
        "issued_at": "2026-08-27T12:00:00Z",
        "producer": "ratatoskr-platform",
        "aggregate_id": format!("operation:{operation_id}"),
        "correlation_id": format!("operation:{operation_id}"),
        "tenant_id": format!("user:{USER_ID}"),
        "schema_version": 1,
        "payload": {
            "operation_id": operation_id,
            "idempotency_key": {
                "algorithm": "sha256",
                "hex": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            },
            "original_permalink": permalink,
            "captured_at": "2026-08-27T11:59:00Z",
            "provider": "instagram",
            "acquisition": "browser_extension",
            "saved_authority": "explicit_user_capture"
        }
    }))
    .expect("the fixed command serializes")
}

async fn ingest(test: &TestDatabase, sequence: u8) {
    test.database
        .ingest_browser_capture_command(
            "cmd.instagram.capture.requested.v1",
            &command(sequence, PERMALINK),
        )
        .await
        .expect("the command is accepted");
}

async fn run(
    test: &TestDatabase,
    surface: &ScriptedSurface,
    now: OffsetDateTime,
) -> ResolutionSummary {
    CaptureResolver::new(test.database.clone())
        .run_due_once(surface, now, POLICY)
        .await
        .expect("the pass runs")
}

/// Every `platform.operation.reported.v1` payload except the queued ones, oldest first.
async fn terminal_reports(test: &TestDatabase) -> Vec<Value> {
    sqlx::query_scalar(
        "select payload from instagram_archive.outbox_events \
         where event_type = 'platform.operation.reported.v1' \
           and payload #>> '{payload,status}' <> 'queued' order by occurred_at, event_id",
    )
    .fetch_all(test.database.pool())
    .await
    .expect("the report rows are readable")
}

async fn fact_count(test: &TestDatabase, event_type: &str) -> i64 {
    sqlx::query_scalar("select count(*) from instagram_archive.outbox_events where event_type = $1")
        .bind(event_type)
        .fetch_one(test.database.pool())
        .await
        .expect("the fact count answers")
}

async fn capture_state(test: &TestDatabase) -> (String, i32, Option<OffsetDateTime>) {
    sqlx::query_as(
        "select status, resolution_attempts, next_resolution_at \
         from instagram_archive.captures",
    )
    .fetch_one(test.database.pool())
    .await
    .expect("the capture is readable")
}

async fn unreported_operations(test: &TestDatabase) -> i64 {
    sqlx::query_scalar(
        "select count(*) from instagram_archive.capture_operations where reported_at is null",
    )
    .fetch_one(test.database.pool())
    .await
    .expect("the operation count answers")
}

async fn observations(test: &TestDatabase, kind: &str) -> i64 {
    sqlx::query_scalar(
        "select count(*) from instagram_archive.availability_observations where availability = $1",
    )
    .bind(kind)
    .fetch_one(test.database.pool())
    .await
    .expect("the observation count answers")
}

async fn cleanup(test: TestDatabase) {
    test.cleanup()
        .await
        .expect("the disposable database is removed");
}

#[test]
fn retry_delays_grow_by_four_and_cap_at_thirty_minutes() {
    let seconds: Vec<i64> = (1..=7).map(|n| retry_delay(n).whole_seconds()).collect();
    assert_eq!(seconds, [30, 120, 480, 1_800, 1_800, 1_800, 1_800]);
}

#[tokio::test]
async fn a_payload_resolves_the_capture_and_reports_success_once() {
    let test = TestDatabase::create().await.expect("a disposable database");
    ingest(&test, 1).await;
    let surface = ScriptedSurface::new([payload()]);

    let summary = run(&test, &surface, base()).await;

    assert_eq!((summary.fetched, summary.reported), (1, 1), "{summary:?}");
    assert_eq!(capture_state(&test).await.0, "resolved");
    assert_eq!(fact_count(&test, "social.source.captured.v1").await, 1);
    let reports = terminal_reports(&test).await;
    assert_eq!(reports.len(), 1, "exactly one terminal report");
    let report = &reports[0];
    assert_eq!(report["tenant_id"], format!("user:{USER_ID}"));
    assert_eq!(report["payload"]["status"], "succeeded");
    assert_eq!(report["payload"]["stage"], "capture_preserved");
    let source = source_identity(Uuid::parse_str(USER_ID).expect("fixed user"), PERMALINK);
    assert_eq!(
        report["payload"]["results"][0]["result_kind"],
        "social.post"
    );
    assert_eq!(
        report["payload"]["results"][0]["target"],
        format!("social_source:{source}")
    );
    let fact_tenant: String = sqlx::query_scalar(
        "select payload ->> 'tenant_id' from instagram_archive.outbox_events \
         where event_type = 'social.source.captured.v1'",
    )
    .fetch_one(test.database.pool())
    .await
    .expect("the fact is readable");
    assert_eq!(
        fact_tenant,
        format!("user:{USER_ID}"),
        "owner is the tenant"
    );
    assert_eq!(unreported_operations(&test).await, 0);

    let again = run(&test, &surface, base() + Duration::hours(1)).await;
    assert_eq!(again, ResolutionSummary::default(), "nothing is left to do");
    assert_eq!(surface.calls(), 1, "a concluded capture is never refetched");
    assert_eq!(terminal_reports(&test).await.len(), 1);
    cleanup(test).await;
}

#[tokio::test]
async fn a_deleted_post_is_reported_deleted_without_a_social_fact() {
    let test = TestDatabase::create().await.expect("a disposable database");
    ingest(&test, 1).await;
    let surface = ScriptedSurface::new([SurfaceOutcome::Deleted]);

    run(&test, &surface, base()).await;

    assert_eq!(capture_state(&test).await.0, "unavailable");
    let reports = terminal_reports(&test).await;
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0]["payload"]["status"], "failed");
    assert_eq!(reports[0]["payload"]["stage"], "capture_unavailable");
    assert_eq!(
        reports[0]["payload"]["error"]["code"],
        "social.source.deleted"
    );
    assert_eq!(reports[0]["payload"]["error"]["retryable"], false);
    assert_eq!(fact_count(&test, "social.source.captured.v1").await, 0);
    cleanup(test).await;
}

#[tokio::test]
async fn private_unsupported_and_unproven_failures_are_reported_inaccessible() {
    for outcome in [
        SurfaceOutcome::Private,
        SurfaceOutcome::Unsupported,
        SurfaceOutcome::Unavailable,
        SurfaceOutcome::Payload {
            body: "[1, 2]".to_owned(),
        },
    ] {
        let test = TestDatabase::create().await.expect("a disposable database");
        ingest(&test, 1).await;
        let surface = ScriptedSurface::new([outcome.clone()]);

        run(&test, &surface, base()).await;

        let reports = terminal_reports(&test).await;
        assert_eq!(reports.len(), 1, "{outcome:?}");
        assert_eq!(
            reports[0]["payload"]["error"]["code"], "social.source.unavailable",
            "{outcome:?}"
        );
        assert_eq!(
            reports[0]["payload"]["error"]["retryable"], false,
            "{outcome:?}"
        );
        assert_eq!(fact_count(&test, "social.source.captured.v1").await, 0);
        cleanup(test).await;
    }
}

#[tokio::test]
async fn four_transient_failures_reschedule_and_the_fifth_attempt_can_succeed() {
    let test = TestDatabase::create().await.expect("a disposable database");
    ingest(&test, 1).await;
    let surface = ScriptedSurface::new([
        SurfaceOutcome::TemporarilyUnavailable,
        SurfaceOutcome::TransportFailure,
        SurfaceOutcome::TemporarilyUnavailable,
        SurfaceOutcome::TemporarilyUnavailable,
        payload(),
    ]);
    let mut now = base();

    for (attempt, delay) in [30_i64, 120, 480, 1_800].into_iter().enumerate() {
        let summary = run(&test, &surface, now).await;
        assert_eq!((summary.fetched, summary.retried), (1, 1), "{summary:?}");
        let (status, attempts, next) = capture_state(&test).await;
        assert_eq!(status, "accepted");
        assert_eq!(attempts, i32::try_from(attempt + 1).expect("small"));
        let next = next.expect("a retry is scheduled");
        assert_eq!(
            next.unix_timestamp(),
            (now + Duration::seconds(delay)).unix_timestamp(),
            "attempt {} waits {delay} s",
            attempt + 1
        );
        assert!(terminal_reports(&test).await.is_empty(), "still unreported");
        assert_eq!(unreported_operations(&test).await, 1);

        let early = run(&test, &surface, now + Duration::seconds(delay - 1)).await;
        assert_eq!(early.claimed, 0, "not due before its backoff");
        now += Duration::seconds(delay);
    }
    assert_eq!(surface.calls(), 4);
    assert_eq!(observations(&test, "temporarily_unavailable").await, 0);

    let last = run(&test, &surface, now).await;

    assert_eq!((last.fetched, last.reported), (1, 1), "{last:?}");
    assert_eq!(capture_state(&test).await.0, "resolved");
    let reports = terminal_reports(&test).await;
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0]["payload"]["status"], "succeeded");
    cleanup(test).await;
}

#[tokio::test]
async fn five_transient_failures_end_in_one_retryable_failure() {
    let test = TestDatabase::create().await.expect("a disposable database");
    ingest(&test, 1).await;
    let surface = ScriptedSurface::new([
        SurfaceOutcome::TemporarilyUnavailable,
        SurfaceOutcome::TemporarilyUnavailable,
        SurfaceOutcome::TransportFailure,
        SurfaceOutcome::TemporarilyUnavailable,
        SurfaceOutcome::TransportFailure,
    ]);
    let mut now = base();
    for _ in 0..5 {
        run(&test, &surface, now).await;
        now += Duration::minutes(31);
    }

    let reports = terminal_reports(&test).await;
    assert_eq!(reports.len(), 1, "one terminal report");
    assert_eq!(reports[0]["payload"]["status"], "failed");
    assert_eq!(
        reports[0]["payload"]["error"]["code"],
        "social.source.unavailable"
    );
    assert_eq!(reports[0]["payload"]["error"]["retryable"], true);
    assert_eq!(capture_state(&test).await.0, "unavailable");
    assert_eq!(observations(&test, "temporarily_unavailable").await, 1);
    assert_eq!(observations(&test, "resolution_failed").await, 0);

    run(&test, &surface, now).await;
    assert_eq!(surface.calls(), 5, "no sixth fetch");
    assert_eq!(terminal_reports(&test).await.len(), 1);
    cleanup(test).await;
}

#[tokio::test]
async fn two_operations_on_one_capture_each_get_a_report_from_one_fetch() {
    let test = TestDatabase::create().await.expect("a disposable database");
    ingest(&test, 1).await;
    ingest(&test, 2).await;
    let surface = ScriptedSurface::new([payload()]);

    let summary = run(&test, &surface, base()).await;

    assert_eq!((summary.fetched, summary.reported), (1, 2), "{summary:?}");
    assert_eq!(surface.calls(), 1, "one capture, one fetch");
    let reports = terminal_reports(&test).await;
    assert_eq!(reports.len(), 2);
    let mut operations: Vec<&str> = reports
        .iter()
        .map(|report| report["payload"]["operation_id"].as_str().expect("an id"))
        .collect();
    operations.sort_unstable();
    operations.dedup();
    assert_eq!(operations.len(), 2, "each operation has its own report");
    assert_eq!(unreported_operations(&test).await, 0);
    cleanup(test).await;
}

#[tokio::test]
async fn a_late_operation_on_a_resolved_capture_is_reported_without_a_refetch() {
    let test = TestDatabase::create().await.expect("a disposable database");
    ingest(&test, 1).await;
    let surface = ScriptedSurface::new([payload()]);
    run(&test, &surface, base()).await;
    ingest(&test, 2).await;

    let summary = run(&test, &surface, base()).await;

    assert_eq!((summary.fetched, summary.reported), (0, 1), "{summary:?}");
    assert_eq!(surface.calls(), 1);
    let reports = terminal_reports(&test).await;
    assert_eq!(reports.len(), 2);
    assert!(
        reports
            .iter()
            .all(|report| report["payload"]["status"] == "succeeded")
    );
    cleanup(test).await;
}

#[tokio::test]
async fn a_resolved_capture_with_an_unreported_operation_only_reports() {
    let test = TestDatabase::create().await.expect("a disposable database");
    ingest(&test, 1).await;
    // The crash: the capture concluded, the process died before reporting.
    let capture_id: Uuid = sqlx::query_scalar("select capture_id from instagram_archive.captures")
        .fetch_one(test.database.pool())
        .await
        .expect("the capture exists");
    test.database
        .resolve_capture_permalink(capture_id, &ScriptedSurface::new([payload()]), base())
        .await
        .expect("the capture concludes");
    assert_eq!(unreported_operations(&test).await, 1);
    assert!(terminal_reports(&test).await.is_empty());
    let surface = ScriptedSurface::new([]);

    let summary = run(&test, &surface, base()).await;

    assert_eq!((summary.fetched, summary.reported), (0, 1), "{summary:?}");
    assert_eq!(surface.calls(), 0, "no second fetch");
    let reports = terminal_reports(&test).await;
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0]["payload"]["status"], "succeeded");
    assert_eq!(fact_count(&test, "social.source.captured.v1").await, 1);
    cleanup(test).await;
}

#[tokio::test]
async fn a_reopened_capture_is_fetched_again_for_its_new_operation() {
    let test = TestDatabase::create().await.expect("a disposable database");
    ingest(&test, 1).await;
    run(
        &test,
        &ScriptedSurface::new([SurfaceOutcome::Deleted]),
        base(),
    )
    .await;
    assert_eq!(capture_state(&test).await.0, "unavailable");
    ingest(&test, 2).await;
    assert_eq!(capture_state(&test).await.0, "accepted", "reopened");
    let surface = ScriptedSurface::new([payload()]);

    run(&test, &surface, base()).await;

    assert_eq!(surface.calls(), 1);
    let reports = terminal_reports(&test).await;
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0]["payload"]["status"], "failed");
    assert_eq!(reports[1]["payload"]["status"], "succeeded");
    cleanup(test).await;
}
