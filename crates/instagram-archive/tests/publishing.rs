//! Social-source publishing contract against disposable archive databases:
//! one truthful captured fact per first preservation, updated facts on
//! changed records, nothing for unavailable outcomes, notes that never reach
//! the wire, transactional appends, and byte-identical redelivery.
//!
//! The surface seam is replayed from committed fixtures; no test makes a live
//! call. A missing database server is a failure, never a skip.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use ratatoskr_event_envelope::EventEnvelope;
use ratatoskr_identifiers::Extensions;
use ratatoskr_instagram_archive::capture::{CaptureRequest, CaptureSubmission, ClientSource};
use ratatoskr_instagram_archive::permalink::CanonicalPermalink;
use ratatoskr_instagram_archive::publishing::{
    EventTransport, FactKind, PublishError, TransportError, run_once, source_identity,
};
use ratatoskr_instagram_archive::resolution::{PublicSurface, ResolutionOutcome, SurfaceOutcome};
use ratatoskr_instagram_archive::test_support::TestDatabase;
use ratatoskr_social_contracts::{
    AcquisitionMethod, CaptureCompleteness, RemovalReason, SavedAuthority,
    SocialSourceAnalysisCompleted, SocialSourceCaptured, SocialSourceRemoved, SocialSourceUpdated,
    UpstreamAvailability,
};
use time::OffsetDateTime;
use uuid::Uuid;

/// A synthetic, redacted oEmbed-style payload; no live call produced it.
const REEL_FIXTURE: &str = include_str!("fixtures/oembed/reel_public.json");

/// A later synthetic answer for the same permalink, changed upstream.
const REEL_FIXTURE_UPDATED: &str = include_str!("fixtures/oembed/reel_public_updated.json");

/// The canonical reel permalink the fixtures document.
const REEL_PERMALINK: &str = "https://www.instagram.com/reel/Csynthetic1/";

/// A surface that replays scripted outcomes and counts its fetches.
#[derive(Debug)]
struct FakePublicSurface {
    script: Mutex<VecDeque<SurfaceOutcome>>,
    fetches: AtomicUsize,
}

impl FakePublicSurface {
    fn answering(outcome: SurfaceOutcome) -> Self {
        Self {
            script: Mutex::new(VecDeque::from(vec![outcome])),
            fetches: AtomicUsize::new(0),
        }
    }
}

impl PublicSurface for FakePublicSurface {
    #[expect(
        clippy::expect_used,
        reason = "test fake: a poisoned script lock cannot be recovered"
    )]
    async fn fetch(&self, _permalink: &CanonicalPermalink) -> SurfaceOutcome {
        self.fetches.fetch_add(1, Ordering::Relaxed);
        let script = self.script.lock().expect("script lock");
        script
            .front()
            .expect("the script was checked non-empty")
            .clone()
    }
}

/// Submits one accepted capture through the real intake path.
#[expect(
    clippy::expect_used,
    reason = "integration-test helper: an intake refusal fails the test"
)]
async fn submitted_capture(
    database: &ratatoskr_instagram_archive::Database,
    user_ref: Uuid,
    client_source: ClientSource,
) -> Uuid {
    let submission = database
        .submit_capture(&CaptureRequest {
            user_ref,
            url: REEL_PERMALINK.to_owned(),
            captured_at: OffsetDateTime::UNIX_EPOCH,
            client_source,
            note: None,
            client_idempotency_key: None,
        })
        .await
        .expect("intake must accept a canonical reel permalink");
    match submission {
        CaptureSubmission::Created(record) | CaptureSubmission::Reused(record) => record.capture_id,
    }
}

/// Resolves one capture once through the real resolution path.
#[expect(
    clippy::expect_used,
    reason = "integration-test helper: an unexpected outcome fails the test"
)]
async fn resolve_once(
    test: &TestDatabase,
    capture_id: Uuid,
    surface: &impl PublicSurface,
    resolved_at: OffsetDateTime,
) -> ResolutionOutcome {
    test.database
        .resolve_capture_permalink(capture_id, surface, resolved_at)
        .await
        .expect("the resolution must answer")
}

/// Reads every social-source fact recorded for one capture.
#[expect(
    clippy::expect_used,
    reason = "integration-test helper: an unreadable outbox fails the test"
)]
async fn read_facts(test: &TestDatabase, capture_id: Uuid) -> Vec<String> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "select payload->>'event_type' from instagram_archive.outbox_events \
         where aggregate_type = 'capture' and aggregate_id = $1 \
         order by event_id",
    )
    .bind(capture_id)
    .fetch_all(test.database.pool())
    .await
    .expect("the outbox query must answer");
    rows.into_iter().map(|(event_type,)| event_type).collect()
}

