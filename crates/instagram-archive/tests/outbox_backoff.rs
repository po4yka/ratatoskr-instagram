//! The outbox pass honours backoff so a failing head row never starves later rows
//! (XR-021 CONTRACTS.md S02 rule 3).

#![allow(
    clippy::expect_used,
    reason = "the disposable database assertions are the integration contract"
)]

use std::sync::Mutex;

use ratatoskr_instagram_archive::publishing::run_once;
use ratatoskr_instagram_archive::test_support::TestDatabase;
use ratatoskr_instagram_archive::{EventTransport, TransportError};
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
            return Err(TransportError("the carrier refused the fact".to_owned()));
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
