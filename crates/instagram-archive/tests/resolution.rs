//! Public-resolution contract against disposable archive databases: raw
//! revisions are immutable and parser-versioned, re-resolution appends, the
//! normalized record is deterministic and truthfully provenanced, and
//! unsupported or failed outcomes fabricate nothing.
//!
//! The surface seam is replayed from committed fixtures; no test makes a live
//! call. A missing database server is a failure, never a skip.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use ratatoskr_instagram_archive::AvailabilityObservationKind;
use ratatoskr_instagram_archive::capture::{CaptureRequest, CaptureSubmission, ClientSource};
use ratatoskr_instagram_archive::permalink::CanonicalPermalink;
use ratatoskr_instagram_archive::resolution::{
    OEMBED_PARSER_VERSION, PublicSurface, ResolutionOutcome, SurfaceOutcome,
};
use ratatoskr_instagram_archive::test_support::TestDatabase;
use sha2::Digest as _;
use time::OffsetDateTime;
use uuid::Uuid;

/// A synthetic, redacted oEmbed-style payload; no live call produced it.
const REEL_FIXTURE: &str = include_str!("fixtures/oembed/reel_public.json");

/// A later synthetic answer for the same permalink, changed upstream.
const REEL_FIXTURE_UPDATED: &str = include_str!("fixtures/oembed/reel_public_updated.json");

/// The canonical reel permalink the fixtures document.
const REEL_PERMALINK: &str = "https://www.instagram.com/reel/Csynthetic1/";

/// One stored revision as read back from the archive.
type RevisionRow = (Uuid, Uuid, Vec<u8>, Vec<u8>, i64, String, OffsetDateTime);

/// One revision joined with its raw evidence kind and parser version.
type EvidenceRow = (Uuid, String, Vec<u8>, Vec<u8>, i64, String);

/// A surface that replays scripted outcomes and counts its fetches.
#[derive(Debug)]
struct FakePublicSurface {
    script: Mutex<VecDeque<SurfaceOutcome>>,
    fetches: AtomicUsize,
}

impl FakePublicSurface {
    /// A surface that answers every fetch with one outcome.
    fn answering(outcome: SurfaceOutcome) -> Self {
        Self::scripted(vec![outcome])
    }

    /// A surface that answers each fetch with the next scripted outcome,
    /// repeating the final one when the script runs dry.
    fn scripted(script: Vec<SurfaceOutcome>) -> Self {
        assert!(!script.is_empty(), "a fake surface needs a script");
        Self {
            script: Mutex::new(VecDeque::from(script)),
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
        let mut script = self.script.lock().expect("script lock");
        if script.len() > 1 {
            script
                .pop_front()
                .expect("the script was just checked non-empty")
        } else {
            script
                .front()
                .expect("the script was just checked non-empty")
                .clone()
        }
    }
}

/// Submits one accepted capture through the real intake path.
#[expect(
    clippy::expect_used,
    reason = "integration-test helper: an intake refusal fails the test"
)]
async fn submitted_reel_capture(database: &ratatoskr_instagram_archive::Database) -> Uuid {
    let submission = database
        .submit_capture(&CaptureRequest {
            user_ref: Uuid::now_v7(),
            url: REEL_PERMALINK.to_owned(),
            captured_at: OffsetDateTime::UNIX_EPOCH,
            client_source: ClientSource::BrowserExtension,
            note: None,
            client_idempotency_key: None,
        })
        .await
        .expect("intake must accept a canonical reel permalink");
    match submission {
        CaptureSubmission::Created(record) | CaptureSubmission::Reused(record) => record.capture_id,
    }
}

/// Reads one stored revision back with its raw evidence.
#[expect(
    clippy::expect_used,
    reason = "integration-test helper: an unanswered read fails the test"
)]
async fn read_revision(
    database: &ratatoskr_instagram_archive::Database,
    revision_id: Uuid,
) -> Option<RevisionRow> {
    sqlx::query_as(
        "select r.revision_id, r.media_id, b.body, b.content_hash, b.byte_size, \
                r.parser_version, r.resolved_at \
         from instagram_archive.media_revisions r \
         join instagram_archive.raw_records b on b.raw_record_id = r.raw_record_id \
         where r.revision_id = $1",
    )
    .bind(revision_id)
    .fetch_optional(database.pool())
    .await
    .expect("the revision read must answer")
}

/// Counts the rows of one archive table.
#[expect(
    clippy::expect_used,
    reason = "integration-test helper: an unanswered count fails the test"
)]
async fn table_count(database: &ratatoskr_instagram_archive::Database, table: &str) -> i64 {
    sqlx::query_scalar(&format!("select count(*) from instagram_archive.{table}"))
        .fetch_one(database.pool())
        .await
        .expect("the count query must answer")
}