/// Decodes the stored envelope of the single fact for a capture into its
/// typed social payload.
#[expect(
    clippy::expect_used,
    reason = "integration-test helper: a malformed fact fails the test"
)]
async fn decoded_captured_payload(
    test: &TestDatabase,
    capture_id: Uuid,
) -> (SocialSourceCaptured, String) {
    let row: (serde_json::Value,) = sqlx::query_as(
        "select payload from instagram_archive.outbox_events \
         where aggregate_type = 'capture' and aggregate_id = $1",
    )
    .bind(capture_id)
    .fetch_one(test.database.pool())
    .await
    .expect("exactly one fact must exist");
    let envelope = EventEnvelope::from_json(
        serde_json::to_vec(&row.0)
            .expect("the stored payload re-serializes")
            .as_slice(),
    )
    .expect("the stored payload must parse as an envelope");
    let payload = envelope
        .payload_as::<SocialSourceCaptured>()
        .expect("a captured fact must decode as captured");
    (payload, envelope.event_type.to_string())
}

/// Builds the typed Knowledge completion envelope that a consumer receives.
#[expect(
    clippy::expect_used,
    reason = "test helper: a valid canonical envelope is required by the contract"
)]
fn completion_envelope(payload: &SocialSourceAnalysisCompleted, event_id: Uuid) -> Vec<u8> {
    let owner = payload.owner.to_string();
    let template = serde_json::json!({
        "event_id": event_id.to_string(),
        "event_type": "knowledge.analysis.completed.v1",
        "occurred_at": payload.completed_at.to_wire(),
        "producer": "ratatoskr-knowledge",
        "aggregate_id": format!("social_source:{}", payload.social_source_id),
        "correlation_id": owner,
        "tenant_id": payload.owner.to_string(),
        "schema_version": 1,
        "payload": {}
    });
    let mut envelope = EventEnvelope::from_json(
        serde_json::to_vec(&template)
            .expect("template serializes")
            .as_slice(),
    )
    .expect("template parses");
    envelope.set_payload(payload).expect("typed payload sets");
    envelope
        .to_canonical_json()
        .expect("canonical envelope renders")
        .into_bytes()
}

// ---------------------------------------------------------------------------
// Captured emission
// ---------------------------------------------------------------------------

#[tokio::test]
async fn first_resolution_emits_single_captured_fact() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let capture_id = submitted_capture(
        &test.database,
        Uuid::now_v7(),
        ClientSource::IosShareExtension,
    )
    .await;
    let surface = FakePublicSurface::answering(SurfaceOutcome::Payload {
        body: REEL_FIXTURE.to_owned(),
    });

    resolve_once(&test, capture_id, &surface, OffsetDateTime::UNIX_EPOCH).await;

    let facts = read_facts(&test, capture_id).await;
    assert_eq!(
        facts.len(),
        1,
        "exactly one fact must exist after the first successful resolution"
    );
    assert_eq!(
        facts[0], "social.source.captured.v1",
        "the first preservation is a captured fact"
    );
    // The stored body must be a complete envelope whose type matches and
    // whose payload decodes typed.
    let (_, event_type) = decoded_captured_payload(&test, capture_id).await;
    assert_eq!(event_type, "social.source.captured.v1");
    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn replaying_the_same_resolution_stays_one_fact() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let owner = Uuid::now_v7();
    let surface = FakePublicSurface::answering(SurfaceOutcome::Payload {
        body: REEL_FIXTURE.to_owned(),
    });
    let first_id = submitted_capture(&test.database, owner, ClientSource::IosShareExtension).await;
    resolve_once(&test, first_id, &surface, OffsetDateTime::UNIX_EPOCH).await;

    // The same canonical URL delivered again for the same user reuses the
    // original capture untouched: no second capture, no new fact. (A genuine
    // re-resolution of the SAME capture is a content change and publishes
    // `updated`, covered by changed_revision_emits_updated_fact.)
    let reused = submitted_capture(&test.database, owner, ClientSource::IosShareExtension).await;
    assert_eq!(
        reused, first_id,
        "intake deduplicates on (user_ref, canonical_url)"
    );

    let facts = read_facts(&test, first_id).await;
    assert_eq!(facts.len(), 1, "reuse must not add a fact");
    assert_eq!(facts[0], "social.source.captured.v1");
    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn unavailable_outcome_emits_nothing() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let capture_id = submitted_capture(
        &test.database,
        Uuid::now_v7(),
        ClientSource::BrowserExtension,
    )
    .await;

    let outcome = resolve_once(
        &test,
        capture_id,
        &FakePublicSurface::answering(SurfaceOutcome::Private),
        OffsetDateTime::UNIX_EPOCH,
    )
    .await;
    assert!(
        matches!(outcome, ResolutionOutcome::Unavailable(_)),
        "a private answer must end as an unavailable fallback"
    );

    let facts = read_facts(&test, capture_id).await;
    assert!(facts.is_empty(), "an unavailable outcome publishes nothing");
    test.cleanup().await.expect("cleanup");
}

