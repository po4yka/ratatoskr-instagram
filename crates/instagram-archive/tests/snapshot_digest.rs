//! The content digest and the observation time of an Instagram fact describe the post
//! (XR-021 CONTRACTS.md R5-09): a changed caption or a post deleted upstream is a new fact that
//! Knowledge admits as current, and a completion links only the digest that is current.
//!
//! Every test runs against a disposable `PostgreSQL` database; the public surface is replayed from
//! committed synthetic fixtures and no test makes a live call.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Mutex;

use ratatoskr_event_envelope::EventEnvelope;
use ratatoskr_identifiers::Extensions;
use ratatoskr_instagram_archive::capture::{CaptureRequest, CaptureSubmission, ClientSource};
use ratatoskr_instagram_archive::permalink::CanonicalPermalink;
use ratatoskr_instagram_archive::publishing::snapshot_content_digest;
use ratatoskr_instagram_archive::resolution::{PublicSurface, SurfaceOutcome};
use ratatoskr_instagram_archive::test_support::TestDatabase;
use ratatoskr_instagram_archive::{AnalysisCompletionOutcome, Database};
use ratatoskr_social_contracts::{
    PostText, SocialSourceAnalysisCompleted, SocialSourceCaptured, SocialSourceSnapshot,
    SocialSourceUpdated, UpstreamAvailability,
};
use sha2::Digest as _;
use time::OffsetDateTime;
use uuid::Uuid;

const REEL_FIXTURE: &str = include_str!("fixtures/oembed/reel_public.json");
const REEL_FIXTURE_UPDATED: &str = include_str!("fixtures/oembed/reel_public_updated.json");
const REEL_PERMALINK: &str = "https://www.instagram.com/reel/Csynthetic1/";

/// The golden preimage of the digest of the captured fixture: the canonical JSON of what the
/// snapshot states, pinned byte for byte so a change to the rendering fails here instead of
/// silently re-keying every analysis.
const GOLDEN_PREIMAGE: &str = "{\n  \"domain\": \"instagram.snapshot-content\",\n  \"permalink\": \"https://www.instagram.com/reel/Csynthetic1/\",\n  \"external_post_id\": \"Csynthetic1\",\n  \"caption\": \"A public reel about composition\",\n  \"upstream\": \"available\",\n  \"media\": []\n}\n";

/// A surface that answers one scripted outcome.
#[derive(Debug)]
struct Answer(Mutex<VecDeque<SurfaceOutcome>>);

impl Answer {
    fn new(outcome: SurfaceOutcome) -> Self {
        Self(Mutex::new(VecDeque::from(vec![outcome])))
    }

    fn payload(body: &str) -> Self {
        Self::new(SurfaceOutcome::Payload {
            body: body.to_owned(),
        })
    }
}

impl PublicSurface for Answer {
    #[expect(
        clippy::expect_used,
        reason = "test fake: a poisoned script lock cannot be recovered"
    )]
    async fn fetch(&self, _permalink: &CanonicalPermalink) -> SurfaceOutcome {
        self.0
            .lock()
            .expect("script lock")
            .front()
            .expect("the script holds one outcome")
            .clone()
    }
}

/// One published fact of a capture, decoded.
struct Fact {
    event_type: String,
    source: SocialSourceSnapshot,
}

fn after_epoch(seconds: i64) -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(seconds)
}

#[expect(
    clippy::expect_used,
    reason = "integration-test helper: an intake refusal fails the test"
)]
async fn submitted(database: &Database, owner: Uuid) -> Uuid {
    let submission = database
        .submit_capture(&CaptureRequest {
            user_ref: owner,
            url: REEL_PERMALINK.to_owned(),
            captured_at: OffsetDateTime::UNIX_EPOCH,
            client_source: ClientSource::BrowserExtension,
            note: None,
            client_idempotency_key: None,
        })
        .await
        .expect("intake accepts a canonical reel permalink");
    match submission {
        CaptureSubmission::Created(record) | CaptureSubmission::Reused(record) => record.capture_id,
    }
}

#[expect(
    clippy::expect_used,
    reason = "integration-test helper: an unexpected outcome fails the test"
)]
async fn resolve(test: &TestDatabase, capture_id: Uuid, answer: &Answer, at: OffsetDateTime) {
    test.database
        .resolve_capture_permalink(capture_id, answer, at)
        .await
        .expect("the resolution answers");
}

