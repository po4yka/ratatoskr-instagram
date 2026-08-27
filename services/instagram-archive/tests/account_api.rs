//! Loopback official-account command API remains owner-bound and credential-free.

#![expect(
    clippy::expect_used,
    reason = "router-test setup failures are assertions"
)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::json;
use tower::ServiceExt as _;
use uuid::Uuid;

use ratatoskr_instagram_archive::credentials::crypto::{CredentialKeyring, KEY_LEN};
use ratatoskr_instagram_archive::test_support::{
    FakeInstagramProvider, FakeOAuthCodeRelay, TestDatabase,
};
use ratatoskr_instagram_archive_service::{
    OfficialAccountRuntime, product_router, product_router_with_official_accounts,
};

const USER: &str = "018f1a2b-3c4d-7e6f-8a9b-0c1d2e3f4a5b";

fn runtime() -> OfficialAccountRuntime {
    OfficialAccountRuntime::new(
        CredentialKeyring::new(7, BTreeMap::from([(7, [0x42; KEY_LEN])])).expect("test key exists"),
        Arc::new(FakeInstagramProvider::new([])),
        Arc::new(FakeOAuthCodeRelay::default()),
        "123456789".to_owned(),
        "https://platform.example.test/v1/oauth/callback/instagram".to_owned(),
        Duration::from_mins(10),
        5,
        1,
        false,
        true,
    )
}

async fn request(
    router: axum::Router,
    method: &str,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, String) {
    let response = router
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body collects")
        .to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test]
async fn begin_complete_refresh_capabilities_and_revoke_routes_are_loopback_commands() {
    let test = TestDatabase::create().await.expect("fresh database");
    let router = product_router_with_official_accounts(test.database.clone(), Some(runtime()));
    let (status, body) = request(
        router,
        "POST",
        "/v1/accounts/instagram/oauth/begin",
        json!({ "user_ref": USER }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("authorization_url"), "{body}");
    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn complete_accepts_relay_id_and_never_authorization_code() {
    let test = TestDatabase::create().await.expect("fresh database");
    let router = product_router_with_official_accounts(test.database.clone(), Some(runtime()));
    let (status, body) = request(
        router,
        "POST",
        "/v1/accounts/instagram/oauth/complete",
        json!({
            "user_ref": USER,
            "relay_id": "relay-id",
            "authorization_code": "MUST_NOT_CROSS_THIS_BOUNDARY"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(!body.contains("MUST_NOT_CROSS_THIS_BOUNDARY"));
    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn owner_mismatch_is_refused() {
    let test = TestDatabase::create().await.expect("fresh database");
    let router = product_router_with_official_accounts(test.database.clone(), Some(runtime()));
    let (status, _) = request(
        router,
        "GET",
        &format!(
            "/v1/accounts/instagram/{}/capabilities?user_ref={}",
            Uuid::now_v7(),
            Uuid::now_v7()
        ),
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn disabled_oauth_routes_are_unavailable() {
    let test = TestDatabase::create().await.expect("fresh database");
    let (status, body) = request(
        product_router(test.database.clone()),
        "POST",
        "/v1/accounts/instagram/oauth/begin",
        json!({ "user_ref": USER }),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn account_responses_never_contain_credentials() {
    let test = TestDatabase::create().await.expect("fresh database");
    let (status, body) = request(
        product_router_with_official_accounts(test.database.clone(), Some(runtime())),
        "POST",
        "/v1/accounts/instagram/oauth/begin",
        json!({ "user_ref": USER }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for forbidden in ["access_token", "refresh_token", "client_secret", "keyring"] {
        assert!(!body.contains(forbidden), "{forbidden} leaked: {body}");
    }
    test.cleanup().await.expect("cleanup drops");
}
