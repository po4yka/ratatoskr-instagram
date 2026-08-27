//! Synthetic official Meta adapter contract; no live account is used.

#![expect(
    clippy::expect_used,
    reason = "adapter-test setup failures are assertions"
)]

use secrecy::SecretString;

use ratatoskr_instagram_archive::provider::{
    BASIC_READ_SCOPE, ProviderFailureClass, ReqwestInstagramProvider,
};
use ratatoskr_instagram_archive::provider_budget::RequestClass;

const TOKEN: &str = "SYNTHETIC_SENTINEL_ACCESS_TOKEN";

fn provider(max_response_bytes: usize) -> ReqwestInstagramProvider {
    ReqwestInstagramProvider::for_test(
        "123456789".to_owned(),
        SecretString::from("synthetic-client-secret"),
        "https://platform.example.test/v1/oauth/callback/instagram".to_owned(),
        reqwest::Url::parse("http://127.0.0.1:49001/oauth/access_token")
            .expect("test exchange URL parses"),
        reqwest::Url::parse("http://127.0.0.1:49001/").expect("test graph URL parses"),
        max_response_bytes,
    )
    .expect("test provider builds")
}

#[test]
fn exchange_request_matches_documented_instagram_login_contract() {
    let request = provider(64 * 1024)
        .exchange_request(&SecretString::from("synthetic-single-use-code"))
        .expect("exchange request builds");
    assert_eq!(request.method(), reqwest::Method::POST);
    assert_eq!(request.url().path(), "/oauth/access_token");
    assert!(
        request.url().query().is_none(),
        "secrets belong in the form body"
    );
    let body = request
        .body()
        .and_then(reqwest::Body::as_bytes)
        .expect("form body is buffered");
    let form = String::from_utf8_lossy(body);
    assert!(form.contains("grant_type=authorization_code"), "{form}");
    assert!(form.contains("client_id=123456789"), "{form}");
    assert!(form.contains("redirect_uri="), "{form}");
    assert!(form.contains("code=synthetic-single-use-code"), "{form}");
}

#[test]
fn account_and_permission_fixtures_parse_strictly() {
    let provider = provider(64 * 1024);
    let account = provider
        .parse_account(include_bytes!("fixtures/meta/account_business.json"))
        .expect("documented account fixture parses");
    assert_eq!(account.provider_account_id, "17841400000000000");
    let permissions = provider
        .parse_permissions(include_bytes!(
            "fixtures/meta/permissions_basic_granted.json"
        ))
        .expect("documented permission fixture parses");
    assert_eq!(
        permissions.statuses.get(BASIC_READ_SCOPE),
        Some(&ratatoskr_instagram_archive::capability_reconciliation::PermissionStatus::Granted)
    );
}

#[test]
fn bearer_token_never_appears_in_url_or_diagnostics() {
    let request = provider(64 * 1024)
        .account_request(&SecretString::from(TOKEN))
        .expect("account request builds");
    assert!(!request.url().as_str().contains(TOKEN));
    assert!(!format!("{request:?}").contains(TOKEN));
    let authorization = request
        .headers()
        .get(reqwest::header::AUTHORIZATION)
        .expect("bearer header exists");
    assert_eq!(
        authorization.to_str().expect("ASCII header"),
        format!("Bearer {TOKEN}")
    );
}

#[test]
fn oversized_or_unknown_provider_response_is_refused() {
    let tiny = provider(8);
    assert_eq!(
        tiny.parse_account(include_bytes!("fixtures/meta/account_business.json"))
            .expect_err("oversize response refused")
            .class,
        ProviderFailureClass::ResponseRefused
    );
    let unknown = br#"{"id":"1","username":"x","account_type":"BUSINESS","surprise":true}"#;
    assert_eq!(
        provider(64 * 1024)
            .parse_account(unknown)
            .expect_err("unknown field refused")
            .class,
        ProviderFailureClass::ResponseRefused
    );
}

#[test]
fn auth_validation_rate_limit_server_and_network_failures_classify_typed() {
    assert_eq!(
        ReqwestInstagramProvider::classify_status(401),
        ProviderFailureClass::Authentication
    );
    assert_eq!(
        ReqwestInstagramProvider::classify_status(400),
        ProviderFailureClass::Validation
    );
    assert_eq!(
        ReqwestInstagramProvider::classify_status(429),
        ProviderFailureClass::RateLimited
    );
    assert_eq!(
        ReqwestInstagramProvider::classify_status(503),
        ProviderFailureClass::Server
    );
    assert_eq!(format!("{:?}", ProviderFailureClass::Network), "Network");
}

#[test]
fn discovery_retries_only_transient_failures_within_budget() {
    for failure in [
        ProviderFailureClass::Network,
        ProviderFailureClass::RateLimited,
        ProviderFailureClass::Server,
    ] {
        assert!(ReqwestInstagramProvider::should_retry(
            RequestClass::AccountDiscovery,
            failure
        ));
    }
    assert!(!ReqwestInstagramProvider::should_retry(
        RequestClass::PermissionDiscovery,
        ProviderFailureClass::Authentication
    ));
}

#[test]
fn oauth_code_exchange_is_never_retried() {
    for failure in [
        ProviderFailureClass::Network,
        ProviderFailureClass::RateLimited,
        ProviderFailureClass::Server,
    ] {
        assert!(!ReqwestInstagramProvider::should_retry(
            RequestClass::CodeExchange,
            failure
        ));
    }
}
