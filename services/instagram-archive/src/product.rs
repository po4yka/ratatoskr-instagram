//! The product plane: explicit capture intake for platform callers.
//!
//! One route today — `POST /v1/captures` speaking the documented platform
//! capture grammar. Refusals are typed JSON bodies (`{"error": code}`), never
//! driver messages or internal details: a caller needs the class of the
/// refusal to fix its request, and nothing else.
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use metrics::counter;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use ratatoskr_instagram_archive::Database;
use ratatoskr_instagram_archive::capture::{
    CaptureError, CaptureRequest, CaptureSubmission, ClientSource,
};

/// The only platform this bounded context accepts.
const PLATFORM: &str = "instagram";

struct ProductState {
    database: Database,
}

/// Builds the product router serving the capture intake.
pub fn product_router(database: Database) -> Router {
    let state = Arc::new(ProductState { database });
    Router::new()
        .route("/v1/captures", post(capture_intake))
        .with_state(state)
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