// ---------------------------------------------------------------------------
// Updated emission
// ---------------------------------------------------------------------------

#[tokio::test]
async fn changed_revision_emits_updated_fact() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let capture_id = submitted_capture(
        &test.database,
        Uuid::now_v7(),
        ClientSource::IosShareExtension,
    )
    .await;
    let first = FakePublicSurface::answering(SurfaceOutcome::Payload {
        body: REEL_FIXTURE.to_owned(),
    });
    let later = FakePublicSurface::answering(SurfaceOutcome::Payload {
        body: REEL_FIXTURE_UPDATED.to_owned(),
    });

    resolve_once(&test, capture_id, &first, OffsetDateTime::UNIX_EPOCH).await;
    resolve_once(
        &test,
        capture_id,
        &later,
        OffsetDateTime::UNIX_EPOCH
            .replace_minute(1)
            .expect("minute"),
    )
    .await;

    let facts = read_facts(&test, capture_id).await;
    assert_eq!(facts.len(), 2, "one captured plus one updated fact");
    assert_eq!(facts[0], "social.source.captured.v1");
    assert_eq!(facts[1], "social.source.updated.v1");
    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn deletion_observation_republishes_content_untouched() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let capture_id = submitted_capture(
        &test.database,
        Uuid::now_v7(),
        ClientSource::IosShareExtension,
    )
    .await;
    let preserved = FakePublicSurface::answering(SurfaceOutcome::Payload {
        body: REEL_FIXTURE.to_owned(),
    });
    resolve_once(&test, capture_id, &preserved, OffsetDateTime::UNIX_EPOCH).await;

    let outcome = resolve_once(
        &test,
        capture_id,
        &FakePublicSurface::answering(SurfaceOutcome::Deleted),
        OffsetDateTime::UNIX_EPOCH
            .replace_minute(2)
            .expect("minute"),
    )
    .await;
    assert!(matches!(outcome, ResolutionOutcome::Unavailable(_)));

    let facts = read_facts(&test, capture_id).await;
    assert_eq!(facts.len(), 2, "captured plus deleted-upstream updated");
    assert_eq!(facts[1], "social.source.updated.v1");

    let row: (serde_json::Value,) = sqlx::query_as(
        "select payload from instagram_archive.outbox_events \
         where aggregate_type = 'capture' and aggregate_id = $1 \
         order by event_id desc limit 1",
    )
    .bind(capture_id)
    .fetch_one(test.database.pool())
    .await
    .expect("the newest fact must exist");
    let envelope = EventEnvelope::from_json(
        serde_json::to_vec(&row.0)
            .expect("re-serializes")
            .as_slice(),
    )
    .expect("parses");
    let payload = envelope
        .payload_as::<SocialSourceUpdated>()
        .expect("an updated fact decodes as updated");
    assert_eq!(
        payload.source.upstream_availability,
        UpstreamAvailability::DeletedUpstream,
        "the deletion observation is republished verbatim"
    );
    assert!(
        payload.source.text.is_some(),
        "preserved content stays untouched by the deletion observation"
    );
    test.cleanup().await.expect("cleanup");
}

// ---------------------------------------------------------------------------
// Provenance truthfulness
// ---------------------------------------------------------------------------

