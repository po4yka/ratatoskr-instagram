//! The product plane: explicit capture intake for platform callers.
//!
//! One route today — `POST /v1/captures` speaking the documented platform
//! capture grammar. Refusals are typed JSON bodies (`{"error": code}`), never
//! driver messages or internal details: a caller needs the class of the
/// refusal to fix its request, and nothing else.
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use metrics::counter;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use ratatoskr_instagram_archive::Database;
use ratatoskr_instagram_archive::account::ProviderRevokeOutcome;
use ratatoskr_instagram_archive::capability_reconciliation::{
    AccountType, CapabilityReason, CapabilityState, StoredAccountCapability,
};
use ratatoskr_instagram_archive::capture::{
    CaptureError, CaptureRequest, CaptureSubmission, ClientSource,
};
use ratatoskr_instagram_archive::credentials::crypto::CredentialKeyring;
use ratatoskr_instagram_archive::provider::{InstagramProvider, OAuthCodeRelay};
use ratatoskr_instagram_archive::telemetry::{
    OAuthOperation, OAuthOutcome, record_oauth_operation,
};

/// The only platform this bounded context accepts.
const PLATFORM: &str = "instagram";

struct ProductState {
    database: Database,
    official: Option<Arc<OfficialAccountRuntime>>,
}

/// Injected official-account dependencies and finite policy.
pub struct OfficialAccountRuntime {
    keyring: CredentialKeyring,
    provider: Arc<dyn InstagramProvider>,
    relay: Arc<dyn OAuthCodeRelay>,
    client_id: String,
    redirect_uri: String,
    flow_ttl: std::time::Duration,
    call_budget: u32,
    discovery_retries: u32,
    pkce_supported: bool,
    refresh_supported: bool,
}

impl std::fmt::Debug for OfficialAccountRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OfficialAccountRuntime")
            .field("keyring", &self.keyring)
            .field("provider", &"[INJECTED]")
            .field("relay", &"[INJECTED]")
            .field("client_id", &self.client_id)
            .field("redirect_uri", &self.redirect_uri)
            .field("flow_ttl", &self.flow_ttl)
            .field("call_budget", &self.call_budget)
            .field("discovery_retries", &self.discovery_retries)
            .field("pkce_supported", &self.pkce_supported)
            .field("refresh_supported", &self.refresh_supported)
            .finish()
    }
}

impl OfficialAccountRuntime {
    /// Creates enabled official-account route dependencies.
    #[must_use]
    #[allow(
        clippy::too_many_arguments,
        reason = "constructor mirrors one closed operator policy"
    )]
    pub fn new(
        keyring: CredentialKeyring,
        provider: Arc<dyn InstagramProvider>,
        relay: Arc<dyn OAuthCodeRelay>,
        client_id: String,
        redirect_uri: String,
        flow_ttl: std::time::Duration,
        call_budget: u32,
        discovery_retries: u32,
        pkce_supported: bool,
        refresh_supported: bool,
    ) -> Self {
        Self {
            keyring,
            provider,
            relay,
            client_id,
            redirect_uri,
            flow_ttl,
            call_budget,
            discovery_retries,
            pkce_supported,
            refresh_supported,
        }
    }
}

/// Builds the product router serving the capture intake.
pub fn product_router(database: Database) -> Router {
    product_router_with_official_accounts(database, None)
}

