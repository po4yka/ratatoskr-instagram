//! Product-plane contract: `POST /v1/captures` speaks the platform capture
//! grammar — created on first delivery, reuse on replay, typed refusals.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::json;
use tower::ServiceExt as _;
use uuid::Uuid;

use ratatoskr_instagram_archive::test_support::TestDatabase;

const USER: &str = "018f1a2b-3c4d-7e6f-8a9b-0c1d2e3f4a5b";
const CAPTURED_AT: &str = "2026-08-17T06:30:00Z";

/// The grammar-shaped body every happy-path case starts from.
fn grammar_body() -> serde_json::Value {
    json!({
        "user_ref": USER,
        "platform": "instagram",
        "canonical_url": "https://instagram.com/p/DHcxI7hpS5t/?utm_source=share",
        "captured_at": CAPTURED_AT,
        "source": "ios_share_extension",
        "note": "composition study",
    })
}

#[expect(
    clippy::expect_used,
    reason = "router-test helper: an unanswered request or unreadable body is the failure"
)]
async fn post_captures(
    database: &ratatoskr_instagram_archive::Database,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let router = ratatoskr_instagram_archive_service::product_router(database.clone());
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/captures")
                .header("content-type", "application/json")
                .header("idempotency-key", "platform-op-key")
                .body(Body::from(body.to_string()))
                .expect("a valid request"),
        )
        .await
        .expect("the router answers");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a collectible body")
        .to_bytes();
    let parsed = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("JSON error bodies stay structured")
    };
    (status, parsed)
}

#[tokio::test]
async fn first_submission_answers_created_with_the_canonical_capture() {
    let test = TestDatabase::create().await.expect("a fresh test database");

    let (status, body) = post_captures(&test.database, grammar_body()).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(
        body["canonical_url"],
        "https://www.instagram.com/p/DHcxI7hpS5t/"
    );
    assert_eq!(body["status"], "accepted");
    assert_eq!(body["saved_authority"], "explicit_user_capture");
    assert_eq!(body["acquisition_method"], "share_extension");
    assert_eq!(body["captured_at"], CAPTURED_AT);
    assert_eq!(body["reused"], false);
    assert!(
        body["capture_id"].as_str().is_some(),
        "the local identity is returned: {body}"
    );

    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn unchanged_replay_answers_ok_marked_reused() {
    let test = TestDatabase::create().await.expect("a fresh test database");

    let (first_status, first_body) = post_captures(&test.database, grammar_body()).await;
    assert_eq!(first_status, StatusCode::CREATED, "{first_body}");

    // A replay carries a different save time and note, like a real retry would.
    let mut replay = grammar_body();
    replay["captured_at"] = json!("2026-08-18T20:11:32Z");
    replay["note"] = json!("a different note");
    let (second_status, second_body) = post_captures(&test.database, replay).await;
    assert_eq!(second_status, StatusCode::OK, "{second_body}");
    assert_eq!(second_body["reused"], true);
    assert_eq!(
        second_body["capture_id"], first_body["capture_id"],
        "the replay converges on the original capture"
    );
    assert_eq!(
        second_body["captured_at"], first_body["captured_at"],
        "reuse preserves the original save time"
    );

    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn unknown_platform_is_refused_with_a_typed_code() {
    let test = TestDatabase::create().await.expect("a fresh test database");

    let mut body = grammar_body();
    body["platform"] = json!("tiktok");
    let (status, answer) = post_captures(&test.database, body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["error"], "unknown_platform", "{answer}");

    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn non_permalink_paths_are_refused_as_unsupported_urls() {
    let test = TestDatabase::create().await.expect("a fresh test database");

    let mut body = grammar_body();
    body["canonical_url"] = json!("https://www.instagram.com/someuser/");
    let (status, answer) = post_captures(&test.database, body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["error"], "unsupported_url", "{answer}");

    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn telegram_source_is_refused_until_the_vocabulary_extends() {
    let test = TestDatabase::create().await.expect("a fresh test database");

    let mut body = grammar_body();
    body["source"] = json!("telegram");
    let (status, answer) = post_captures(&test.database, body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["error"], "unsupported_client_source", "{answer}");

    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn unparsable_bodies_and_timestamps_are_refused() {
    let test = TestDatabase::create().await.expect("a fresh test database");

    // Not JSON at all.
    let router = ratatoskr_instagram_archive_service::product_router(test.database.clone());
    let raw = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/captures")
                .header("content-type", "application/json")
                .body(Body::from("not json"))
                .expect("a valid request"),
        )
        .await
        .expect("the router answers");
    assert_eq!(raw.status(), StatusCode::BAD_REQUEST);

    // Valid JSON, unparsable instant.
    let mut body = grammar_body();
    body["captured_at"] = json!("yesterday afternoon");
    let (_, answer) = post_captures(&test.database, body).await;
    assert_eq!(answer["error"], "invalid_request", "{answer}");

    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn unknown_user_refs_are_accepted_shapes_only_the_owner_knows() {
    // The intake trusts its platform caller for identity (see design.md);
    // there is deliberately no user lookup to leak account existence.
    let test = TestDatabase::create().await.expect("a fresh test database");
    let mut body = grammar_body();
    body["user_ref"] = json!(Uuid::now_v7().to_string());
    let (status, _) = post_captures(&test.database, body).await;
    assert_eq!(status, StatusCode::CREATED);
    test.cleanup().await.expect("cleanup drops");
}
