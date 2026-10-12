//! An unacknowledged removal is visible to an operator and invisible to readiness (XR-021
//! CONTRACTS.md R5-11 rule 4). The gauges are process-global, so this test binary holds only
//! this test and installs the only metrics recorder.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "the disposable database assertions are the integration contract"
)]

use std::time::Duration;

use metrics_exporter_prometheus::PrometheusBuilder;
use ratatoskr_instagram_archive::publishing::{
    AckOutcome, PassVerdict, REMOVAL_EVENT_TYPE, ResendPolicy, acknowledge, run_once,
    unacknowledged_removals,
};
use ratatoskr_instagram_archive::test_support::TestDatabase;
use ratatoskr_instagram_archive::{EventTransport, TransportError};
use serde_json::json;
use uuid::Uuid;

const POLICY: ResendPolicy = ResendPolicy::new(Duration::from_secs(30), Duration::from_secs(100));

/// Accepts every delivery.
struct Accepting;

impl EventTransport for Accepting {
    async fn deliver(&self, _event_id: Uuid, _envelope_json: &str) -> Result<(), TransportError> {
        Ok(())
    }
}

fn gauge(rendered: &str, name: &str) -> f64 {
    rendered
        .lines()
        .find(|line| line.starts_with(name))
        .and_then(|line| line.rsplit(' ').next())
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("the gauge {name} is rendered: {rendered}"))
}

#[tokio::test]
async fn an_unacknowledged_removal_is_not_unpublished_work() {
    let recorder = PrometheusBuilder::new()
        .install_recorder()
        .expect("this test binary installs the only recorder");
    let test = TestDatabase::create().await.expect("a disposable database");
    let owner = Uuid::now_v7();
    let removal = Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.outbox_events \
         (event_id, event_type, aggregate_type, aggregate_id, payload, occurred_at) \
         values ($1, $2, 'capture', $3, $4, now() - interval '120 seconds')",
    )
    .bind(removal)
    .bind(REMOVAL_EVENT_TYPE)
    .bind(Uuid::now_v7())
    .bind(json!({"event_id": removal.to_string(), "tenant_id": format!("user:{owner}")}))
    .execute(test.database.pool())
    .await
    .expect("the removal is inserted");

    let pass = run_once(test.database.pool(), &Accepting, 16, POLICY)
        .await
        .expect("the pass runs");

    assert_eq!(
        (pass.delivered, pass.remaining, pass.verdict),
        (1, 0, PassVerdict::Healthy)
    );
    let rendered = recorder.render();
    assert!(
        gauge(&rendered, "instagram_outbox_unpublished_depth") < 0.5,
        "an unacknowledged removal is not unpublished depth: {rendered}"
    );
    assert!(
        (gauge(&rendered, "instagram_unacknowledged_tombstones") - 1.0).abs() < 0.5,
        "{rendered}"
    );
    assert!(
        gauge(
            &rendered,
            "instagram_oldest_unacknowledged_tombstone_age_seconds"
        ) >= 119.0,
        "{rendered}"
    );
    let waiting = unacknowledged_removals(test.database.pool())
        .await
        .expect("the gauges' source answers");
    assert_eq!(waiting.count, 1);
    assert!(waiting.oldest_age_seconds >= 119.0);

    acknowledge(
        test.database.pool(),
        removal,
        &format!("user:{owner}"),
        AckOutcome::Applied,
    )
    .await
    .expect("the acknowledgement is recorded");
    run_once(test.database.pool(), &Accepting, 16, POLICY)
        .await
        .expect("the next pass runs");
    let rendered = recorder.render();
    assert!(
        gauge(&rendered, "instagram_unacknowledged_tombstones") < 0.5,
        "the gauge moves down once Knowledge answered: {rendered}"
    );
    assert!(
        gauge(
            &rendered,
            "instagram_oldest_unacknowledged_tombstone_age_seconds"
        ) < 0.5
    );
    test.cleanup().await.expect("cleanup");
}
