//! Owner-bound official OAuth flow persistence and least-privilege authorization URL.

#![expect(
    clippy::expect_used,
    reason = "integration-test setup failures are assertions"
)]

use std::collections::BTreeMap;
use std::time::Duration;

use base64::Engine as _;
use secrecy::ExposeSecret as _;
use secrecy::SecretString;
use sha2::{Digest as _, Sha256};
use time::OffsetDateTime;
use uuid::Uuid;

use ratatoskr_instagram_archive::capability_reconciliation::{AccountType, PermissionStatus};
use ratatoskr_instagram_archive::credentials::crypto::{
    CredentialKeyring, KEY_LEN, TokenBinding, TokenKind,
};
use ratatoskr_instagram_archive::provider::BASIC_READ_SCOPE;
use ratatoskr_instagram_archive::provider::{
    ExchangedToken, ProviderAccount, ProviderPermissions, RelayClaim,
};
use ratatoskr_instagram_archive::test_support::{
    FakeInstagramProvider, FakeOAuthCodeRelay, FakeProviderStep, TestDatabase,
};

fn keyring() -> CredentialKeyring {
    CredentialKeyring::new(7, BTreeMap::from([(7, [0x42; KEY_LEN])])).expect("test key exists")
}

async fn begin(test: &TestDatabase, pkce: bool) -> ratatoskr_instagram_archive::oauth::OAuthBegin {
    test.database
        .begin_official_oauth(
            &keyring(),
            Uuid::from_u128(0x018f_1a2b_3c4d_7e6f_8a9b_0c1d_2e3f_4a5b),
            "123456789",
            "https://platform.example.test/v1/oauth/callback/instagram",
            Duration::from_mins(10),
            pkce,
            OffsetDateTime::UNIX_EPOCH,
        )
        .await
        .expect("OAuth begin succeeds")
}

async fn prepared_completion(
    test: &TestDatabase,
) -> (
    Uuid,
    FakeInstagramProvider,
    FakeOAuthCodeRelay,
    &'static str,
    &'static str,
) {
    const RELAY_ID: &str = "relay-018f1a2b";
    const ACCESS_TOKEN: &str = "SYNTHETIC_COMPLETION_ACCESS_TOKEN";
    let user_ref = Uuid::from_u128(0x018f_1a2b_3c4d_7e6f_8a9b_0c1d_2e3f_4a5b);
    let begun = begin(test, false).await;
    let url = reqwest::Url::parse(&begun.authorization_url).expect("authorization URL parses");
    let state = url
        .query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.into_owned())
        .expect("state exists");
    let relay = FakeOAuthCodeRelay::default();
    relay
        .insert(
            RELAY_ID.to_owned(),
            RelayClaim {
                user_ref,
                state: SecretString::from(state),
                authorization_code: SecretString::from("SYNTHETIC_COMPLETION_CODE"),
                redirect_uri: "https://platform.example.test/v1/oauth/callback/instagram"
                    .to_owned(),
            },
        )
        .expect("relay inserts");
    let provider = FakeInstagramProvider::new([
        FakeProviderStep::Exchange(Ok(ExchangedToken {
            access_token: SecretString::from(ACCESS_TOKEN),
            user_id: "17841400000000000".to_owned(),
            permissions: vec![BASIC_READ_SCOPE.to_owned()],
            expires_in_seconds: Some(5_184_000),
        })),
        FakeProviderStep::Account(Ok(ProviderAccount {
            provider_account_id: "17841400000000000".to_owned(),
            username: "synthetic_business".to_owned(),
            account_type: AccountType::Business,
        })),
        FakeProviderStep::Permissions(Ok(ProviderPermissions {
            statuses: BTreeMap::from([(BASIC_READ_SCOPE.to_owned(), PermissionStatus::Granted)]),
        })),
    ]);
    (user_ref, provider, relay, RELAY_ID, ACCESS_TOKEN)
}

