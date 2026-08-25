//! Capture intake contract: provenance on creation, deduplicating identity,
//! and the unavailable fallback that preserves the attempt truthfully.

use time::OffsetDateTime;
use time::macros::datetime;
use uuid::Uuid;

use ratatoskr_instagram_archive::capability::AvailabilityObservationKind as Kind;
use ratatoskr_instagram_archive::capture::{
    CaptureError, CaptureRequest, CaptureStatus, CaptureSubmission, ClientSource,
};
use ratatoskr_instagram_archive::test_support::TestDatabase;

const POST_URL: &str = "https://instagram.com/p/DHcxI7hpS5t/";
const CANONICAL_POST_URL: &str = "https://www.instagram.com/p/DHcxI7hpS5t/";

/// A whole-second instant: `timestamptz` keeps microseconds, so tests use
/// inputs that survive the round trip byte for byte.
const CAPTURED_AT: OffsetDateTime = datetime!(2026-08-17 10:30:00 +04:00);

async fn submit(
    database: &ratatoskr_instagram_archive::Database,
    user_ref: Uuid,
    url: &str,
    client_source: ClientSource,
) -> Result<CaptureSubmission, CaptureError> {
    let request = CaptureRequest {
        user_ref,
        url: url.to_owned(),
        captured_at: CAPTURED_AT,
        client_source,
        note: Some("composition study".to_owned()),
        client_idempotency_key: None,
    };
    database.submit_capture(&request).await
}

#[tokio::test]
async fn fresh_submission_creates_a_capture_with_explicit_provenance() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let submission = submit(
        &test.database,
        Uuid::now_v7(),
        POST_URL,
        ClientSource::IosShareExtension,
    )
    .await
    .expect("a fresh capture is created");
    assert!(!submission.is_reuse(), "the first delivery creates");

    let record = submission.record();
    assert_eq!(
        record.canonical_url, CANONICAL_POST_URL,
        "the canonical form is stored"
    );
    assert_eq!(
        record.saved_authority, "explicit_user_capture",
        "an explicit capture never claims more than explicit authority"
    );
    assert_eq!(record.acquisition_method, "share_extension");
    assert_eq!(record.client_source, "ios_share_extension");
    assert_eq!(record.status, CaptureStatus::Accepted);
    assert_eq!(
        record.captured_at, CAPTURED_AT,
        "the user's save time is preserved"
    );

    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn every_supported_client_source_maps_to_its_contract_acquisition_method() {
    for (source, expected_method) in [
        (ClientSource::IosShareExtension, "share_extension"),
        (ClientSource::AndroidShareTarget, "share_extension"),
        (ClientSource::BrowserExtension, "browser_extension"),
    ] {
        let test = TestDatabase::create().await.expect("a fresh test database");
        let submission = submit(&test.database, Uuid::now_v7(), POST_URL, source)
            .await
            .expect("every supported source creates a capture");
        assert_eq!(
            submission.record().acquisition_method,
            expected_method,
            "{source:?}"
        );
        assert_eq!(submission.record().client_source, source.wire_value());
        test.cleanup().await.expect("cleanup drops");
    }
}