/// Resolves once and returns the stored resolution, failing the test otherwise.
#[expect(
    clippy::expect_used,
    reason = "integration-test helper: an unresolved payload fails the test"
)]
async fn resolve_once(
    test: &TestDatabase,
    capture_id: Uuid,
    surface: &FakePublicSurface,
    resolved_at: OffsetDateTime,
) -> ratatoskr_instagram_archive::resolution::StoredResolution {
    let outcome = test
        .database
        .resolve_capture_permalink(capture_id, surface, resolved_at)
        .await
        .expect("resolution must not error");
    let stored = if let ResolutionOutcome::Resolved(stored) = outcome {
        Some(stored)
    } else {
        None
    };
    stored.expect("a payload outcome must resolve")
}

#[tokio::test]
async fn resolved_payload_is_stored_as_an_immutable_oembed_revision() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let capture_id = submitted_reel_capture(&test.database).await;
    let surface = FakePublicSurface::answering(SurfaceOutcome::Payload {
        body: REEL_FIXTURE.to_owned(),
    });

    let stored = resolve_once(&test, capture_id, &surface, OffsetDateTime::UNIX_EPOCH).await;

    let revision: Option<EvidenceRow> = sqlx::query_as(
        "select r.revision_id, b.record_kind, b.body, b.content_hash, b.byte_size, \
                r.parser_version \
         from instagram_archive.media_revisions r \
         join instagram_archive.raw_records b on b.raw_record_id = r.raw_record_id \
         where r.revision_id = $1",
    )
    .bind(stored.revision_id)
    .fetch_optional(test.database.pool())
    .await
    .expect("the revision query must answer");
    let (_, record_kind, body, content_hash, byte_size, parser_version) =
        revision.expect("one immutable revision must exist for the stored resolution");
    assert_eq!(record_kind, "oembed_response", "raw evidence kind");
    assert_eq!(
        body,
        REEL_FIXTURE.as_bytes(),
        "the payload must be preserved byte for byte"
    );
    assert_eq!(
        byte_size,
        i64::try_from(REEL_FIXTURE.len()).expect("fixture size fits i64"),
        "byte size records the preserved length"
    );
    assert_eq!(
        content_hash,
        sha2::Sha256::digest(REEL_FIXTURE.as_bytes()).to_vec(),
        "the content hash covers exactly the preserved bytes"
    );
    assert_eq!(
        parser_version, OEMBED_PARSER_VERSION,
        "each revision records the parser version interpreting it"
    );

    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn second_resolution_appends_a_revision_and_leaves_the_first_untouched() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let capture_id = submitted_reel_capture(&test.database).await;
    let first_time = OffsetDateTime::UNIX_EPOCH;
    let second_time = first_time.replace_minute(7).expect("minute 7 exists");

    let surface = FakePublicSurface::answering(SurfaceOutcome::Payload {
        body: REEL_FIXTURE.to_owned(),
    });
    let first = resolve_once(&test, capture_id, &surface, first_time).await;
    let first_before = read_revision(&test.database, first.revision_id)
        .await
        .expect("the first revision must exist after the first resolution");

    let updated_surface = FakePublicSurface::answering(SurfaceOutcome::Payload {
        body: REEL_FIXTURE_UPDATED.to_owned(),
    });
    let second = resolve_once(&test, capture_id, &updated_surface, second_time).await;

    assert_ne!(
        first.revision_id, second.revision_id,
        "each attempt appends its own revision"
    );
    assert_eq!(
        first.media_id, second.media_id,
        "both revisions belong to the same source"
    );

    let first_after = read_revision(&test.database, first.revision_id)
        .await
        .expect("the first revision must still exist");
    assert_eq!(
        first_before, first_after,
        "re-resolution must not mutate the earlier revision"
    );
    assert_eq!(first_after.2, REEL_FIXTURE.as_bytes());

    let count = table_count(&test.database, "media_revisions").await;
    assert_eq!(count, 2, "two attempts produce exactly two revisions");

    let current: Uuid = sqlx::query_scalar(
        "select current_revision_id from instagram_archive.media where media_id = $1",
    )
    .bind(first.media_id)
    .fetch_one(test.database.pool())
    .await
    .expect("the media row must exist");
    assert_eq!(
        current, second.revision_id,
        "the normalized source points at the newest revision"
    );

    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn resolved_media_carries_public_resolution_provenance_and_capture_linkage() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let captured_at = OffsetDateTime::UNIX_EPOCH;
    let submission = test
        .database
        .submit_capture(&CaptureRequest {
            user_ref: Uuid::now_v7(),
            url: REEL_PERMALINK.to_owned(),
            captured_at,
            client_source: ClientSource::BrowserExtension,
            note: Some("keep for later".to_owned()),
            client_idempotency_key: None,
        })
        .await
        .expect("intake must accept the permalink");
    let capture_id = submission.record().capture_id;

    let surface = FakePublicSurface::answering(SurfaceOutcome::Payload {
        body: REEL_FIXTURE.to_owned(),
    });
    let stored = resolve_once(&test, capture_id, &surface, OffsetDateTime::UNIX_EPOCH).await;

    let media: (String, String, String) = sqlx::query_as(
        "select acquisition_method, saved_authority, upstream_status \
         from instagram_archive.media where media_id = $1",
    )
    .bind(stored.media_id)
    .fetch_one(test.database.pool())
    .await
    .expect("the normalized media row must exist");
    assert_eq!(
        media,
        (
            "public_resolution".to_owned(),
            "explicit_user_capture".to_owned(),
            "available".to_owned()
        ),
        "provenance is fixed by the mode ceiling, never by caller discipline"
    );

    let capture: (
        Option<Uuid>,
        String,
        Option<String>,
        OffsetDateTime,
        Option<String>,
    ) = sqlx::query_as(
        "select media_id, status, note, captured_at, canonical_url \
             from instagram_archive.captures where capture_id = $1",
    )
    .bind(capture_id)
    .fetch_one(test.database.pool())
    .await
    .expect("the capture row must exist");
    assert_eq!(
        capture.0,
        Some(stored.media_id),
        "capture links to its source"
    );
    assert_eq!(capture.1, "resolved", "resolution concludes the intake");
    assert_eq!(capture.2.as_deref(), Some("keep for later"));
    assert_eq!(capture.3, captured_at, "captured time is never rewritten");
    assert_eq!(capture.4.as_deref(), Some(REEL_PERMALINK));

    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn unsupported_outcome_fabricates_nothing() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let capture_id = submitted_reel_capture(&test.database).await;
    let surface = FakePublicSurface::answering(SurfaceOutcome::Unsupported);

    let outcome = test
        .database
        .resolve_capture_permalink(capture_id, &surface, OffsetDateTime::UNIX_EPOCH)
        .await
        .expect("an unsupported answer is not an error");

    assert_eq!(
        outcome,
        ResolutionOutcome::Unavailable(AvailabilityObservationKind::Unsupported),
        "an unsupported object reports exactly that kind"
    );

    let observations: Vec<String> = sqlx::query_as(
        "select availability from instagram_archive.availability_observations \
         where capture_id = $1",
    )
    .bind(capture_id)
    .fetch_all(test.database.pool())
    .await
    .expect("the observation query must answer")
    .into_iter()
    .map(|(availability,)| availability)
    .collect();
    assert_eq!(
        observations,
        vec!["unsupported".to_owned()],
        "one verbatim observation is appended"
    );

    let status: String =
        sqlx::query_scalar("select status from instagram_archive.captures where capture_id = $1")
            .bind(capture_id)
            .fetch_one(test.database.pool())
            .await
            .expect("the capture must exist");
    assert_eq!(status, "unavailable", "the intake concludes truthfully");

    let media_count = table_count(&test.database, "media").await;
    let revision_count = table_count(&test.database, "media_revisions").await;
    assert_eq!(
        (media_count, revision_count),
        (0, 0),
        "an unsupported answer fabricates no source and no history"
    );

    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn failure_kinds_survive_verbatim_without_inventing_deletion() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let capture_id = submitted_reel_capture(&test.database).await;
    let surface = FakePublicSurface::scripted(vec![
        SurfaceOutcome::Private,
        SurfaceOutcome::TemporarilyUnavailable,
        SurfaceOutcome::TransportFailure,
    ]);

    for attempt in 0..3 {
        let outcome = test
            .database
            .resolve_capture_permalink(capture_id, &surface, OffsetDateTime::UNIX_EPOCH)
            .await
            .expect("a classified failure is not an error");
        assert!(
            matches!(outcome, ResolutionOutcome::Unavailable(_)),
            "attempt {attempt} must conclude unavailable, got {outcome:?}"
        );
    }

    let mut observations: Vec<String> = sqlx::query_as(
        "select availability from instagram_archive.availability_observations \
         where capture_id = $1 order by observed_at, observation_id",
    )
    .bind(capture_id)
    .fetch_all(test.database.pool())
    .await
    .expect("the observation query must answer")
    .into_iter()
    .map(|(availability,)| availability)
    .collect();
    observations.sort();
    assert_eq!(
        observations,
        [
            "private".to_owned(),
            "resolution_failed".to_owned(),
            "temporarily_unavailable".to_owned(),
        ],
        "each outcome survives as its own kind"
    );
    assert!(
        !observations.contains(&"deleted".to_owned()),
        "no failure may be rewritten to deleted"
    );

    let status: String =
        sqlx::query_scalar("select status from instagram_archive.captures where capture_id = $1")
            .bind(capture_id)
            .fetch_one(test.database.pool())
            .await
            .expect("the capture must exist");
    assert_eq!(status, "unavailable");

    let media_count = table_count(&test.database, "media").await;
    let revision_count = table_count(&test.database, "media_revisions").await;
    assert_eq!(
        (media_count, revision_count),
        (0, 0),
        "failed outcomes fabricate nothing"
    );

    test.cleanup().await.expect("cleanup must drop");
}
