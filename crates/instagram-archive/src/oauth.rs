//! Owner-bound, single-use official Instagram OAuth flows.

use std::time::Duration;

use base64::Engine as _;
use rand::RngCore as _;
use secrecy::SecretString;
use sha2::{Digest as _, Sha256};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::Database;
use crate::capability_reconciliation::{
    AccountObservation, AccountType, CapabilityPersistenceError, PermissionStatus,
    StoredAccountCapability,
};
use crate::credentials::crypto::{CredentialKeyring, CryptoError, TokenBinding, TokenKind};
use crate::provider::{AUTHORIZE_ENDPOINT, BASIC_READ_SCOPE, ReqwestInstagramProvider};
use crate::provider::{
    InstagramProvider, OAuthCodeRelay, ProviderError, ProviderFailureClass, RelayError,
};
use crate::provider_budget::{BudgetError, MetaUsage, ProviderBudget, RequestClass, UsageOutcome};

/// Authorization begin response; raw state exists only inside the URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthBegin {
    /// Local flow identity.
    pub flow_id: Uuid,
    /// Provider authorization URL for the user's browser.
    pub authorization_url: String,
}

/// Successful connected-account result without credential material.
#[derive(Debug)]
pub struct ConnectedOfficialAccount {
    /// Local account identity.
    pub account_id: Uuid,
    /// Stable provider identity.
    pub provider_account_id: String,
    /// Provider-observed account type.
    pub account_type: AccountType,
    /// Complete latest capability matrix.
    pub capabilities: Vec<StoredAccountCapability>,
}