#[tokio::test]
async fn snapshot_maps_resolved_capture_provenance_verbatim() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let owner = Uuid::now_v7();
    let capture_id =
        submitted_capture(&test.database, owner, ClientSource::IosShareExtension).await;
    let surface = FakePublicSurface::answering(SurfaceOutcome::Payload {
        body: REEL_FIXTURE.to_owned(),
    });
    resolve_once(&test, capture_id, &surface, OffsetDateTime::UNIX_EPOCH).await;

    let (payload, _) = decoded_captured_payload(&test, capture_id).await;
    let snapshot = payload.source;
    assert_eq!(snapshot.acquisition, AcquisitionMethod::ShareExtension);
    assert_eq!(
        snapshot.saved_authority,
        SavedAuthority::ExplicitUserCapture,
        "a share-style capture keeps the explicit ceiling"
    );
    assert_eq!(
        snapshot.completeness,
        CaptureCompleteness::Partial,
        "metadata-only captures declare partial completeness"
    );
    assert!(
        !snapshot.warnings.is_empty(),
        "partial completeness requires at least one warning naming the gap"
    );
    assert!(snapshot.media.is_empty());
    assert!(snapshot.raw_blob.is_some());
    assert_eq!(
        snapshot.upstream_availability,
        UpstreamAvailability::Available
    );
    assert!(
        snapshot.author.is_none(),
        "no author is observed by this lane; absence stays absent"
    );
    assert!(
        snapshot.published_at.is_none(),
        "publication time is never inferred from capture time"
    );
    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn snapshot_refuses_unknown_provenance_token() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let capture_id = submitted_capture(
        &test.database,
        Uuid::now_v7(),
        ClientSource::IosShareExtension,
    )
    .await;
    // This disposable database deliberately bypasses the production CHECK to
    // prove the read-side publisher still fails closed if historical damage
    // or a manual repair introduces an unknown token.
    sqlx::query(
        "alter table instagram_archive.captures \
         drop constraint captures_acquisition_method_check",
    )
    .execute(test.database.pool())
    .await
    .expect("the disposable schema permits the hostile row setup");
    sqlx::query(
        "update instagram_archive.captures set acquisition_method = 'carrier_pigeon' \
         where capture_id = $1",
    )
    .bind(capture_id)
    .execute(test.database.pool())
    .await
    .expect("the hostile provenance row is stored only in this test");

    let mut transaction = test.database.pool().begin().await.expect("a transaction");
    let error = ratatoskr_instagram_archive::publishing::append_fact(
        &mut transaction,
        FactKind::Captured,
        capture_id,
    )
    .await
    .expect_err("unknown provenance must prevent publication");
    assert!(
        matches!(error, PublishError::ContractViolation(_, _)),
        "unknown provenance is a contract violation: {error}"
    );
    drop(transaction);
    assert!(
        read_facts(&test, capture_id).await.is_empty(),
        "a refused snapshot leaves no outbox fact"
    );
    test.cleanup().await.expect("cleanup");
}

// ---------------------------------------------------------------------------
// Knowledge result linkage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn analysis_completion_links_matching_capture_once() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let owner = Uuid::now_v7();
    let capture_id =
        submitted_capture(&test.database, owner, ClientSource::IosShareExtension).await;
    resolve_once(
        &test,
        capture_id,
        &FakePublicSurface::answering(SurfaceOutcome::Payload {
            body: REEL_FIXTURE.to_owned(),
        }),
        OffsetDateTime::UNIX_EPOCH,
    )
    .await;
    let (captured, _) = decoded_captured_payload(&test, capture_id).await;
    let completion = SocialSourceAnalysisCompleted {
        owner: captured.source.owner,
        social_source_id: captured.source.social_source_id,
        content_digest: captured.source.content_digest.clone(),
        completed_at: captured.source.captured_at,
        extensions: Extensions::default(),
    };
    let event = completion_envelope(&completion, Uuid::now_v7());

    test.database
        .ingest_analysis_completed(&event)
        .await
        .expect("the matching completion must be accepted");
    test.database
        .ingest_analysis_completed(&event)
        .await
        .expect("the same delivery must be deduplicated");

    let links: (i64,) = sqlx::query_as(
        "select count(*) from instagram_archive.capture_analysis_links \
         where capture_id = $1",
    )
    .bind(capture_id)
    .fetch_one(test.database.pool())
    .await
    .expect("the linkage query answers");
    assert_eq!(links.0, 1, "one completion links exactly one capture");
    let inbox: (i64,) = sqlx::query_as(
        "select count(*) from instagram_archive.inbox_events \
         where consumer_name = 'ratatoskr-instagram-analysis'",
    )
    .fetch_one(test.database.pool())
    .await
    .expect("the inbox query answers");
    assert_eq!(inbox.0, 1, "replay must not create a second inbox record");
    test.cleanup().await.expect("cleanup");
}

