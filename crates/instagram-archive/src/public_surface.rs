//! The production public-resolution surface: Meta's `instagram_oembed` Graph endpoint
//! (XR-021 CONTRACTS.md S10 CD5).
//!
//! [`HttpPublicSurface`] implements [`PublicSurface`] over reqwest with redirects disabled, a
//! 3 s connect and 10 s total deadline, and a bounded body read. The status table lives in the
//! one pure function [`classify_response`], so correcting it against recorded live evidence is a
//! one-line change. The access token is a [`SecretString`]: it is redacted in `Debug`, it is never
//! logged, and no error or log line carries the request URL (which embeds it).

use std::time::Duration;

use reqwest::redirect::Policy;
use secrecy::{ExposeSecret as _, SecretString};

use crate::permalink::CanonicalPermalink;
use crate::resolution::{PublicSurface, SurfaceOutcome};

/// Largest response body the surface will read, in bytes.
pub const MAX_BODY_BYTES: usize = 256 * 1024;

/// TCP/TLS connection deadline.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// End-to-end deadline of one request, body included.
const TOTAL_TIMEOUT: Duration = Duration::from_secs(10);

/// Counter of responses that rejected the service credential (HTTP 401).
pub const CREDENTIAL_REJECTED_TOTAL: &str = "instagram_public_resolution_credential_rejected_total";

/// Why the surface could not be built. Messages never carry the endpoint or the token.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PublicSurfaceError {
    /// The configured endpoint is not an absolute URL.
    #[error("the public-resolution endpoint is not a valid URL")]
    InvalidEndpoint,
    /// The HTTP client could not be constructed.
    #[error("the public-resolution HTTP client could not be built")]
    Client,
}

/// Meta `instagram_oembed` over HTTPS.
///
/// Host and scheme policy (https on `graph.facebook.com` or `graph.instagram.com`) is enforced
/// by [`crate::config::PublicResolutionConfig`], the only production path to this constructor.
pub struct HttpPublicSurface {
    client: reqwest::Client,
    endpoint: reqwest::Url,
    access_token: SecretString,
}

impl std::fmt::Debug for HttpPublicSurface {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HttpPublicSurface")
            .field("endpoint", &self.endpoint.host_str())
            .field("access_token", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl HttpPublicSurface {
    /// Builds the surface for one endpoint and app access token.
    ///
    /// # Errors
    ///
    /// [`PublicSurfaceError`] when the endpoint does not parse or the client cannot be built.
    pub fn new(endpoint: &str, access_token: SecretString) -> Result<Self, PublicSurfaceError> {
        let endpoint =
            reqwest::Url::parse(endpoint).map_err(|_| PublicSurfaceError::InvalidEndpoint)?;
        if endpoint.cannot_be_a_base() {
            return Err(PublicSurfaceError::InvalidEndpoint);
        }
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(TOTAL_TIMEOUT)
            .build()
            .map_err(|_| PublicSurfaceError::Client)?;
        Ok(Self {
            client,
            endpoint,
            access_token,
        })
    }

    /// Reads the body up to [`MAX_BODY_BYTES`]; `None` when it is larger or unreadable.
    async fn read_bounded(mut response: reqwest::Response) -> Option<Vec<u8>> {
        if response
            .content_length()
            .is_some_and(|length| length > MAX_BODY_BYTES as u64)
        {
            return None;
        }
        let mut body = Vec::new();
        while let Ok(chunk) = response.chunk().await {
            let Some(chunk) = chunk else {
                return Some(body);
            };
            if body.len().saturating_add(chunk.len()) > MAX_BODY_BYTES {
                return None;
            }
            body.extend_from_slice(&chunk);
        }
        None
    }
}

impl PublicSurface for HttpPublicSurface {
    async fn fetch(&self, permalink: &CanonicalPermalink) -> SurfaceOutcome {
        let mut url = self.endpoint.clone();
        url.query_pairs_mut()
            .append_pair("url", &permalink.url)
            .append_pair("access_token", self.access_token.expose_secret());
        // A reqwest error renders the URL, which carries the token: only its class is kept.
        let Ok(response) = self.client.get(url).send().await else {
            return SurfaceOutcome::TransportFailure;
        };
        let status = response.status().as_u16();
        if status == 401 {
            metrics::counter!(CREDENTIAL_REJECTED_TOTAL).increment(1);
            tracing::error!(
                error_class = "public_resolution_credential_rejected",
                "the Meta oEmbed endpoint rejected the configured access token"
            );
        }
        let Some(bytes) = Self::read_bounded(response).await else {
            return SurfaceOutcome::TransportFailure;
        };
        // A body that is not UTF-8 cannot be a JSON object; it classifies like any bad 200.
        let body = String::from_utf8_lossy(&bytes);
        classify_response(status, &body)
    }
}

/// Maps one HTTP answer onto what the approved surface proved.
///
/// This table is a starting point taken from the endpoint documentation, to be corrected against
/// recorded live responses; it is the only place that decides. `Deleted` is produced only by a
/// definitive not-found answer, never by a server error or a timeout.
///
/// | status | outcome |
/// | --- | --- |
/// | 200 with a JSON object | `Payload` |
/// | 200 with anything else | `Unavailable` |
/// | 404 | `Deleted` |
/// | 403 | `Private` |
/// | 400 | `Unsupported` |
/// | 401, 429, 5xx | `TemporarilyUnavailable` |
/// | anything else | `Unavailable` |
#[must_use]
pub fn classify_response(status: u16, body: &str) -> SurfaceOutcome {
    match status {
        200 => {
            if matches!(
                serde_json::from_str::<serde_json::Value>(body),
                Ok(serde_json::Value::Object(_))
            ) {
                SurfaceOutcome::Payload {
                    body: body.to_owned(),
                }
            } else {
                SurfaceOutcome::Unavailable
            }
        }
        404 => SurfaceOutcome::Deleted,
        403 => SurfaceOutcome::Private,
        400 => SurfaceOutcome::Unsupported,
        401 | 429 | 500..=599 => SurfaceOutcome::TemporarilyUnavailable,
        _ => SurfaceOutcome::Unavailable,
    }
}
