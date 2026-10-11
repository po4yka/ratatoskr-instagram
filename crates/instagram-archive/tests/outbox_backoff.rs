//! The outbox pass honours backoff so a failing head row never starves later rows
//! (XR-021 CONTRACTS.md S02 rule 3).

#![allow(
    clippy::expect_used,
    reason = "the disposable database assertions are the integration contract"
)]

use std::sync::Mutex;

use metrics_exporter_prometheus::PrometheusBuilder;
use ratatoskr_instagram_archive::publishing::{PassVerdict, run_once};
use ratatoskr_instagram_archive::test_support::TestDatabase;
use ratatoskr_instagram_archive::{EventTransport, TransportError, UndeliverableClass};
use serde_json::json;
use uuid::Uuid;

/// Delivers everything except one poisoned row, and remembers what it delivered.
struct PoisonedTransport {
    poisoned: Uuid,
    delivered: Mutex<Vec<Uuid>>,
}

impl EventTransport for PoisonedTransport {
    async fn deliver(&self, event_id: Uuid, _envelope_json: &str) -> Result<(), TransportError> {
        if event_id == self.poisoned {
            return Err(TransportError::Transient(
                "the carrier refused the fact".to_owned(),
            ));
        }
        self.delivered.lock().expect("the lock").push(event_id);
        Ok(())
    }
}

async fn insert_row(test: &TestDatabase, event_id: Uuid, occurred_offset_seconds: i32) {
    sqlx::query(
        "insert into instagram_archive.outbox_events \
         (event_id, event_type, aggregate_type, aggregate_id, payload, occurred_at) \
         values ($1, 'platform.operation.reported.v1', 'capture', $2, $3, \
                 now() + make_interval(secs => $4))",
    )
    .bind(event_id)
    .bind(Uuid::now_v7())
    .bind(json!({"event_id": event_id.to_string()}))
    .bind(f64::from(occurred_offset_seconds))
    .execute(test.database.pool())
    .await
    .expect("the outbox row is inserted");
}

#[tokio::test]
async fn failing_rows_do_not_starve_later_rows() {
    let test = TestDatabase::create().await.expect("a disposable database");
    // The head row is OLDER by occurrence but has the LARGER id, so ordering by id alone would
    // put the healthy row first and hide the starvation; ordering must follow occurrence.
    let healthy = Uuid::from_u128(0x0191_0000_0000_7000_8000_0000_0000_0001);
    let head = Uuid::from_u128(0x0191_0000_0000_7000_8000_0000_0000_0002);
    insert_row(&test, head, -10).await;
    insert_row(&test, healthy, 0).await;
    let transport = PoisonedTransport {
        poisoned: head,
        delivered: Mutex::new(Vec::new()),
    };

    let first = run_once(test.database.pool(), &transport, 1)
        .await
        .expect("the first pass runs");
    assert_eq!(
        (first.delivered, first.failed),
        (0, 1),
        "the head row fails"
    );
    let (attempts, backed_off, error): (i32, bool, Option<String>) = sqlx::query_as(
        "select attempt_count, next_attempt_at > now(), last_error \
         from instagram_archive.outbox_events where event_id = $1",
    )
    .bind(head)
    .fetch_one(test.database.pool())
    .await
    .expect("the head row is readable");
    assert_eq!(attempts, 1);
    assert!(backed_off, "the failure schedules a later attempt");
    assert!(
        error.is_some_and(|class| !class.is_empty()),
        "a safe error class is kept"
    );

    let second = run_once(test.database.pool(), &transport, 1)
        .await
        .expect("the second pass runs");
    assert_eq!(
        (second.delivered, second.failed),
        (1, 0),
        "the healthy row is delivered while the head row waits out its backoff"
    );
    assert_eq!(
        *transport.delivered.lock().expect("the lock"),
        vec![healthy]
    );
    let published: bool = sqlx::query_scalar(
        "select published_at is not null from instagram_archive.outbox_events \
         where event_id = $1",
    )
    .bind(head)
    .fetch_one(test.database.pool())
    .await
    .expect("the head row is readable");
    assert!(!published, "the failed row stays unpublished");
    test.cleanup()
        .await
        .expect("the disposable database is removed");
}

/// Refuses one row as too large for the carrier and delivers every other row.
struct OversizeTransport {
    oversize: Uuid,
    attempts: Mutex<Vec<Uuid>>,
}

impl EventTransport for OversizeTransport {
    async fn deliver(&self, event_id: Uuid, _envelope_json: &str) -> Result<(), TransportError> {
        self.attempts.lock().expect("the lock").push(event_id);
        if event_id == self.oversize {
            return Err(TransportError::Undeliverable(
                UndeliverableClass::PayloadTooLarge,
            ));
        }
        Ok(())
    }
}

