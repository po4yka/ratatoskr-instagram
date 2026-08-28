//! Budgeted and privacy-safe public re-resolution job tests.

use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use ratatoskr_instagram_archive::privacy_deletion::{
    DeletionRequest, DeletionStore, DeletionTarget,
};
use ratatoskr_instagram_archive::re_resolution::{
    PriorResolutionState, ReResolutionAttemptOutcome, ReResolutionBudget, ReResolutionCandidate,
    ReResolutionSelection, ReResolutionSkipReason, RefreshAccounting, RefreshClassification,
    attempt_with_budget, claim_capture_for_resolution, classify_refresh, select_candidates,
};
use ratatoskr_instagram_archive::test_support::TestDatabase;

#[test]
fn selection_admits_only_due_recent_live_transient_or_resolved_captures() {
    let now = OffsetDateTime::now_utc();
    let resolved = Uuid::from_u128(1);
    let transient = Uuid::from_u128(2);
    let failed = Uuid::from_u128(3);
    let private = Uuid::from_u128(4);
    let deleted = Uuid::from_u128(5);
    let unsupported = Uuid::from_u128(6);
    let permanent = Uuid::from_u128(7);
    let old = Uuid::from_u128(8);
    let removed = Uuid::from_u128(9);
    let not_due = Uuid::from_u128(10);
    let candidate =
        |capture_id, due_seconds, age_days, prior_state, locally_removed| ReResolutionCandidate {
            capture_id,
            captured_at: now - Duration::days(age_days),
            next_resolution_at: now + Duration::seconds(due_seconds),
            locally_removed,
            prior_state,
        };
    let selection = select_candidates(
        vec![
            candidate(failed, -10, 1, PriorResolutionState::ResolverFailed, false),
            candidate(
                private,
                -9,
                1,
                PriorResolutionState::PrivateOrInaccessible,
                false,
            ),
            candidate(resolved, -30, 1, PriorResolutionState::Resolved, false),
            candidate(deleted, -8, 1, PriorResolutionState::Deleted, false),
            candidate(
                transient,
                -20,
                1,
                PriorResolutionState::TemporarilyUnavailable,
                false,
            ),
            candidate(unsupported, -7, 1, PriorResolutionState::Unsupported, false),
            candidate(
                permanent,
                -6,
                1,
                PriorResolutionState::PermanentlyUnavailable,
                false,
            ),
            candidate(old, -40, 31, PriorResolutionState::Resolved, false),
            candidate(removed, -5, 1, PriorResolutionState::Resolved, true),
            candidate(not_due, 60, 1, PriorResolutionState::Resolved, false),
        ],
        now,
        Duration::days(30),
    );

    assert_eq!(
        selection,
        ReResolutionSelection {
            admitted: vec![resolved, transient, failed],
            skipped: vec![
                (old, ReResolutionSkipReason::TooOld),
                (private, ReResolutionSkipReason::PrivacyTerminal),
                (deleted, ReResolutionSkipReason::PrivacyTerminal),
                (unsupported, ReResolutionSkipReason::Unsupported),
                (permanent, ReResolutionSkipReason::Unsupported),
                (removed, ReResolutionSkipReason::LocallyRemoved),
                (not_due, ReResolutionSkipReason::NotDue),
            ],
        }
    );
}