#[tokio::test]
async fn duplicate_submission_reuses_the_original_record_untouched() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let database = &test.database;
    let pool = test.database.pool();
    let user = Uuid::now_v7();

    let first = submit(database, user, POST_URL, ClientSource::IosShareExtension)
        .await
        .expect("the first delivery creates");

    // The duplicate differs in everything except the identity pair.
    let request = CaptureRequest {
        user_ref: user,
        url: CANONICAL_POST_URL.to_owned(),
        captured_at: datetime!(2026-08-18 23:59:59 +04:00),
        client_source: ClientSource::BrowserExtension,
        note: Some("a different note".to_owned()),
        client_idempotency_key: Some("platform-op-key".to_owned()),
    };
    let second = database
        .submit_capture(&request)
        .await
        .expect("the replay succeeds");

    assert!(second.is_reuse(), "the replay must report reuse");
    assert_eq!(
        second.record(),
        first.record(),
        "reuse hands back the original record untouched"
    );

    let rows: i64 = sqlx::query_scalar("select count(*) from instagram_archive.captures")
        .fetch_one(pool)
        .await
        .expect("the count answers");
    assert_eq!(rows, 1, "exactly one capture row exists");

    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn different_users_are_distinct_captures_for_the_same_url() {
    let test = TestDatabase::create().await.expect("a fresh test database");

    let one = submit(
        &test.database,
        Uuid::now_v7(),
        POST_URL,
        ClientSource::IosShareExtension,
    )
    .await
    .expect("first user creates");
    let two = submit(
        &test.database,
        Uuid::now_v7(),
        POST_URL,
        ClientSource::IosShareExtension,
    )
    .await
    .expect("second user creates for the same URL");
    assert_ne!(one.record().capture_id, two.record().capture_id);

    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn different_urls_are_distinct_captures_for_one_user() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let database = &test.database;
    let user = Uuid::now_v7();

    let post = submit(database, user, POST_URL, ClientSource::IosShareExtension)
        .await
        .expect("post creates");
    let reel = submit(
        database,
        user,
        "https://www.instagram.com/reels/DHab_c9-x/",
        ClientSource::IosShareExtension,
    )
    .await
    .expect("reel creates");

    assert_ne!(post.record().capture_id, reel.record().capture_id);
    assert!(!post.is_reuse());
    assert!(!reel.is_reuse());

    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn racing_duplicates_converge_on_one_row() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let database = &test.database;
    let pool = test.database.pool();
    let user = Uuid::now_v7();

    let request = CaptureRequest {
        user_ref: user,
        url: POST_URL.to_owned(),
        captured_at: CAPTURED_AT,
        client_source: ClientSource::IosShareExtension,
        note: None,
        client_idempotency_key: None,
    };

    let (left, right) = tokio::join!(
        database.submit_capture(&request),
        database.submit_capture(&request)
    );
    let left = left.expect("both racers succeed");
    let right = right.expect("both racers succeed");

    let creators = [left.is_reuse(), right.is_reuse()]
        .iter()
        .filter(|reuse| !**reuse)
        .count();
    assert_eq!(creators, 1, "exactly one racer creates; both converge");

    let rows: i64 = sqlx::query_scalar("select count(*) from instagram_archive.captures")
        .fetch_one(pool)
        .await
        .expect("the count answers");
    assert_eq!(rows, 1);

    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn telegram_is_refused_until_the_contract_vocabulary_extends() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let error = submit(
        &test.database,
        Uuid::now_v7(),
        POST_URL,
        ClientSource::Telegram,
    )
    .await
    .expect_err("telegram has no honest acquisition method yet");
    assert!(
        matches!(error, CaptureError::UnsupportedClientSource),
        "the refusal names the missing vocabulary: {error}"
    );

    let rows: i64 = sqlx::query_scalar("select count(*) from instagram_archive.captures")
        .fetch_one(test.database.pool())
        .await
        .expect("the count answers");
    assert_eq!(rows, 0, "nothing is stored behind the refusal");

    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn non_permalink_urls_are_refused_without_storing() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let error = submit(
        &test.database,
        Uuid::now_v7(),
        "https://www.instagram.com/explore/tags/rust/",
        ClientSource::IosShareExtension,
    )
    .await
    .expect_err("an explore path is not a permalink");
    assert!(
        matches!(error, CaptureError::InvalidUrl(_)),
        "the refusal carries the permalink reason: {error}"
    );

    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn recording_unavailability_preserves_the_attempt_truthfully() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let pool = test.database.pool();
    let submission = submit(
        &test.database,
        Uuid::now_v7(),
        POST_URL,
        ClientSource::IosShareExtension,
    )
    .await
    .expect("the capture exists first");
    let capture_id = submission.record().capture_id;

    let observed_at = datetime!(2026-08-19 08:00:00 UTC);
    let status = test
        .database
        .record_capture_unavailable(capture_id, Kind::Private, observed_at)
        .await
        .expect("the fallback records");
    assert_eq!(status, CaptureStatus::Unavailable);

    let row: (String, Option<String>, String, OffsetDateTime) = sqlx::query_as(
        "select canonical_url, note, status, captured_at \
         from instagram_archive.captures where capture_id = $1",
    )
    .bind(capture_id)
    .fetch_one(pool)
    .await
    .expect("the capture row is readable");
    assert_eq!(row.0, CANONICAL_POST_URL, "the URL survives untouched");
    assert_eq!(
        row.1.as_deref(),
        Some("composition study"),
        "the note survives untouched"
    );
    assert_eq!(row.2, "unavailable", "the status records unavailability");
    assert_eq!(row.3, CAPTURED_AT, "the save time survives untouched");

    let observations: i64 = sqlx::query_scalar(
        "select count(*) from instagram_archive.availability_observations \
         where capture_id = $1 and availability = 'private' and media_id is null",
    )
    .bind(capture_id)
    .fetch_one(pool)
    .await
    .expect("the observation count answers");
    assert_eq!(
        observations, 1,
        "one private observation bound to the capture"
    );

    let media_rows: i64 = sqlx::query_scalar("select count(*) from instagram_archive.media")
        .fetch_one(pool)
        .await
        .expect("the media count answers");
    assert_eq!(media_rows, 0, "no content may be fabricated");

    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn unavailability_observations_accumulate_without_fabricating_content() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let pool = test.database.pool();
    let submission = submit(
        &test.database,
        Uuid::now_v7(),
        POST_URL,
        ClientSource::IosShareExtension,
    )
    .await
    .expect("the capture exists first");
    let capture_id = submission.record().capture_id;

    test.database
        .record_capture_unavailable(
            capture_id,
            Kind::Deleted,
            datetime!(2026-08-19 08:00:00 UTC),
        )
        .await
        .expect("the first observation records");
    test.database
        .record_capture_unavailable(
            capture_id,
            Kind::TemporarilyUnavailable,
            datetime!(2026-08-20 09:30:00 UTC),
        )
        .await
        .expect("the second observation records");

    let ordered: Vec<(String, OffsetDateTime)> = sqlx::query_as(
        "select availability, observed_at from instagram_archive.availability_observations \
         where capture_id = $1 order by observed_at",
    )
    .bind(capture_id)
    .fetch_all(pool)
    .await
    .expect("the observations are readable");
    assert_eq!(
        ordered,
        vec![
            ("deleted".to_owned(), datetime!(2026-08-19 08:00:00 UTC)),
            (
                "temporarily_unavailable".to_owned(),
                datetime!(2026-08-20 09:30:00 UTC)
            ),
        ],
        "observations accumulate in order"
    );

    let media_rows: i64 = sqlx::query_scalar("select count(*) from instagram_archive.media")
        .fetch_one(pool)
        .await
        .expect("the media count answers");
    assert_eq!(media_rows, 0, "still no content fabricated");

    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn recording_unavailability_for_an_unknown_capture_writes_nothing() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let pool = test.database.pool();

    let error = test
        .database
        .record_capture_unavailable(
            Uuid::now_v7(),
            Kind::Private,
            datetime!(2026-08-19 08:00:00 UTC),
        )
        .await
        .expect_err("an unknown capture cannot become unavailable");
    assert!(
        matches!(error, CaptureError::UnknownCapture),
        "the refusal is a not-found, not a silent write: {error}"
    );

    let observations: i64 =
        sqlx::query_scalar("select count(*) from instagram_archive.availability_observations")
            .fetch_one(pool)
            .await
            .expect("the observation count answers");
    assert_eq!(observations, 0, "nothing was written");

    test.cleanup().await.expect("cleanup drops");
}