#[tokio::test]
async fn an_oversize_envelope_is_undeliverable_and_later_rows_publish() {
    let recorder = PrometheusBuilder::new()
        .install_recorder()
        .expect("this test binary installs the only recorder");
    let test = TestDatabase::create().await.expect("a disposable database");
    let oversize = Uuid::from_u128(0x0191_0000_0000_7000_8000_0000_0000_0011);
    let healthy = Uuid::from_u128(0x0191_0000_0000_7000_8000_0000_0000_0012);
    insert_row(&test, oversize, -10).await;
    insert_row(&test, healthy, 0).await;
    let transport = OversizeTransport {
        oversize,
        attempts: Mutex::new(Vec::new()),
    };

    let pass = run_once(test.database.pool(), &transport, 16)
        .await
        .expect("the pass runs");

    assert_eq!(
        (
            pass.delivered,
            pass.failed,
            pass.undeliverable,
            pass.remaining
        ),
        (1, 0, 1, 0),
        "the healthy row publishes in the same pass and the undeliverable row leaves the queue"
    );
    assert_eq!(
        pass.verdict,
        PassVerdict::Healthy,
        "an undeliverable row never flips readiness"
    );
    let (undeliverable, class, published): (bool, Option<String>, bool) = sqlx::query_as(
        "select undeliverable_at is not null, last_error, published_at is not null \
         from instagram_archive.outbox_events where event_id = $1",
    )
    .bind(oversize)
    .fetch_one(test.database.pool())
    .await
    .expect("the oversize row is readable");
    assert!(undeliverable, "undeliverable_at is set at once");
    assert_eq!(class.as_deref(), Some("payload_too_large"));
    assert!(!published, "an undeliverable row is never marked published");
    assert!(
        recorder
            .render()
            .contains("instagram_outbox_undeliverable_total{class=\"payload_too_large\"} 1"),
        "the counter carries the class: {}",
        recorder.render()
    );

    let again = run_once(test.database.pool(), &transport, 16)
        .await
        .expect("the second pass runs");
    assert_eq!(
        (again.delivered, again.failed, again.undeliverable),
        (0, 0, 0)
    );
    assert_eq!(
        *transport.attempts.lock().expect("the lock"),
        vec![oversize, healthy],
        "the undeliverable row is attempted exactly once"
    );
    test.cleanup()
        .await
        .expect("the disposable database is removed");
}

async fn make_due(test: &TestDatabase, event_id: Uuid) {
    sqlx::query(
        "update instagram_archive.outbox_events set next_attempt_at = now() where event_id = $1",
    )
    .bind(event_id)
    .execute(test.database.pool())
    .await
    .expect("the row is made due");
}

#[tokio::test]
async fn a_failing_row_flips_the_verdict_from_its_third_attempt() {
    let test = TestDatabase::create().await.expect("a disposable database");
    let head = Uuid::from_u128(0x0191_0000_0000_7000_8000_0000_0000_0021);
    insert_row(&test, head, 0).await;
    let transport = PoisonedTransport {
        poisoned: head,
        delivered: Mutex::new(Vec::new()),
    };

    let mut verdicts = Vec::new();
    for _ in 0..3 {
        make_due(&test, head).await;
        let pass = run_once(test.database.pool(), &transport, 16)
            .await
            .expect("the pass runs");
        assert_eq!(pass.failed, 1);
        verdicts.push(pass.verdict);
    }
    assert_eq!(
        verdicts,
        [
            PassVerdict::Healthy,
            PassVerdict::Healthy,
            PassVerdict::Failing
        ],
        "two failed attempts are tolerated and the third reports a failing publisher"
    );

    let waiting = run_once(test.database.pool(), &transport, 16)
        .await
        .expect("the pass runs");
    assert_eq!(
        (waiting.failed, waiting.verdict),
        (0, PassVerdict::Waiting),
        "a pass that finds the stuck row in backoff keeps the previous verdict"
    );
    test.cleanup()
        .await
        .expect("the disposable database is removed");
}

#[tokio::test]
async fn a_row_older_than_five_minutes_that_fails_reports_a_failing_publisher() {
    let test = TestDatabase::create().await.expect("a disposable database");
    let head = Uuid::from_u128(0x0191_0000_0000_7000_8000_0000_0000_0031);
    insert_row(&test, head, -301).await;
    let transport = PoisonedTransport {
        poisoned: head,
        delivered: Mutex::new(Vec::new()),
    };

    let pass = run_once(test.database.pool(), &transport, 16)
        .await
        .expect("the pass runs");

    assert_eq!((pass.failed, pass.verdict), (1, PassVerdict::Failing));
    test.cleanup()
        .await
        .expect("the disposable database is removed");
}

#[tokio::test]
async fn a_clean_pass_after_failures_is_healthy() {
    let test = TestDatabase::create().await.expect("a disposable database");
    let head = Uuid::from_u128(0x0191_0000_0000_7000_8000_0000_0000_0041);
    insert_row(&test, head, -301).await;
    let failing = PoisonedTransport {
        poisoned: head,
        delivered: Mutex::new(Vec::new()),
    };
    run_once(test.database.pool(), &failing, 16)
        .await
        .expect("the failing pass runs");

    make_due(&test, head).await;
    let recovering = PoisonedTransport {
        poisoned: Uuid::nil(),
        delivered: Mutex::new(Vec::new()),
    };
    let pass = run_once(test.database.pool(), &recovering, 16)
        .await
        .expect("the clean pass runs");

    assert_eq!(
        (pass.delivered, pass.remaining, pass.verdict),
        (1, 0, PassVerdict::Healthy)
    );
    test.cleanup()
        .await
        .expect("the disposable database is removed");
}