#[tokio::test]
async fn deletion_between_selection_and_claim_prevents_request_and_resurrection() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let capture_id = Uuid::now_v7();
    let owner = Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.captures \
         (capture_id, user_ref, canonical_url, acquisition_method, saved_authority, \
          client_source, status, captured_at, next_resolution_at) values ($1, $2, \
          'https://www.instagram.com/p/RERESRACE/', 'share_extension', \
          'explicit_user_capture', 'ios_share_extension', 'failed', now(), now())",
    )
    .bind(capture_id)
    .bind(owner)
    .execute(test.database.pool())
    .await
    .expect("selected capture stores");
    DeletionStore::new(&test.database)
        .apply(DeletionRequest {
            operation_id: Uuid::now_v7(),
            user_ref: owner,
            target: DeletionTarget::Capture(capture_id),
        })
        .await
        .expect("privacy deletion wins race");
    let now = OffsetDateTime::now_utc();
    let mut budget = available_budget(now);
    let before = budget;
    let mut resolver_calls = 0_u32;

    let outcome = claim_capture_for_resolution(
        &test.database,
        owner,
        capture_id,
        &mut budget,
        512,
        now,
        Duration::days(30),
        || resolver_calls += 1,
    )
    .await
    .expect("claim recheck answers");
    let durable: (i64, i64) = sqlx::query_as(
        "select \
           (select count(*) from instagram_archive.media_revisions), \
           (select count(*) from instagram_archive.outbox_events \
            where event_type <> 'social.source.removed.v1')",
    )
    .fetch_one(test.database.pool())
    .await
    .expect("durable race state reads");

    assert_eq!(
        outcome,
        ReResolutionAttemptOutcome::Skipped(ReResolutionSkipReason::LocallyRemoved)
    );
    assert_eq!(resolver_calls, 0);
    assert_eq!(budget, before);
    assert_eq!(durable, (0, 0));

    test.cleanup().await.expect("cleanup must drop");
}

#[test]
fn unchanged_refresh_does_not_publish_a_duplicate_update() {
    let digest = [0x44_u8; 32];
    assert_eq!(
        classify_refresh(&digest, &digest),
        RefreshAccounting {
            classification: RefreshClassification::Unchanged,
            evidence_appended: true,
            update_emitted: false,
        }
    );
}

fn available_budget(now: OffsetDateTime) -> ReResolutionBudget {
    ReResolutionBudget {
        max_items: 2,
        items_admitted: 0,
        max_requests: 2,
        requests_reserved: 0,
        max_response_bytes: 2048,
        response_bytes: 0,
        max_concurrency: 1,
        in_flight: 0,
        deadline_at: now + Duration::minutes(1),
        endpoint_remaining: Some(2),
    }
}

#[test]
fn request_never_starts_when_any_run_or_provider_budget_guard_is_exhausted() {
    let now = OffsetDateTime::now_utc();
    let cases = [
        (
            ReResolutionSkipReason::ItemBudget,
            ReResolutionBudget {
                max_items: 0,
                ..available_budget(now)
            },
        ),
        (
            ReResolutionSkipReason::ItemBudget,
            ReResolutionBudget {
                items_admitted: 2,
                ..available_budget(now)
            },
        ),
        (
            ReResolutionSkipReason::RequestBudget,
            ReResolutionBudget {
                requests_reserved: 2,
                ..available_budget(now)
            },
        ),
        (
            ReResolutionSkipReason::ByteBudget,
            ReResolutionBudget {
                response_bytes: 1537,
                ..available_budget(now)
            },
        ),
        (
            ReResolutionSkipReason::Deadline,
            ReResolutionBudget {
                deadline_at: now - Duration::seconds(1),
                ..available_budget(now)
            },
        ),
        (
            ReResolutionSkipReason::Concurrency,
            ReResolutionBudget {
                in_flight: 1,
                ..available_budget(now)
            },
        ),
        (
            ReResolutionSkipReason::ProviderBudget,
            ReResolutionBudget {
                endpoint_remaining: None,
                ..available_budget(now)
            },
        ),
        (
            ReResolutionSkipReason::ProviderBudget,
            ReResolutionBudget {
                endpoint_remaining: Some(0),
                ..available_budget(now)
            },
        ),
    ];

    for (expected, mut budget) in cases {
        let before = budget;
        let mut resolver_calls = 0_u32;
        let outcome = attempt_with_budget(&mut budget, 512, now, || resolver_calls += 1);
        assert_eq!(outcome, ReResolutionAttemptOutcome::Skipped(expected));
        assert_eq!(
            resolver_calls, 0,
            "guard {expected:?} must refuse before I/O"
        );
        assert_eq!(budget, before, "guard {expected:?} reserves no counter");
    }
}