#[tokio::test]
async fn begin_returns_owner_bound_single_read_scope_authorize_url() {
    let test = TestDatabase::create().await.expect("fresh database");
    let begun = begin(&test, false).await;
    let url = reqwest::Url::parse(&begun.authorization_url).expect("authorization URL parses");
    assert_eq!(url.host_str(), Some("www.instagram.com"));
    let query = url.query_pairs().collect::<BTreeMap<_, _>>();
    assert_eq!(
        query.get("client_id").map(std::convert::AsRef::as_ref),
        Some("123456789")
    );
    assert_eq!(
        query.get("scope").map(std::convert::AsRef::as_ref),
        Some(BASIC_READ_SCOPE)
    );
    assert!(query.contains_key("state"));
    assert!(!begun.authorization_url.contains("publish"));
    assert!(!begun.authorization_url.contains("manage_messages"));
    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn begin_stores_state_hash_not_raw_state() {
    let test = TestDatabase::create().await.expect("fresh database");
    let begun = begin(&test, false).await;
    let url = reqwest::Url::parse(&begun.authorization_url).expect("authorization URL parses");
    let state = url
        .query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.into_owned())
        .expect("state exists");
    let stored: Vec<u8> = sqlx::query_scalar(
        "select state_hash from instagram_archive.oauth_flows where flow_id = $1",
    )
    .bind(begun.flow_id)
    .fetch_one(test.database.pool())
    .await
    .expect("flow loads");
    assert_ne!(stored, state.as_bytes());
    assert_eq!(stored, Sha256::digest(state.as_bytes()).as_slice());
    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn begin_encrypts_pkce_verifier_when_supported() {
    let test = TestDatabase::create().await.expect("fresh database");
    let begun = begin(&test, true).await;
    let row: (Vec<u8>, i32) = sqlx::query_as(
        "select pkce_verifier_envelope, key_version from instagram_archive.oauth_flows
         where flow_id = $1",
    )
    .bind(begun.flow_id)
    .fetch_one(test.database.pool())
    .await
    .expect("flow loads");
    assert_eq!(row.1, 7);
    let opened = keyring()
        .open(
            TokenBinding {
                subject_id: begun.flow_id,
                kind: TokenKind::PkceVerifier,
            },
            &row.0,
        )
        .expect("PKCE envelope authenticates");
    assert!(
        !row.0
            .windows(opened.expose_secret().len())
            .any(|part| { part == opened.expose_secret().as_bytes() })
    );
    let url = reqwest::Url::parse(&begun.authorization_url).expect("authorization URL parses");
    let challenge = url
        .query_pairs()
        .find(|(key, _)| key == "code_challenge")
        .map(|(_, value)| value.into_owned())
        .expect("challenge exists");
    let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(opened.expose_secret().as_bytes()));
    assert_eq!(challenge, expected);
    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn begin_flow_expires_at_the_configured_deadline() {
    let test = TestDatabase::create().await.expect("fresh database");
    let begun = begin(&test, false).await;
    let expires_at: OffsetDateTime = sqlx::query_scalar(
        "select expires_at from instagram_archive.oauth_flows where flow_id = $1",
    )
    .bind(begun.flow_id)
    .fetch_one(test.database.pool())
    .await
    .expect("flow loads");
    assert_eq!(
        expires_at,
        OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(600)
    );
    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn unknown_expired_owner_mismatched_redirect_mismatched_and_replayed_state_persist_nothing() {
    let test = TestDatabase::create().await.expect("fresh database");
    let (user_ref, provider, relay, relay_id, _) = prepared_completion(&test).await;
    let result = test
        .database
        .complete_official_oauth(
            &keyring(),
            &provider,
            &relay,
            Uuid::now_v7(),
            relay_id,
            "https://platform.example.test/v1/oauth/callback/instagram",
            5,
            1,
            OffsetDateTime::UNIX_EPOCH,
        )
        .await;
    assert!(result.is_err());
    let accounts: i64 = sqlx::query_scalar("select count(*) from instagram_archive.accounts")
        .fetch_one(test.database.pool())
        .await
        .expect("account count answers");
    assert_eq!(accounts, 0);
    assert_ne!(user_ref, Uuid::nil());
    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn successful_completion_claims_relay_once_and_consumes_flow() {
    let test = TestDatabase::create().await.expect("fresh database");
    let (user_ref, provider, relay, relay_id, _) = prepared_completion(&test).await;
    let connected = test
        .database
        .complete_official_oauth(
            &keyring(),
            &provider,
            &relay,
            user_ref,
            relay_id,
            "https://platform.example.test/v1/oauth/callback/instagram",
            5,
            1,
            OffsetDateTime::UNIX_EPOCH,
        )
        .await
        .expect("completion succeeds");
    assert_eq!(connected.provider_account_id, "17841400000000000");
    let replay_result = test
        .database
        .complete_official_oauth(
            &keyring(),
            &provider,
            &relay,
            user_ref,
            relay_id,
            "https://platform.example.test/v1/oauth/callback/instagram",
            5,
            1,
            OffsetDateTime::UNIX_EPOCH,
        )
        .await;
    assert!(replay_result.is_err());
    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn exchange_failure_consumes_flow_without_connection() {
    let test = TestDatabase::create().await.expect("fresh database");
    let user_ref = Uuid::from_u128(0x018f_1a2b_3c4d_7e6f_8a9b_0c1d_2e3f_4a5b);
    let begun = begin(&test, false).await;
    let url = reqwest::Url::parse(&begun.authorization_url).expect("URL parses");
    let state = url
        .query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.into_owned())
        .expect("state exists");
    let relay = FakeOAuthCodeRelay::default();
    relay
        .insert(
            "exchange-failure".to_owned(),
            RelayClaim {
                user_ref,
                state: SecretString::from(state),
                authorization_code: SecretString::from("SYNTHETIC_FAILED_CODE"),
                redirect_uri: "https://platform.example.test/v1/oauth/callback/instagram"
                    .to_owned(),
            },
        )
        .expect("relay inserts");
    let provider = FakeInstagramProvider::new([FakeProviderStep::Exchange(Err(
        ratatoskr_instagram_archive::provider::ProviderError {
            class: ratatoskr_instagram_archive::provider::ProviderFailureClass::Authentication,
            http_status: Some(401),
        },
    ))]);
    let result = test
        .database
        .complete_official_oauth(
            &keyring(),
            &provider,
            &relay,
            user_ref,
            "exchange-failure",
            "https://platform.example.test/v1/oauth/callback/instagram",
            5,
            1,
            OffsetDateTime::UNIX_EPOCH,
        )
        .await;
    assert!(result.is_err());
    let consumed: Option<OffsetDateTime> = sqlx::query_scalar(
        "select consumed_at from instagram_archive.oauth_flows where flow_id = $1",
    )
    .bind(begun.flow_id)
    .fetch_one(test.database.pool())
    .await
    .expect("flow loads");
    assert!(consumed.is_some());
    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn successful_completion_stores_no_plaintext_and_links_raw_discovery_evidence() {
    let test = TestDatabase::create().await.expect("fresh database");
    let (user_ref, provider, relay, relay_id, token) = prepared_completion(&test).await;
    let connected = test
        .database
        .complete_official_oauth(
            &keyring(),
            &provider,
            &relay,
            user_ref,
            relay_id,
            "https://platform.example.test/v1/oauth/callback/instagram",
            5,
            1,
            OffsetDateTime::UNIX_EPOCH,
        )
        .await
        .expect("completion succeeds");
    let envelope: Vec<u8> = sqlx::query_scalar(
        "select access_token_envelope from instagram_archive.credentials where account_id = $1",
    )
    .bind(connected.account_id)
    .fetch_one(test.database.pool())
    .await
    .expect("credential loads");
    assert!(
        !envelope
            .windows(token.len())
            .any(|part| part == token.as_bytes())
    );
    let evidence: i64 = sqlx::query_scalar(
        "select count(distinct raw_record_id)
         from instagram_archive.account_permission_observations where account_id = $1",
    )
    .bind(connected.account_id)
    .fetch_one(test.database.pool())
    .await
    .expect("evidence linkage count answers");
    assert_eq!(evidence, 1);
    test.cleanup().await.expect("cleanup drops");
}

#[tokio::test]
async fn successful_completion_accounts_exchange_identity_and_permission_calls() {
    let test = TestDatabase::create().await.expect("fresh database");
    let (user_ref, provider, relay, relay_id, _) = prepared_completion(&test).await;
    test.database
        .complete_official_oauth(
            &keyring(),
            &provider,
            &relay,
            user_ref,
            relay_id,
            "https://platform.example.test/v1/oauth/callback/instagram",
            5,
            1,
            OffsetDateTime::UNIX_EPOCH,
        )
        .await
        .expect("completion succeeds");
    let attempts: Vec<String> = sqlx::query_scalar(
        "select request_class from instagram_archive.provider_api_usage order by attempt_ordinal",
    )
    .fetch_all(test.database.pool())
    .await
    .expect("usage rows load");
    assert_eq!(
        attempts,
        ["code_exchange", "account_discovery", "permission_discovery"]
    );
    test.cleanup().await.expect("cleanup drops");
}