/// Resolves a fresh capture at `+30 s` with the first fixture and returns its id.
async fn captured_post(test: &TestDatabase, owner: Uuid) -> Uuid {
    let capture_id = submitted(&test.database, owner).await;
    resolve(
        test,
        capture_id,
        &Answer::payload(REEL_FIXTURE),
        after_epoch(30),
    )
    .await;
    capture_id
}

/// Every fact of the capture, oldest first.
#[expect(
    clippy::expect_used,
    reason = "integration-test helper: a malformed fact fails the test"
)]
async fn facts(test: &TestDatabase, capture_id: Uuid) -> Vec<Fact> {
    let rows: Vec<(serde_json::Value,)> = sqlx::query_as(
        "select payload from instagram_archive.outbox_events \
         where aggregate_type = 'capture' and aggregate_id = $1 order by event_id",
    )
    .bind(capture_id)
    .fetch_all(test.database.pool())
    .await
    .expect("the outbox answers");
    rows.into_iter()
        .map(|(payload,)| {
            let bytes = serde_json::to_vec(&payload).expect("the payload re-serializes");
            let envelope = EventEnvelope::from_json(&bytes).expect("a stored envelope parses");
            let event_type = envelope.event_type.to_string();
            let source = if event_type == "social.source.captured.v1" {
                envelope
                    .payload_as::<SocialSourceCaptured>()
                    .expect("a captured fact decodes")
                    .source
            } else {
                envelope
                    .payload_as::<SocialSourceUpdated>()
                    .expect("an updated fact decodes")
                    .source
            };
            Fact { event_type, source }
        })
        .collect()
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a valid canonical envelope is required by the contract"
)]
fn completion_envelope(payload: &SocialSourceAnalysisCompleted) -> Vec<u8> {
    let owner = payload.owner.to_string();
    let template = serde_json::json!({
        "event_id": Uuid::now_v7().to_string(),
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

fn completion_for(fact: &Fact) -> SocialSourceAnalysisCompleted {
    SocialSourceAnalysisCompleted {
        owner: fact.source.owner,
        social_source_id: fact.source.social_source_id,
        content_digest: fact.source.content_digest.clone(),
        completed_at: fact.source.captured_at,
        extensions: Extensions::default(),
    }
}

#[tokio::test]
async fn the_digest_describes_the_post() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let capture_id = captured_post(&test, Uuid::now_v7()).await;
    let published = facts(&test, capture_id).await;
    let source = &published[0].source;

    let mut other_caption = source.clone();
    other_caption.text = Some(PostText::parse("The caption was edited").expect("a caption"));
    let mut deleted = source.clone();
    deleted.upstream_availability = UpstreamAvailability::DeletedUpstream;

    let original = snapshot_content_digest(source);
    let edited = snapshot_content_digest(&other_caption);
    let removed = snapshot_content_digest(&deleted);
    assert_ne!(original, edited, "a changed caption is a different digest");
    assert_ne!(
        original, removed,
        "a post deleted upstream is a different digest"
    );
    assert_ne!(
        edited, removed,
        "the caption and the deletion are different facts"
    );
    assert_eq!(
        original,
        snapshot_content_digest(&source.clone()),
        "two builds of the same stored rows have equal digests"
    );
    assert_eq!(
        source.content_digest, original,
        "the published digest is the digest of the published snapshot"
    );
    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn the_preimage_bytes_are_fixed() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let capture_id = captured_post(&test, Uuid::now_v7()).await;
    let published = facts(&test, capture_id).await;

    let expected = format!("{:x}", sha2::Sha256::digest(GOLDEN_PREIMAGE.as_bytes()));
    assert_eq!(
        published[0].source.content_digest.hex.to_string(),
        expected,
        "the digest is the SHA-256 of the golden canonical JSON preimage"
    );
    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn a_changed_caption_is_a_later_and_different_updated_fact() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let capture_id = captured_post(&test, Uuid::now_v7()).await;
    resolve(
        &test,
        capture_id,
        &Answer::payload(REEL_FIXTURE_UPDATED),
        after_epoch(90),
    )
    .await;

    let published = facts(&test, capture_id).await;
    assert_eq!(published.len(), 2, "one captured and one updated fact");
    assert_eq!(published[0].event_type, "social.source.captured.v1");
    assert_eq!(published[1].event_type, "social.source.updated.v1");
    assert_ne!(
        published[0].source.content_digest, published[1].source.content_digest,
        "the edited caption is a different digest"
    );
    assert_eq!(
        published[0].source.captured_at.to_wire(),
        "1970-01-01T00:00:00Z",
        "a captured fact keeps the capture's own time"
    );
    let (resolved_at,): (OffsetDateTime,) = sqlx::query_as(
        "select resolved_at from instagram_archive.media_revisions order by resolved_at desc limit 1",
    )
    .fetch_one(test.database.pool())
    .await
    .expect("the newest revision");
    assert_eq!(resolved_at, after_epoch(90));
    assert_eq!(
        published[1].source.captured_at.to_wire(),
        "1970-01-01T00:01:30Z",
        "an updated fact is observed when the new revision was resolved"
    );
    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn an_unchanged_re_resolution_publishes_nothing() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let capture_id = captured_post(&test, Uuid::now_v7()).await;
    resolve(
        &test,
        capture_id,
        &Answer::payload(REEL_FIXTURE),
        after_epoch(90),
    )
    .await;

    let published = facts(&test, capture_id).await;
    assert_eq!(published.len(), 1, "an unchanged answer is not a new fact");
    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn a_deletion_observed_upstream_is_a_third_digest_and_later() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let capture_id = captured_post(&test, Uuid::now_v7()).await;
    resolve(
        &test,
        capture_id,
        &Answer::payload(REEL_FIXTURE_UPDATED),
        after_epoch(90),
    )
    .await;
    resolve(
        &test,
        capture_id,
        &Answer::new(SurfaceOutcome::Deleted),
        after_epoch(150),
    )
    .await;

    let published = facts(&test, capture_id).await;
    assert_eq!(published.len(), 3, "captured, edited, deleted upstream");
    let deleted = &published[2];
    assert_eq!(deleted.event_type, "social.source.updated.v1");
    assert_eq!(
        deleted.source.upstream_availability,
        UpstreamAvailability::DeletedUpstream
    );
    assert_ne!(
        deleted.source.content_digest,
        published[0].source.content_digest
    );
    assert_ne!(
        deleted.source.content_digest,
        published[1].source.content_digest
    );
    assert_eq!(
        deleted.source.captured_at.to_wire(),
        "1970-01-01T00:02:30Z",
        "the deletion fact is observed when the deletion was observed"
    );
    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn the_updated_fact_validates_as_the_contract() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let capture_id = captured_post(&test, Uuid::now_v7()).await;
    resolve(
        &test,
        capture_id,
        &Answer::payload(REEL_FIXTURE_UPDATED),
        after_epoch(90),
    )
    .await;

    let published = facts(&test, capture_id).await;
    published[1]
        .source
        .validate()
        .expect("the updated snapshot satisfies every cross-field invariant");
    assert!(
        published[1].source.captured_at > published[0].source.captured_at,
        "Knowledge admits the update as current only when it is observed strictly later"
    );
    test.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn a_stale_completion_does_not_link_and_the_new_digest_does() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let capture_id = captured_post(&test, Uuid::now_v7()).await;
    resolve(
        &test,
        capture_id,
        &Answer::payload(REEL_FIXTURE_UPDATED),
        after_epoch(90),
    )
    .await;
    let published = facts(&test, capture_id).await;

    let stale = test
        .database
        .ingest_analysis_completed(&completion_envelope(&completion_for(&published[0])))
        .await
        .expect("a stale completion is accepted and skipped");
    assert_eq!(stale, AnalysisCompletionOutcome::Skipped);
    let current = test
        .database
        .ingest_analysis_completed(&completion_envelope(&completion_for(&published[1])))
        .await
        .expect("the current completion is accepted");
    assert_eq!(current, AnalysisCompletionOutcome::Linked);

    let links: Vec<(String,)> = sqlx::query_as(
        "select content_digest from instagram_archive.capture_analysis_links where capture_id = $1",
    )
    .bind(capture_id)
    .fetch_all(test.database.pool())
    .await
    .expect("the links answer");
    assert_eq!(
        links,
        vec![(published[1].source.content_digest.hex.to_string(),)],
        "only the current digest is linked, and it is the one stored"
    );
    test.cleanup().await.expect("cleanup");
}

#[test]
fn no_constant_digest_remains() {
    fn scan(directory: &Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(directory).expect("the source directory reads") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                scan(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let text = std::fs::read_to_string(&path).expect("a source file reads");
                if text.contains("oembed-metadata-only") || text.contains("fn content_digest(") {
                    found.push(path.display().to_string());
                }
            }
        }
    }
    let mut found = Vec::new();
    scan(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut found,
    );
    assert!(found.is_empty(), "a constant digest remains in {found:?}");
}