/// OAuth flow failure with no credential or callback data in Display.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum OAuthError {
    /// Flow is absent, expired, consumed, or binding did not match.
    #[error("official Instagram OAuth flow is unavailable")]
    Unavailable,
    /// Provider authorization URL configuration is invalid.
    #[error("official Instagram OAuth configuration is invalid")]
    Configuration,
    /// Secret envelope failed.
    #[error("official Instagram OAuth secret handling failed")]
    Crypto(#[source] CryptoError),
    /// Owned flow persistence failed.
    #[error("official Instagram OAuth persistence failed")]
    Database(#[source] sqlx::Error),
    /// Platform code relay could not be claimed safely.
    #[error("official Instagram OAuth relay failed")]
    Relay(#[source] RelayError),
    /// Provider exchange or discovery failed.
    #[error("official Instagram provider operation failed")]
    Provider(#[source] ProviderError),
    /// Finite provider attempt budget could not admit or record a call.
    #[error("official Instagram provider budget failed")]
    Budget(#[source] BudgetError),
    /// Capability evidence/projection could not be committed.
    #[error("official Instagram capability reconciliation failed")]
    Capability(#[source] CapabilityPersistenceError),
}

impl Database {
    /// Begins one owner-bound provider authorization.
    ///
    /// # Errors
    ///
    /// Returns [`OAuthError`] when secure material, URL construction, or persistence fails.
    #[allow(
        clippy::too_many_arguments,
        reason = "begin binds the complete owner, redirect, crypto, lifetime, provider-profile, and clock policy"
    )]
    pub async fn begin_official_oauth(
        &self,
        keyring: &CredentialKeyring,
        user_ref: Uuid,
        client_id: &str,
        redirect_uri: &str,
        flow_ttl: Duration,
        pkce_supported: bool,
        now: OffsetDateTime,
    ) -> Result<OAuthBegin, OAuthError> {
        let flow_id = Uuid::now_v7();
        let mut state_bytes = [0_u8; 32];
        rand::rng().fill_bytes(&mut state_bytes);
        let state = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(state_bytes);
        let state_hash = Sha256::digest(state.as_bytes()).to_vec();
        let redirect_hash = Sha256::digest(redirect_uri.as_bytes()).to_vec();
        let ttl_seconds =
            i64::try_from(flow_ttl.as_secs()).map_err(|_| OAuthError::Configuration)?;
        let expires_at = now + time::Duration::seconds(ttl_seconds);
        let mut authorization_url =
            reqwest::Url::parse(AUTHORIZE_ENDPOINT).map_err(|_| OAuthError::Configuration)?;
        authorization_url
            .query_pairs_mut()
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("response_type", "code")
            .append_pair("scope", BASIC_READ_SCOPE)
            .append_pair("state", &state);
        let (pkce_verifier_envelope, key_version) = if pkce_supported {
            let mut verifier_bytes = [0_u8; 32];
            rand::rng().fill_bytes(&mut verifier_bytes);
            let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(verifier_bytes);
            let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(Sha256::digest(verifier.as_bytes()));
            authorization_url
                .query_pairs_mut()
                .append_pair("code_challenge", &challenge)
                .append_pair("code_challenge_method", "S256");
            let envelope = keyring
                .seal(
                    TokenBinding {
                        subject_id: flow_id,
                        kind: TokenKind::PkceVerifier,
                    },
                    &SecretString::from(verifier),
                )
                .map_err(OAuthError::Crypto)?;
            let version =
                i32::try_from(keyring.current_version()).map_err(|_| OAuthError::Configuration)?;
            (Some(envelope), Some(version))
        } else {
            (None, None)
        };
        sqlx::query(
            "insert into instagram_archive.oauth_flows
             (flow_id, user_ref, state_hash, redirect_uri_hash, pkce_verifier_envelope,
              key_version, expires_at, created_at)
             values ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(flow_id)
        .bind(user_ref)
        .bind(state_hash)
        .bind(redirect_hash)
        .bind(pkce_verifier_envelope)
        .bind(key_version)
        .bind(expires_at)
        .bind(now)
        .execute(self.pool())
        .await
        .map_err(OAuthError::Database)?;
        Ok(OAuthBegin {
            flow_id,
            authorization_url: authorization_url.into(),
        })
    }

    /// Claims one Platform relay and completes the owner-bound connection.
    ///
    /// # Errors
    ///
    /// Returns [`OAuthError`] for relay/binding/replay/provider/budget/persistence failures.
    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "completion keeps the single-use claim and credential commit sequence visible as one security boundary"
    )]
    pub async fn complete_official_oauth(
        &self,
        keyring: &CredentialKeyring,
        provider: &dyn InstagramProvider,
        relay: &dyn OAuthCodeRelay,
        user_ref: Uuid,
        relay_id: &str,
        redirect_uri: &str,
        call_budget: u32,
        discovery_retries: u32,
        now: OffsetDateTime,
    ) -> Result<ConnectedOfficialAccount, OAuthError> {
        let claim = relay.claim(relay_id).await.map_err(OAuthError::Relay)?;
        if claim.user_ref != user_ref || claim.redirect_uri != redirect_uri {
            return Err(OAuthError::Unavailable);
        }
        let state_hash =
            Sha256::digest(secrecy::ExposeSecret::expose_secret(&claim.state).as_bytes()).to_vec();
        let redirect_hash = Sha256::digest(redirect_uri.as_bytes()).to_vec();
        let flow_id: Option<Uuid> = sqlx::query_scalar(
            "update instagram_archive.oauth_flows
             set consumed_at = $4
             where user_ref = $1 and state_hash = $2 and redirect_uri_hash = $3
               and consumed_at is null and expires_at > $4
             returning flow_id",
        )
        .bind(user_ref)
        .bind(state_hash)
        .bind(redirect_hash)
        .bind(now)
        .fetch_optional(self.pool())
        .await
        .map_err(OAuthError::Database)?;
        let Some(_flow_id) = flow_id else {
            return Err(OAuthError::Unavailable);
        };

        let operation_id = Uuid::now_v7();
        let mut budget = ProviderBudget::new(self.clone(), operation_id, None, call_budget);
        let exchange_reservation = budget
            .reserve(RequestClass::CodeExchange, now)
            .await
            .map_err(OAuthError::Budget)?;
        let exchanged = provider.exchange_code(&claim.authorization_code).await;
        finish_attempt(&budget, exchange_reservation, exchanged.as_ref().err(), now).await?;
        let exchanged = exchanged.map_err(OAuthError::Provider)?;

        let mut account_retries = discovery_retries;
        let provider_account = loop {
            let reservation = budget
                .reserve(RequestClass::AccountDiscovery, now)
                .await
                .map_err(OAuthError::Budget)?;
            let result = provider.discover_account(&exchanged.access_token).await;
            finish_attempt(&budget, reservation, result.as_ref().err(), now).await?;
            match result {
                Ok(account) => break account,
                Err(error)
                    if account_retries > 0
                        && ReqwestInstagramProvider::should_retry(
                            RequestClass::AccountDiscovery,
                            error.class,
                        ) =>
                {
                    account_retries -= 1;
                }
                Err(error) => return Err(OAuthError::Provider(error)),
            }
        };

        let mut permission_retries = discovery_retries;
        let permissions = loop {
            let reservation = budget
                .reserve(RequestClass::PermissionDiscovery, now)
                .await
                .map_err(OAuthError::Budget)?;
            let result = provider.discover_permissions(&exchanged.access_token).await;
            finish_attempt(&budget, reservation, result.as_ref().err(), now).await?;
            match result {
                Ok(permissions) => break permissions,
                Err(error)
                    if permission_retries > 0
                        && ReqwestInstagramProvider::should_retry(
                            RequestClass::PermissionDiscovery,
                            error.class,
                        ) =>
                {
                    permission_retries -= 1;
                }
                Err(error) => return Err(OAuthError::Provider(error)),
            }
        };

        if exchanged.user_id != provider_account.provider_account_id {
            return Err(OAuthError::Unavailable);
        }
        let existing: Option<(Uuid, Uuid)> = sqlx::query_as(
            "select account_id, user_ref from instagram_archive.accounts
             where provider_account_id = $1",
        )
        .bind(&provider_account.provider_account_id)
        .fetch_optional(self.pool())
        .await
        .map_err(OAuthError::Database)?;
        let candidate_account_id = match existing {
            Some((account_id, owner)) if owner == user_ref => account_id,
            Some(_) => return Err(OAuthError::Unavailable),
            None => Uuid::now_v7(),
        };
        let account_body = serde_json::to_vec(&serde_json::json!({
            "id": &provider_account.provider_account_id,
            "username": &provider_account.username,
            "account_type": account_type_wire(provider_account.account_type),
        }))
        .map_err(|_| OAuthError::Configuration)?;
        let permission_body = serde_json::to_vec(&serde_json::json!({
            "data": permissions.statuses.iter().map(|(permission, status)| {
                serde_json::json!({
                    "permission": permission,
                    "status": permission_status_wire(*status),
                })
            }).collect::<Vec<_>>()
        }))
        .map_err(|_| OAuthError::Configuration)?;
        let mut transaction = self.pool().begin().await.map_err(OAuthError::Database)?;
        let _account_raw = insert_raw_record(&mut transaction, &account_body, now).await?;
        let permission_raw = insert_raw_record(&mut transaction, &permission_body, now).await?;
        let granted_permissions = permissions
            .statuses
            .iter()
            .filter(|(_, status)| **status == PermissionStatus::Granted)
            .map(|(permission, _)| permission.clone())
            .collect::<Vec<_>>();
        let persisted_account_id: Option<Uuid> = sqlx::query_scalar(
            "insert into instagram_archive.accounts as stored
             (account_id, user_ref, provider_account_id, username, account_type,
              connection_status, scopes, connected_at, updated_at)
             values ($1, $2, $3, $4, $5, 'connected', $6, $7, $7)
             on conflict (provider_account_id) do update set
                 username = excluded.username,
                 account_type = excluded.account_type,
                 connection_status = 'connected',
                 scopes = excluded.scopes,
                 updated_at = excluded.updated_at
             where stored.user_ref = excluded.user_ref
             returning stored.account_id",
        )
        .bind(candidate_account_id)
        .bind(user_ref)
        .bind(&provider_account.provider_account_id)
        .bind(&provider_account.username)
        .bind(account_type_wire(provider_account.account_type))
        .bind(&granted_permissions)
        .bind(now)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(OAuthError::Database)?;
        let Some(account_id) = persisted_account_id else {
            return Err(OAuthError::Unavailable);
        };
        let access_envelope = keyring
            .seal(
                TokenBinding {
                    subject_id: account_id,
                    kind: TokenKind::Access,
                },
                &exchanged.access_token,
            )
            .map_err(OAuthError::Crypto)?;
        let key_version =
            i32::try_from(keyring.current_version()).map_err(|_| OAuthError::Configuration)?;
        let expires_at = exchanged
            .expires_in_seconds
            .and_then(|seconds| i64::try_from(seconds).ok())
            .map(|seconds| now + time::Duration::seconds(seconds));
        sqlx::query("delete from instagram_archive.credentials where account_id = $1")
            .bind(account_id)
            .execute(&mut *transaction)
            .await
            .map_err(OAuthError::Database)?;
        sqlx::query(
            "insert into instagram_archive.credentials
             (credential_id, account_id, access_token_envelope, key_version,
              granted_permissions, expires_at, created_at)
             values ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(Uuid::now_v7())
        .bind(account_id)
        .bind(access_envelope)
        .bind(key_version)
        .bind(&granted_permissions)
        .bind(expires_at)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(OAuthError::Database)?;
        sqlx::query(
            "insert into instagram_archive.account_credential_audit
             (audit_id, account_id, change_kind, outcome, detail, occurred_at)
             values ($1, $2, 'authorized', 'succeeded', $3, $4)",
        )
        .bind(Uuid::now_v7())
        .bind(account_id)
        .bind(serde_json::json!({ "provider_profile": "instagram_login" }))
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(OAuthError::Database)?;
        let observation = AccountObservation {
            provider_account_id: provider_account.provider_account_id.clone(),
            account_type: provider_account.account_type,
            permissions: permissions.statuses,
            external_write_consent: false,
            observed_at: now,
            raw_record_id: permission_raw,
        };
        self.reconcile_account_capabilities_in_transaction(
            &mut transaction,
            account_id,
            &observation,
        )
        .await
        .map_err(OAuthError::Capability)?;
        transaction.commit().await.map_err(OAuthError::Database)?;
        let capabilities = self
            .load_account_capabilities(account_id)
            .await
            .map_err(OAuthError::Capability)?;
        Ok(ConnectedOfficialAccount {
            account_id,
            provider_account_id: provider_account.provider_account_id,
            account_type: provider_account.account_type,
            capabilities,
        })
    }
}

async fn finish_attempt(
    budget: &ProviderBudget,
    reservation: crate::provider_budget::UsageReservation,
    error: Option<&ProviderError>,
    now: OffsetDateTime,
) -> Result<(), OAuthError> {
    let outcome = error.map_or(UsageOutcome::Succeeded, |error| {
        provider_usage_outcome(error.class)
    });
    budget
        .complete(
            reservation,
            outcome,
            error.and_then(|error| error.http_status),
            MetaUsage::default(),
            now,
        )
        .await
        .map_err(OAuthError::Budget)
}

const fn provider_usage_outcome(class: ProviderFailureClass) -> UsageOutcome {
    match class {
        ProviderFailureClass::Authentication => UsageOutcome::Authentication,
        ProviderFailureClass::Validation => UsageOutcome::Validation,
        ProviderFailureClass::RateLimited => UsageOutcome::RateLimited,
        ProviderFailureClass::Server => UsageOutcome::Server,
        ProviderFailureClass::Network => UsageOutcome::Network,
        ProviderFailureClass::ResponseRefused => UsageOutcome::ResponseRefused,
        ProviderFailureClass::Unsupported => UsageOutcome::ProviderUnsupported,
    }
}

const fn account_type_wire(account_type: AccountType) -> &'static str {
    match account_type {
        AccountType::Business => "business",
        AccountType::Creator => "creator",
        AccountType::Personal => "personal",
        AccountType::Unknown => "unknown",
    }
}

const fn permission_status_wire(status: PermissionStatus) -> &'static str {
    match status {
        PermissionStatus::Granted => "granted",
        PermissionStatus::Declined => "declined",
        PermissionStatus::Expired => "expired",
        PermissionStatus::Absent => "absent",
        PermissionStatus::Unknown => "unknown",
    }
}

async fn insert_raw_record(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    body: &[u8],
    now: OffsetDateTime,
) -> Result<Uuid, OAuthError> {
    let raw_record_id = Uuid::now_v7();
    let digest = Sha256::digest(body);
    let blob_ref = format!("{digest:x}");
    let byte_size = i64::try_from(body.len()).map_err(|_| OAuthError::Configuration)?;
    sqlx::query(
        "insert into instagram_archive.raw_records
         (raw_record_id, record_kind, blob_ref, content_hash, byte_size, body, observed_at)
         values ($1, 'api_response', $2, $3, $4, $5, $6)",
    )
    .bind(raw_record_id)
    .bind(blob_ref)
    .bind(digest.to_vec())
    .bind(byte_size)
    .bind(body)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(OAuthError::Database)?;
    Ok(raw_record_id)
}
