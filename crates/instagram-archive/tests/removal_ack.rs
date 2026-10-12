//! A `social.source.removed.v1` fact is kept and published again until Knowledge acknowledges it
//! (XR-021 CONTRACTS.md R5-11 rule 4): the state machine of the outbox row and the resend pass,
//! against a disposable `PostgreSQL` database and a recording transport.

#![allow(
    clippy::expect_used,
    reason = "the disposable database assertions are the integration contract"
)]

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ratatoskr_instagram_archive::privacy_deletion::{
    DeletionRequest, DeletionStore, DeletionTarget,
};
use ratatoskr_instagram_archive::publishing::{
    AckOutcome, PassVerdict, REMOVAL_EVENT_TYPE, ResendPolicy, acknowledge, run_once,
};
use ratatoskr_instagram_archive::test_support::TestDatabase;
use ratatoskr_instagram_archive::{EventTransport, TransportError};
use serde_json::json;
use uuid::Uuid;

/// The delay and cap every test uses: the 30 second configuration minimum and a 100 second cap.
const POLICY: ResendPolicy = ResendPolicy::new(Duration::from_secs(30), Duration::from_secs(100));

/// Records every delivery and fails them all while `refusing` is set.
#[derive(Default)]
struct RecordingTransport {
    deliveries: Mutex<Vec<(Uuid, String)>>,
    refusing: AtomicBool,
}

impl RecordingTransport {
    fn refusing() -> Self {
        let transport = Self::default();
        transport.refusing.store(true, Ordering::Release);
        transport
    }

    fn deliveries(&self) -> Vec<(Uuid, String)> {
        self.deliveries.lock().expect("the lock").clone()
    }
}

impl EventTransport for RecordingTransport {
    async fn deliver(&self, event_id: Uuid, envelope_json: &str) -> Result<(), TransportError> {
        if self.refusing.load(Ordering::Acquire) {
            return Err(TransportError::Transient("the broker refused".to_owned()));
        }
        self.deliveries
            .lock()
            .expect("the lock")
            .push((event_id, envelope_json.to_owned()));
        Ok(())
    }
}

fn tenant(owner: Uuid) -> String {
    format!("user:{owner}")
}

/// Inserts one unpublished outbox row of `event_type` whose envelope names `owner` as its tenant.
async fn insert_row(test: &TestDatabase, event_id: Uuid, owner: Uuid, event_type: &str) {
    sqlx::query(
        "insert into instagram_archive.outbox_events \
         (event_id, event_type, aggregate_type, aggregate_id, payload, occurred_at) \
         values ($1, $2, 'capture', $3, $4, now())",
    )
    .bind(event_id)
    .bind(event_type)
    .bind(Uuid::now_v7())
    .bind(json!({
        "event_id": event_id.to_string(),
        "event_type": event_type,
        "tenant_id": tenant(owner),
        "payload": {"marker": "the stored bytes"}
    }))
    .execute(test.database.pool())
    .await
    .expect("the outbox row is inserted");
}

async fn make_resend_due(test: &TestDatabase, event_id: Uuid) {
    sqlx::query(
        "update instagram_archive.outbox_events \
         set next_resend_at = now() - interval '1 second' where event_id = $1",
    )
    .bind(event_id)
    .execute(test.database.pool())
    .await
    .expect("the resend is made due");
}

/// A published removal that Knowledge has not acknowledged and whose first resend is due.
async fn published_due_removal(test: &TestDatabase, event_id: Uuid, owner: Uuid) {
    insert_row(test, event_id, owner, REMOVAL_EVENT_TYPE).await;
    let transport = RecordingTransport::default();
    run_once(test.database.pool(), &transport, 16, POLICY)
        .await
        .expect("the publishing pass runs");
    make_resend_due(test, event_id).await;
}