// ---------------------------------------------------------------------------
// Local deletion propagation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tombstone_emits_removed_fact_once_and_blocks_late_completion() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let owner = Uuid::now_v7();
    let capture_id =
        submitted_capture(&test.database, owner, ClientSource::IosShareExtension).await;
    resolve_once(
        &test,
        capture_id,
        &FakePublicSurface::answering(SurfaceOutcome::Payload {
            body: REEL_FIXTURE.to_owned(),
        }),
        OffsetDateTime::UNIX_EPOCH,
    )
    .await;
    let (captured, _) = decoded_captured_payload(&test, capture_id).await;

    test.database
        .tombstone_capture(
            capture_id,
            RemovalReason::UserRequested,
            OffsetDateTime::UNIX_EPOCH
                .replace_minute(3)
                .expect("minute"),
        )
        .await
        .expect("the local deletion must be recorded");
    test.database
        .tombstone_capture(
            capture_id,
            RemovalReason::UserRequested,
            OffsetDateTime::UNIX_EPOCH
                .replace_minute(3)
                .expect("minute"),
        )
        .await
        .expect("the duplicate deletion must be idempotent");

    let facts = read_facts(&test, capture_id).await;
    assert_eq!(
        facts,
        ["social.source.captured.v1", "social.source.removed.v1"]
    );
    let removal: (serde_json::Value,) = sqlx::query_as(
        "select payload from instagram_archive.outbox_events \
         where aggregate_type = 'capture' and aggregate_id = $1 \
         and event_type = 'social.source.removed.v1'",
    )
    .bind(capture_id)
    .fetch_one(test.database.pool())
    .await
    .expect("the removal fact exists");
    let removal_envelope = EventEnvelope::from_json(
        serde_json::to_vec(&removal.0)
            .expect("the removal re-serializes")
            .as_slice(),
    )
    .expect("the removal parses");
    let removal_payload = removal_envelope
        .payload_as::<SocialSourceRemoved>()
        .expect("the removal payload decodes typed");
    assert_eq!(removal_payload.owner, captured.source.owner);
    assert_eq!(
        removal_payload.social_source_id,
        captured.source.social_source_id
    );
    assert_eq!(removal_payload.reason, RemovalReason::UserRequested);

    let late_completion = completion_envelope(
        &SocialSourceAnalysisCompleted {
            owner: captured.source.owner,
            social_source_id: captured.source.social_source_id,
            content_digest: captured.source.content_digest,
            completed_at: captured.source.captured_at,
            extensions: Extensions::default(),
        },
        Uuid::now_v7(),
    );
    let result = test
        .database
        .ingest_analysis_completed(&late_completion)
        .await
        .expect("a late completion remains a valid delivery");
    assert_eq!(
        result,
        ratatoskr_instagram_archive::AnalysisCompletionOutcome::Skipped,
        "a tombstone prevents the deleted capture from being re-linked"
    );
    let links: (i64,) = sqlx::query_as(
        "select count(*) from instagram_archive.capture_analysis_links \
         where capture_id = $1",
    )
    .bind(capture_id)
    .fetch_one(test.database.pool())
    .await
    .expect("the linkage query answers");
    assert_eq!(links.0, 0, "a late result cannot resurrect a tombstone");
    test.cleanup().await.expect("cleanup");
}

// ---------------------------------------------------------------------------
// Privacy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn note_never_reaches_the_wire() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let note = "private-composition-reminder-9f3a";
    let submission = test
        .database
        .submit_capture(&CaptureRequest {
            user_ref: Uuid::now_v7(),
            url: REEL_PERMALINK.to_owned(),
            captured_at: OffsetDateTime::UNIX_EPOCH,
            client_source: ClientSource::IosShareExtension,
            note: Some(note.to_owned()),
            client_idempotency_key: None,
        })
        .await
        .expect("intake accepts a noted capture");
    let capture_id = match submission {
        CaptureSubmission::Created(record) | CaptureSubmission::Reused(record) => record.capture_id,
    };
    let surface = FakePublicSurface::answering(SurfaceOutcome::Payload {
        body: REEL_FIXTURE.to_owned(),
    });
    resolve_once(&test, capture_id, &surface, OffsetDateTime::UNIX_EPOCH).await;

    let row: (serde_json::Value,) = sqlx::query_as(
        "select payload from instagram_archive.outbox_events \
         where aggregate_type = 'capture' and aggregate_id = $1",
    )
    .bind(capture_id)
    .fetch_one(test.database.pool())
    .await
    .expect("one fact exists");
    let rendered = serde_json::to_string(&row.0).expect("renders");
    assert!(
        !rendered.contains(note),
        "the user's note must never appear in a published payload"
    );
    test.cleanup().await.expect("cleanup");
}