/// Builds the product router with optional disabled-by-default official-account commands.
pub fn product_router_with_official_accounts(
    database: Database,
    official: Option<OfficialAccountRuntime>,
) -> Router {
    let state = Arc::new(ProductState {
        database,
        official: official.map(Arc::new),
    });
    Router::new()
        .route("/v1/captures", post(capture_intake))
        .route("/v1/accounts/instagram/oauth/begin", post(oauth_begin))
        .route(
            "/v1/accounts/instagram/oauth/complete",
            post(oauth_complete),
        )
        .route(
            "/v1/accounts/instagram/{account_id}/refresh",
            post(account_refresh),
        )
        .route(
            "/v1/accounts/instagram/{account_id}/capabilities",
            get(account_capabilities),
        )
        .route(
            "/v1/accounts/instagram/{account_id}/revoke",
            post(account_revoke),
        )
        .with_state(state)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerCommand {
    user_ref: Uuid,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompleteCommand {
    user_ref: Uuid,
    relay_id: String,
}

async fn oauth_begin(
    State(state): State<Arc<ProductState>>,
    body: Result<Json<OwnerCommand>, JsonRejection>,
) -> Response {
    let Some(official) = state.official.as_ref() else {
        return oauth_refusal(
            OAuthOperation::Begin,
            OAuthOutcome::Unavailable,
            StatusCode::SERVICE_UNAVAILABLE,
            "oauth_unavailable",
        );
    };
    let Ok(Json(body)) = body else {
        return oauth_refusal(
            OAuthOperation::Begin,
            OAuthOutcome::Invalid,
            StatusCode::BAD_REQUEST,
            "invalid_request",
        );
    };
    match state
        .database
        .begin_official_oauth(
            &official.keyring,
            body.user_ref,
            &official.client_id,
            &official.redirect_uri,
            official.flow_ttl,
            official.pkce_supported,
            OffsetDateTime::now_utc(),
        )
        .await
    {
        Ok(begun) => {
            record_oauth_operation(OAuthOperation::Begin, OAuthOutcome::Succeeded);
            Json(serde_json::json!({
                "flow_id": begun.flow_id,
                "authorization_url": begun.authorization_url,
            }))
            .into_response()
        }
        Err(error) => {
            tracing::error!(error_class = "oauth_begin", %error, "official OAuth begin failed");
            oauth_refusal(
                OAuthOperation::Begin,
                OAuthOutcome::Internal,
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
            )
        }
    }
}

async fn oauth_complete(
    State(state): State<Arc<ProductState>>,
    body: Result<Json<CompleteCommand>, JsonRejection>,
) -> Response {
    let Some(official) = state.official.as_ref() else {
        return oauth_refusal(
            OAuthOperation::Complete,
            OAuthOutcome::Unavailable,
            StatusCode::SERVICE_UNAVAILABLE,
            "oauth_unavailable",
        );
    };
    let Ok(Json(body)) = body else {
        return oauth_refusal(
            OAuthOperation::Complete,
            OAuthOutcome::Invalid,
            StatusCode::BAD_REQUEST,
            "invalid_request",
        );
    };
    match state
        .database
        .complete_official_oauth(
            &official.keyring,
            official.provider.as_ref(),
            official.relay.as_ref(),
            body.user_ref,
            &body.relay_id,
            &official.redirect_uri,
            official.call_budget,
            official.discovery_retries,
            OffsetDateTime::now_utc(),
        )
        .await
    {
        Ok(account) => {
            record_oauth_operation(OAuthOperation::Complete, OAuthOutcome::Succeeded);
            Json(serde_json::json!({
                "account_id": account.account_id,
                "provider_account_id": account.provider_account_id,
                "account_type": account_type_wire(account.account_type),
                "capabilities": capability_json(&account.capabilities),
            }))
            .into_response()
        }
        Err(error) => {
            tracing::warn!(error_class = "oauth_complete", %error, "official OAuth completion refused");
            oauth_refusal(
                OAuthOperation::Complete,
                OAuthOutcome::Upstream,
                StatusCode::BAD_REQUEST,
                "oauth_completion_failed",
            )
        }
    }
}

async fn account_refresh(
    State(state): State<Arc<ProductState>>,
    Path(account_id): Path<Uuid>,
    body: Result<Json<OwnerCommand>, JsonRejection>,
) -> Response {
    let Some(official) = state.official.as_ref() else {
        return oauth_refusal(
            OAuthOperation::Refresh,
            OAuthOutcome::Unavailable,
            StatusCode::SERVICE_UNAVAILABLE,
            "oauth_unavailable",
        );
    };
    let Ok(Json(body)) = body else {
        return oauth_refusal(
            OAuthOperation::Refresh,
            OAuthOutcome::Invalid,
            StatusCode::BAD_REQUEST,
            "invalid_request",
        );
    };
    match state
        .database
        .refresh_official_account(
            &official.keyring,
            official.provider.as_ref(),
            account_id,
            body.user_ref,
            official.call_budget,
            official.discovery_retries,
            official.refresh_supported,
            OffsetDateTime::now_utc(),
        )
        .await
    {
        Ok(capabilities) => {
            record_oauth_operation(OAuthOperation::Refresh, OAuthOutcome::Succeeded);
            Json(serde_json::json!({
                "account_id": account_id,
                "capabilities": capability_json(&capabilities),
            }))
            .into_response()
        }
        Err(error) => {
            tracing::warn!(error_class = "account_refresh", %error, "official account refresh refused");
            oauth_refusal(
                OAuthOperation::Refresh,
                OAuthOutcome::Upstream,
                StatusCode::BAD_REQUEST,
                "refresh_failed",
            )
        }
    }
}

async fn account_capabilities(
    State(state): State<Arc<ProductState>>,
    Path(account_id): Path<Uuid>,
    query: Result<Query<OwnerCommand>, axum::extract::rejection::QueryRejection>,
) -> Response {
    if state.official.is_none() {
        return oauth_refusal(
            OAuthOperation::Capabilities,
            OAuthOutcome::Unavailable,
            StatusCode::SERVICE_UNAVAILABLE,
            "oauth_unavailable",
        );
    }
    let Ok(Query(query)) = query else {
        return oauth_refusal(
            OAuthOperation::Capabilities,
            OAuthOutcome::Invalid,
            StatusCode::BAD_REQUEST,
            "invalid_request",
        );
    };
    match state
        .database
        .official_account_owned_by(account_id, query.user_ref)
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            return oauth_refusal(
                OAuthOperation::Capabilities,
                OAuthOutcome::Unavailable,
                StatusCode::NOT_FOUND,
                "account_unavailable",
            );
        }
        Err(error) => {
            tracing::error!(error_class = "capability_load", %error, "account ownership query failed");
            return oauth_refusal(
                OAuthOperation::Capabilities,
                OAuthOutcome::Internal,
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
            );
        }
    }
    match state.database.load_account_capabilities(account_id).await {
        Ok(capabilities) => {
            record_oauth_operation(OAuthOperation::Capabilities, OAuthOutcome::Succeeded);
            Json(serde_json::json!({
                "account_id": account_id,
                "capabilities": capability_json(&capabilities),
            }))
            .into_response()
        }
        Err(error) => {
            tracing::error!(error_class = "capability_load", %error, "capability projection load failed");
            oauth_refusal(
                OAuthOperation::Capabilities,
                OAuthOutcome::Internal,
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
            )
        }
    }
}