/// `(resend_count, seconds until next_resend_at, acknowledged, outcome)` of one row.
async fn schedule(test: &TestDatabase, event_id: Uuid) -> (i32, Option<f64>, bool, Option<String>) {
    sqlx::query_as(
        "select resend_count, \
                extract(epoch from next_resend_at - now())::float8, \
                acknowledged_at is not null, ack_outcome \
         from instagram_archive.outbox_events where event_id = $1",
    )
    .bind(event_id)
    .fetch_one(test.database.pool())
    .await
    .expect("the row is readable")
}

#[tokio::test]
async fn publishing_a_removal_arms_the_first_resend() {
    let test = TestDatabase::create().await.expect("a disposable database");
    let owner = Uuid::now_v7();
    let removal = Uuid::now_v7();
    let report = Uuid::now_v7();
    insert_row(&test, removal, owner, REMOVAL_EVENT_TYPE).await;
    insert_row(&test, report, owner, "platform.operation.reported.v1").await;

    let pass = run_once(
        test.database.pool(),
        &RecordingTransport::default(),
        16,
        POLICY,
    )
    .await
    .expect("the pass runs");
    assert_eq!(pass.delivered, 2);

    let armed: (bool,) = sqlx::query_as(
        "select next_resend_at = published_at + interval '30 seconds' \
         from instagram_archive.outbox_events where event_id = $1",
    )
    .bind(removal)
    .fetch_one(test.database.pool())
    .await
    .expect("the removal is readable");
    assert!(
        armed.0,
        "the first resend is due one delay after the first copy"
    );
    let other: (Option<f64>,) = sqlx::query_as(
        "select extract(epoch from next_resend_at)::float8 \
         from instagram_archive.outbox_events where event_id = $1",
    )
    .bind(report)
    .fetch_one(test.database.pool())
    .await
    .expect("the report is readable");
    assert_eq!(other.0, None, "only a removal is armed for a resend");
    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn a_due_removal_is_published_again_and_the_delay_doubles_to_the_cap() {
    let test = TestDatabase::create().await.expect("a disposable database");
    let owner = Uuid::now_v7();
    let removal = Uuid::now_v7();
    insert_row(&test, removal, owner, REMOVAL_EVENT_TYPE).await;
    let transport = RecordingTransport::default();
    run_once(test.database.pool(), &transport, 16, POLICY)
        .await
        .expect("the first pass publishes");
    let (published_at,): (time::OffsetDateTime,) = sqlx::query_as(
        "select published_at from instagram_archive.outbox_events where event_id = $1",
    )
    .bind(removal)
    .fetch_one(test.database.pool())
    .await
    .expect("the row is readable");

    let early = run_once(test.database.pool(), &transport, 16, POLICY)
        .await
        .expect("the early pass runs");
    assert_eq!(
        early.resent, 0,
        "nothing is due before the delay has passed"
    );
    assert_eq!(transport.deliveries().len(), 1);

    // After the first copy the delays are 30 (the old count 0), 60, then 100 (the cap, not 120).
    for (resent_so_far, expected_wait) in [(1, 30.0), (2, 60.0), (3, 100.0)] {
        make_resend_due(&test, removal).await;
        let pass = run_once(test.database.pool(), &transport, 16, POLICY)
            .await
            .expect("the resend pass runs");
        assert_eq!((pass.resent, pass.resend_failed, pass.delivered), (1, 0, 0));
        let (count, wait, acknowledged, _) = schedule(&test, removal).await;
        assert_eq!(count, resent_so_far);
        assert!(!acknowledged);
        let wait = wait.expect("the next resend is scheduled");
        assert!(
            (expected_wait - 2.0..=expected_wait).contains(&wait),
            "resend {resent_so_far} is followed by a wait of {expected_wait} s, saw {wait}"
        );
    }

    let deliveries = transport.deliveries();
    assert_eq!(deliveries.len(), 4, "the first copy and three resends");
    assert!(
        deliveries.iter().all(|delivery| delivery == &deliveries[0]),
        "every copy carries the same event id and the same stored bytes"
    );
    assert_eq!(deliveries[0].0, removal);
    let (still_published,): (time::OffsetDateTime,) = sqlx::query_as(
        "select published_at from instagram_archive.outbox_events where event_id = $1",
    )
    .bind(removal)
    .fetch_one(test.database.pool())
    .await
    .expect("the row is readable");
    assert_eq!(
        still_published, published_at,
        "a resend never moves published_at"
    );
    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn an_applied_or_refused_acknowledgement_ends_the_resend() {
    let test = TestDatabase::create().await.expect("a disposable database");
    let owner = Uuid::now_v7();
    let applied = Uuid::now_v7();
    let refused = Uuid::now_v7();
    published_due_removal(&test, applied, owner).await;
    published_due_removal(&test, refused, owner).await;

    assert!(
        acknowledge(
            test.database.pool(),
            applied,
            &tenant(owner),
            AckOutcome::Applied
        )
        .await
        .expect("the acknowledgement is recorded")
    );
    assert!(
        acknowledge(
            test.database.pool(),
            refused,
            &tenant(owner),
            AckOutcome::Refused
        )
        .await
        .expect("the refusal is recorded")
    );
    assert_eq!(
        (
            schedule(&test, applied).await.2,
            schedule(&test, applied).await.3
        ),
        (true, Some("applied".to_owned()))
    );
    assert_eq!(
        (
            schedule(&test, refused).await.2,
            schedule(&test, refused).await.3
        ),
        (true, Some("refused".to_owned()))
    );

    let transport = RecordingTransport::default();
    let pass = run_once(test.database.pool(), &transport, 16, POLICY)
        .await
        .expect("the pass runs");
    assert_eq!(
        pass.resent, 0,
        "an acknowledged removal is never published again"
    );
    assert!(transport.deliveries().is_empty());

    assert!(
        !acknowledge(
            test.database.pool(),
            applied,
            &tenant(owner),
            AckOutcome::Refused
        )
        .await
        .expect("a duplicate acknowledgement is accepted"),
        "a second acknowledgement of one row changes nothing"
    );
    assert_eq!(schedule(&test, applied).await.3, Some("applied".to_owned()));
    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn an_acknowledgement_for_another_owner_or_an_unknown_id_changes_nothing() {
    let test = TestDatabase::create().await.expect("a disposable database");
    let owner = Uuid::now_v7();
    let removal = Uuid::now_v7();
    let report = Uuid::now_v7();
    published_due_removal(&test, removal, owner).await;
    insert_row(&test, report, owner, "platform.operation.reported.v1").await;

    for (id, tenant_ref) in [
        (removal, tenant(Uuid::now_v7())),
        (Uuid::now_v7(), tenant(owner)),
        (report, tenant(owner)),
    ] {
        assert!(
            !acknowledge(test.database.pool(), id, &tenant_ref, AckOutcome::Applied)
                .await
                .expect("the acknowledgement is accepted and ignored")
        );
    }
    assert!(
        !schedule(&test, removal).await.2,
        "the removal is still open"
    );
    assert!(
        !schedule(&test, report).await.2,
        "a row that is not a removal is never acknowledged"
    );
    assert!(
        acknowledge(
            test.database.pool(),
            removal,
            &tenant(owner),
            AckOutcome::Applied
        )
        .await
        .expect("the owner's acknowledgement is recorded"),
        "the row was open and only the owner's acknowledgement closes it"
    );
    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn a_failed_republish_backs_off_without_touching_the_schedule() {
    let test = TestDatabase::create().await.expect("a disposable database");
    let owner = Uuid::now_v7();
    let removal = Uuid::now_v7();
    published_due_removal(&test, removal, owner).await;
    let before = schedule(&test, removal).await;

    let transport = RecordingTransport::refusing();
    let pass = run_once(test.database.pool(), &transport, 16, POLICY)
        .await
        .expect("the pass runs");

    assert_eq!(
        (pass.resent, pass.resend_failed, pass.failed, pass.verdict),
        (0, 1, 0, PassVerdict::Healthy),
        "a refused republish is counted on its own and never flips the verdict"
    );
    let (count, _, acknowledged, _) = schedule(&test, removal).await;
    assert_eq!((count, acknowledged), (before.0, false));
    let (published, attempts, backed_off): (bool, i32, bool) = sqlx::query_as(
        "select published_at is not null, attempt_count, next_attempt_at > now() \
         from instagram_archive.outbox_events where event_id = $1",
    )
    .bind(removal)
    .fetch_one(test.database.pool())
    .await
    .expect("the row is readable");
    assert!(published, "a failed republish leaves the row published");
    assert_eq!(attempts, 1);
    assert!(backed_off, "the existing failure backoff applies");

    let again = run_once(
        test.database.pool(),
        &RecordingTransport::default(),
        16,
        POLICY,
    )
    .await
    .expect("the next pass runs");
    assert_eq!(
        again.resent, 0,
        "the backoff keeps the row out of the next pass"
    );
    test.cleanup().await.expect("cleanup");
}

/// Inserts a row with explicit acknowledgement columns and returns the database's refusal.
async fn insert_with_ack_columns(
    test: &TestDatabase,
    event_type: &str,
    acknowledged: bool,
    outcome: Option<&str>,
) -> Option<String> {
    let result = sqlx::query(
        "insert into instagram_archive.outbox_events \
         (event_id, event_type, aggregate_type, aggregate_id, payload, occurred_at, \
          acknowledged_at, ack_outcome) \
         values ($1, $2, 'capture', $3, '{}', now(), \
                 case when $4 then now() end, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(event_type)
    .bind(Uuid::now_v7())
    .bind(acknowledged)
    .bind(outcome)
    .execute(test.database.pool())
    .await;
    result.err().map(|error| error.to_string())
}

#[tokio::test]
async fn the_database_refuses_acknowledgement_state_on_the_wrong_rows() {
    let test = TestDatabase::create().await.expect("a disposable database");

    let other_type = insert_with_ack_columns(
        &test,
        "platform.operation.reported.v1",
        true,
        Some("applied"),
    )
    .await
    .expect("another event type cannot carry an acknowledgement");
    assert!(
        other_type.contains("outbox_events_resend_removal_only_check"),
        "{other_type}"
    );

    let unpaired = insert_with_ack_columns(&test, REMOVAL_EVENT_TYPE, false, Some("applied"))
        .await
        .expect("an outcome needs an acknowledgement");
    assert!(
        unpaired.contains("outbox_events_ack_pair_check"),
        "{unpaired}"
    );
    let unknown = insert_with_ack_columns(&test, REMOVAL_EVENT_TYPE, true, Some("ignored"))
        .await
        .expect("the outcome vocabulary is closed");
    assert!(
        unknown.contains("outbox_events_ack_outcome_check"),
        "{unknown}"
    );

    assert_eq!(
        insert_with_ack_columns(&test, REMOVAL_EVENT_TYPE, true, Some("refused")).await,
        None,
        "a removal may carry a complete acknowledgement"
    );
    test.cleanup().await.expect("cleanup");
}

/// Inserts a published row of `event_type` for one aggregate, acknowledged or not.
async fn insert_aggregate_row(
    test: &TestDatabase,
    aggregate_type: &str,
    aggregate_id: Uuid,
    event_type: &str,
    acknowledged: bool,
) -> Uuid {
    let event_id = Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.outbox_events \
         (event_id, event_type, aggregate_type, aggregate_id, payload, occurred_at, \
          published_at, acknowledged_at, ack_outcome, next_resend_at) \
         values ($1, $2, $3, $4, '{}', now(), now(), \
                 case when $5 then now() end, case when $5 then 'applied' end, \
                 case when $2 = 'social.source.removed.v1' then now() end)",
    )
    .bind(event_id)
    .bind(event_type)
    .bind(aggregate_type)
    .bind(aggregate_id)
    .bind(acknowledged)
    .execute(test.database.pool())
    .await
    .expect("the aggregate row is inserted");
    event_id
}

async fn surviving(test: &TestDatabase, aggregate_id: Uuid, ids: &[Uuid]) -> Vec<bool> {
    let mut found = Vec::new();
    for id in ids {
        let exists: bool = sqlx::query_scalar(
            "select exists(select 1 from instagram_archive.outbox_events \
             where event_id = $1 and aggregate_id = $2)",
        )
        .bind(id)
        .bind(aggregate_id)
        .fetch_one(test.database.pool())
        .await
        .expect("the row lookup answers");
        found.push(exists);
    }
    found
}

#[tokio::test]
async fn a_later_privacy_deletion_keeps_the_unacknowledged_removal() {
    let test = TestDatabase::create().await.expect("a disposable database");
    let pool = test.database.pool();
    let owner = Uuid::now_v7();
    let store = DeletionStore::new(&test.database);

    // The capture path: the owner's last capture of a source.
    let capture_id = Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.captures \
         (capture_id, user_ref, canonical_url, acquisition_method, saved_authority, \
          client_source, status, captured_at) \
         values ($1, $2, 'https://www.instagram.com/p/KEEPREM1/', 'share_extension', \
          'explicit_user_capture', 'ios_share_extension', 'accepted', now())",
    )
    .bind(capture_id)
    .bind(owner)
    .execute(pool)
    .await
    .expect("the capture is stored");
    let rows = [
        insert_aggregate_row(
            &test,
            "capture",
            capture_id,
            "social.source.captured.v1",
            false,
        )
        .await,
        insert_aggregate_row(&test, "capture", capture_id, REMOVAL_EVENT_TYPE, true).await,
        insert_aggregate_row(&test, "capture", capture_id, REMOVAL_EVENT_TYPE, false).await,
    ];
    store
        .apply(DeletionRequest {
            operation_id: Uuid::now_v7(),
            user_ref: owner,
            target: DeletionTarget::Capture(capture_id),
        })
        .await
        .expect("the capture deletion succeeds");
    assert_eq!(
        surviving(&test, capture_id, &rows).await,
        [false, false, true],
        "the fact and the answered removal go; the unacknowledged removal stays"
    );
    let removals: i64 = sqlx::query_scalar(
        "select count(*) from instagram_archive.outbox_events \
         where aggregate_id = $1 and event_type = $2 and acknowledged_at is null",
    )
    .bind(capture_id)
    .bind(REMOVAL_EVENT_TYPE)
    .fetch_one(pool)
    .await
    .expect("the removal count answers");
    assert_eq!(
        removals, 2,
        "the old unacknowledged removal and the new one"
    );

    // The media path: a connection's own media.
    let account_id = Uuid::now_v7();
    let media_id = Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.accounts \
         (account_id, user_ref, provider_account_id, username, account_type, connection_status, \
          scopes, connected_at) values ($1, $2, 'provider-keep-removal', 'redacted', \
          'business', 'connected', array['instagram_business_basic'], now())",
    )
    .bind(account_id)
    .bind(owner)
    .execute(pool)
    .await
    .expect("the account is stored");
    sqlx::query(
        "insert into instagram_archive.media \
         (media_id, account_id, provider_media_id, permalink, media_type, acquisition_method, \
          saved_authority, upstream_status) values ($1, $2, 'keep-removal-1', \
          'https://www.instagram.com/p/KEEPREM2/', 'image', 'official_api', \
          'authoritative_platform_state', 'available')",
    )
    .bind(media_id)
    .bind(account_id)
    .execute(pool)
    .await
    .expect("the media is stored");
    let rows = [
        insert_aggregate_row(&test, "media", media_id, "social.source.updated.v1", false).await,
        insert_aggregate_row(&test, "media", media_id, REMOVAL_EVENT_TYPE, true).await,
        insert_aggregate_row(&test, "media", media_id, REMOVAL_EVENT_TYPE, false).await,
    ];
    store
        .apply(DeletionRequest {
            operation_id: Uuid::now_v7(),
            user_ref: owner,
            target: DeletionTarget::Connection(account_id),
        })
        .await
        .expect("the connection deletion succeeds");
    assert_eq!(
        surviving(&test, media_id, &rows).await,
        [false, false, true],
        "the same rule holds on the own-media path"
    );
    test.cleanup().await.expect("cleanup");
}