// ---------------------------------------------------------------------------
// Transactionality and redelivery
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rollback_leaves_no_fact() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let capture_id = submitted_capture(
        &test.database,
        Uuid::now_v7(),
        ClientSource::IosShareExtension,
    )
    .await;
    let surface = FakePublicSurface::answering(SurfaceOutcome::Payload {
        body: REEL_FIXTURE.to_owned(),
    });
    resolve_once(&test, capture_id, &surface, OffsetDateTime::UNIX_EPOCH).await;

    // An append inside a transaction that never commits vanishes with it.
    let mut abandoned = test.database.pool().begin().await.expect("a txn");
    ratatoskr_instagram_archive::publishing::append_fact(
        &mut abandoned,
        FactKind::Updated,
        capture_id,
    )
    .await
    .expect("the append itself succeeds inside the open transaction");
    drop(abandoned);

    let facts = read_facts(&test, capture_id).await;
    assert_eq!(facts.len(), 1, "the rolled-back fact leaves no trace");
    test.cleanup().await.expect("cleanup");
}

/// A carrier that records every attempt's bytes and fails the first N calls.
struct FlakyTransport {
    failures_first: std::sync::atomic::AtomicUsize,
    attempts: Mutex<Vec<(Uuid, String)>>,
}

impl EventTransport for FlakyTransport {
    async fn deliver(&self, event_id: Uuid, envelope_json: &str) -> Result<(), TransportError> {
        // The attempt is logged before the outcome, so a failed attempt's
        // bytes are still comparable with the successful redelivery.
        let mut attempts = match self.attempts.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        attempts.push((event_id, envelope_json.to_owned()));
        if self.failures_first.fetch_sub(1, Ordering::SeqCst) > 1 {
            return Err(TransportError("carrier down".to_owned()));
        }
        Ok(())
    }
}

#[tokio::test]
async fn redelivery_is_byte_identical() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let capture_id = submitted_capture(
        &test.database,
        Uuid::now_v7(),
        ClientSource::IosShareExtension,
    )
    .await;
    let surface = FakePublicSurface::answering(SurfaceOutcome::Payload {
        body: REEL_FIXTURE.to_owned(),
    });
    resolve_once(&test, capture_id, &surface, OffsetDateTime::UNIX_EPOCH).await;

    let transport = FlakyTransport {
        failures_first: std::sync::atomic::AtomicUsize::new(2),
        attempts: Mutex::new(Vec::new()),
    };

    let failed = run_once(test.database.pool(), &transport, 8)
        .await
        .expect("the failing pass completes");
    assert_eq!(
        failed.delivered, 0,
        "nothing is marked published on failure"
    );
    assert_eq!(failed.failed, 1);

    let succeeded = run_once(test.database.pool(), &transport, 8)
        .await
        .expect("the recovering pass completes");
    assert_eq!(succeeded.delivered, 1);

    let attempts = {
        let guard = match transport.attempts.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.clone()
    };
    assert_eq!(attempts.len(), 2, "the fact was attempted exactly twice");
    assert_eq!(
        attempts[0], attempts[1],
        "a redelivery carries bytes identical to the first attempt"
    );

    let unpublished: (i64,) = sqlx::query_as(
        "select count(*) from instagram_archive.outbox_events where published_at is null",
    )
    .fetch_one(test.database.pool())
    .await
    .expect("depth query answers");
    assert_eq!(
        unpublished.0, 0,
        "the recovered pass marks the row published"
    );
    test.cleanup().await.expect("cleanup");
}

// ---------------------------------------------------------------------------
// Identity derivation
// ---------------------------------------------------------------------------

#[test]
fn identity_is_stable_per_owner_and_permalink() {
    let owner = Uuid::now_v7();
    let first = source_identity(owner, REEL_PERMALINK);
    assert_eq!(
        first,
        source_identity(owner, REEL_PERMALINK),
        "the same pair derives the same identity"
    );
    assert_ne!(
        first,
        source_identity(Uuid::now_v7(), REEL_PERMALINK),
        "another owner of the same URL derives another identity"
    );
    assert_ne!(
        first,
        source_identity(owner, "https://www.instagram.com/p/Cother123/"),
        "the same owner at another URL derives another identity"
    );
}