async fn account_revoke(
    State(state): State<Arc<ProductState>>,
    Path(account_id): Path<Uuid>,
    body: Result<Json<OwnerCommand>, JsonRejection>,
) -> Response {
    if state.official.is_none() {
        return oauth_refusal(
            OAuthOperation::Revoke,
            OAuthOutcome::Unavailable,
            StatusCode::SERVICE_UNAVAILABLE,
            "oauth_unavailable",
        );
    }
    let Ok(Json(body)) = body else {
        return oauth_refusal(
            OAuthOperation::Revoke,
            OAuthOutcome::Invalid,
            StatusCode::BAD_REQUEST,
            "invalid_request",
        );
    };
    match state
        .database
        .scrub_revoked_account(
            account_id,
            body.user_ref,
            ProviderRevokeOutcome::Unsupported,
            OffsetDateTime::now_utc(),
        )
        .await
    {
        Ok(()) => {
            record_oauth_operation(OAuthOperation::Revoke, OAuthOutcome::Succeeded);
            Json(serde_json::json!({
                "account_id": account_id,
                "status": "revoked",
            }))
            .into_response()
        }
        Err(error) => {
            tracing::warn!(error_class = "account_revoke", %error, "official account revoke refused");
            oauth_refusal(
                OAuthOperation::Revoke,
                OAuthOutcome::Unavailable,
                StatusCode::NOT_FOUND,
                "account_unavailable",
            )
        }
    }
}

fn capability_json(capabilities: &[StoredAccountCapability]) -> Vec<serde_json::Value> {
    capabilities
        .iter()
        .map(|row| {
            serde_json::json!({
                "capability": row.reconciled.capability.wire_value(),
                "state": capability_state_wire(row.reconciled.state),
                "reason": capability_reason_wire(row.reconciled.reason),
                "generation_id": row.generation_id,
            })
        })
        .collect()
}

const fn account_type_wire(account_type: AccountType) -> &'static str {
    match account_type {
        AccountType::Business => "business",
        AccountType::Creator => "creator",
        AccountType::Personal => "personal",
        AccountType::Unknown => "unknown",
    }
}

const fn capability_state_wire(state: CapabilityState) -> &'static str {
    match state {
        CapabilityState::Available => "available",
        CapabilityState::Unavailable => "unavailable",
        CapabilityState::NotSupported => "not_supported",
    }
}

const fn capability_reason_wire(reason: CapabilityReason) -> &'static str {
    match reason {
        CapabilityReason::Granted => "granted",
        CapabilityReason::AccountTypeUnsupported => "account_type_unsupported",
        CapabilityReason::PermissionDeclined => "permission_declined",
        CapabilityReason::PermissionExpired => "permission_expired",
        CapabilityReason::PermissionAbsent => "permission_absent",
        CapabilityReason::PermissionUnknown => "permission_unknown",
        CapabilityReason::MissingPermission => "missing_permission",
        CapabilityReason::WriteConsentRequired => "write_consent_required",
        CapabilityReason::ProviderNotSupported => "provider_not_supported",
        CapabilityReason::Revoked => "revoked",
        CapabilityReason::ReauthorizationRequired => "reauthorization_required",
    }
}

/// One grammar-shaped capture submission.
///
/// Field names follow the platform capture grammar: `canonical_url` carries
/// whatever form the client delivered — canonicalization happens here — and
/// `source` names the delivering Ratatoskr client.
#[derive(Debug, Deserialize)]
struct CaptureIntakeBody {
    /// The Ratatoskr user acting; minted by the platform, trusted from this hop.
    user_ref: Uuid,
    /// Must name this platform.
    platform: String,
    /// The delivered URL, any accepted permalink form.
    canonical_url: String,
    /// When the user performed the save, RFC 3339.
    #[serde(with = "time::serde::rfc3339")]
    captured_at: OffsetDateTime,
    /// The delivering Ratatoskr client.
    source: String,
    /// Optional private user note.
    note: Option<String>,
}

/// The intake answer for either outcome of a submission.
#[derive(Debug, Serialize)]
struct CaptureIntakeResponse {
    /// The local identity of the capture.
    capture_id: Uuid,
    /// The canonical permalink stored for the capture.
    canonical_url: String,
    /// Lifecycle status, always `accepted` for a fresh intake answer.
    status: &'static str,
    /// The wire acquisition method implied by the client source.
    acquisition_method: String,
    /// Always `explicit_user_capture`: what an explicit capture proves.
    saved_authority: String,
    /// The wire value of the delivering client source.
    client_source: String,
    /// The preserved save time; reuse hands back the original instant.
    #[serde(with = "time::serde::rfc3339")]
    captured_at: OffsetDateTime,
    /// Whether an earlier capture was reused instead of created.
    reused: bool,
}

/// A structured refusal body. The code is stable and machine-readable.
#[derive(Debug, Serialize)]
struct Refusal {
    /// The refusal class.
    error: &'static str,
}

/// `POST /v1/captures`.
async fn capture_intake(
    State(state): State<Arc<ProductState>>,
    headers: HeaderMap,
    body: Result<Json<CaptureIntakeBody>, JsonRejection>,
) -> Response {
    let Ok(Json(body)) = body else {
        return refusal(StatusCode::BAD_REQUEST, "invalid_request");
    };

    if body.platform != PLATFORM {
        return refusal(StatusCode::BAD_REQUEST, "unknown_platform");
    }
    let Some(client_source) = ClientSource::parse(&body.source) else {
        return refusal(StatusCode::BAD_REQUEST, "unsupported_client_source");
    };

    let request = CaptureRequest {
        user_ref: body.user_ref,
        url: body.canonical_url,
        captured_at: body.captured_at,
        client_source,
        note: body.note,
        client_idempotency_key: header_value(&headers, "idempotency-key"),
    };

    match state.database.submit_capture(&request).await {
        Ok(submission) => {
            record_outcome(submission.is_reuse());
            answered(&submission)
        }
        Err(CaptureError::InvalidUrl(_)) => {
            counter!("instagram_capture_rejected_total").increment(1);
            refusal(StatusCode::BAD_REQUEST, "unsupported_url")
        }
        Err(CaptureError::UnsupportedClientSource) => {
            counter!("instagram_capture_rejected_total").increment(1);
            refusal(StatusCode::BAD_REQUEST, "unsupported_client_source")
        }
        // Everything else — persistence failures and any variant added later
        // — stays unclassified outside: no driver text, no schema details,
        // nothing crosses the boundary. Typed reasons go to operators' logs.
        Err(error) => {
            tracing::error!(%error, "the capture intake could not persist the submission");
            refusal(StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
        }
    }
}

/// Renders the intake answer with the status code the outcome earns.
fn answered(submission: &CaptureSubmission) -> Response {
    let reused = submission.is_reuse();
    let record = submission.record();
    let status = if reused {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    (
        status,
        Json(CaptureIntakeResponse {
            capture_id: record.capture_id,
            canonical_url: record.canonical_url.clone(),
            status: record.status.wire_value(),
            acquisition_method: record.acquisition_method.clone(),
            saved_authority: record.saved_authority.clone(),
            client_source: record.client_source.clone(),
            captured_at: record.captured_at,
            reused,
        }),
    )
        .into_response()
}

/// A machine-readable refusal with no internal detail.
fn refusal(status: StatusCode, code: &'static str) -> Response {
    (status, Json(Refusal { error: code })).into_response()
}

fn oauth_refusal(
    operation: OAuthOperation,
    outcome: OAuthOutcome,
    status: StatusCode,
    code: &'static str,
) -> Response {
    record_oauth_operation(operation, outcome);
    refusal(status, code)
}

/// Reads a bounded header value, or `None` when absent or over-long.
fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    const MAX_HEADER_LEN: usize = 256;
    let value = headers.get(name)?;
    let value = value.to_str().ok()?;
    (value.len() <= MAX_HEADER_LEN && !value.trim().is_empty()).then(|| value.trim().to_owned())
}

/// Intake outcomes land in exactly two counters, named for what happened.
fn record_outcome(reused: bool) {
    if reused {
        counter!("instagram_capture_deduplicated_total").increment(1);
    } else {
        counter!("instagram_capture_accepted_total").increment(1);
    }
}
