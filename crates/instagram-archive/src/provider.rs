//! Official Instagram API with Instagram Login provider profile.
//!
//! Contract re-verified 2026-08-27 against Meta's official Instagram API collection and
//! Instagram Login documentation:
//! <https://www.postman.com/meta/instagram/documentation/6yqw8pt/instagram-api>
//! <https://developers.facebook.com/docs/instagram-platform/instagram-api-with-instagram-login/>
//!
//! This is deliberately not the Facebook Login/Page-linked profile. It serves professional
//! business and creator accounts, uses the `instagram_business_*` permission family, and requests
//! only basic read authority. Provider write permissions remain absent; own-media reads use only
//! the connected account's reviewed media edge. The Graph path version is explicit so Meta's
//! default cannot drift.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;

use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::capability_reconciliation::{AccountType, PermissionStatus};
use crate::provider_budget::RequestClass;

/// Reviewed Graph API version for this provider profile.
pub const GRAPH_API_VERSION: &str = "v26.0";
/// Exact least-privilege permission requested at authorization.
pub const BASIC_READ_SCOPE: &str = "instagram_business_basic";
/// Entire requested permission inventory, intentionally one read permission.
pub const REQUESTED_SCOPES: [&str; 1] = [BASIC_READ_SCOPE];
/// Fixed production authorization endpoint.
pub const AUTHORIZE_ENDPOINT: &str = "https://www.instagram.com/oauth/authorize";
/// Fixed production single-use code exchange endpoint.
pub const CODE_EXCHANGE_ENDPOINT: &str = "https://api.instagram.com/oauth/access_token";
/// Fixed production Instagram Graph host.
pub const GRAPH_API_ORIGIN: &str = "https://graph.instagram.com";
/// Fields needed to reconcile account identity and type, no media payload.
pub const ACCOUNT_DISCOVERY_FIELDS: &str = "id,username,account_type";
/// Reviewed own-media metadata fields; stories and ephemeral surfaces are absent.
pub const OWN_MEDIA_FIELDS: &str = "id,owner,caption,media_type,media_product_type,media_url,permalink,thumbnail_url,timestamp,username";
/// The selected profile exposes refresh of long-lived Instagram user tokens.
pub const REFRESH_SUPPORTED: bool = true;
/// No separately reliable revoke call is enabled for this reviewed profile.
/// Local credential scrubbing remains mandatory and authoritative.
pub const REVOKE_SUPPORTED: bool = false;

/// Successful authorization-code exchange material, held only in secret wrappers.
#[derive(Debug)]
pub struct ExchangedToken {
    /// Short- or long-lived provider access token.
    pub access_token: SecretString,
    /// Stable provider user identity returned by exchange.
    pub user_id: String,
    /// Permission names the exchange itself reports, if any.
    pub permissions: Vec<String>,
    /// Provider-reported lifetime when present.
    pub expires_in_seconds: Option<u64>,
}

/// Strict account-discovery result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderAccount {
    /// Stable provider identity.
    pub provider_account_id: String,
    /// Mutable display username, never used as identity.
    pub username: String,
    /// Provider-observed account type.
    pub account_type: AccountType,
}

/// Strict complete permission-discovery result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderPermissions {
    /// Permission name-to-observed status map.
    pub statuses: BTreeMap<String, PermissionStatus>,
}

/// One strictly bounded connected-account media observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderOwnMediaItem {
    /// Stable provider media identity.
    pub provider_media_id: String,
    /// Provider-declared owner identity.
    pub owner_provider_account_id: String,
    /// Provider media kind.
    pub media_type: String,
    /// Product surface such as feed or reels.
    pub media_product_type: String,
    /// Provider canonical media URL.
    pub permalink: String,
    /// Mutable caption when exposed.
    pub caption: Option<String>,
    /// Strictly parsed provider publication timestamp.
    pub published_at: OffsetDateTime,
    /// Expiring provider media URL observation, never a `BlobRef`.
    pub media_url: Option<String>,
    /// Expiring provider thumbnail URL observation, never a `BlobRef`.
    pub thumbnail_url: Option<String>,
}

/// One accepted provider page plus exact raw evidence bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderOwnMediaPage {
    /// Ordered newest-first items.
    pub items: Vec<ProviderOwnMediaItem>,
    /// Opaque provider continuation for the same traversal.
    pub next_cursor: Option<String>,
    /// Exact capped response body used for normalization.
    pub raw_body: Vec<u8>,
}

/// Boxed provider future keeps ports object-safe without an async-trait dependency.
pub type ProviderFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ProviderError>> + Send + 'a>>;

/// Narrow official-provider port used by the account lifecycle.
pub trait InstagramProvider: Send + Sync {
    /// Exchanges one single-use authorization code; callers never retry this operation.
    fn exchange_code<'a>(&'a self, code: &'a SecretString) -> ProviderFuture<'a, ExchangedToken>;
    /// Discovers stable identity and provider-observed account type.
    fn discover_account<'a>(
        &'a self,
        access_token: &'a SecretString,
    ) -> ProviderFuture<'a, ProviderAccount>;
    /// Discovers the complete current permission status set.
    fn discover_permissions<'a>(
        &'a self,
        access_token: &'a SecretString,
    ) -> ProviderFuture<'a, ProviderPermissions>;
    /// Refreshes a provider-supported long-lived access token.
    fn refresh_token<'a>(
        &'a self,
        access_token: &'a SecretString,
    ) -> ProviderFuture<'a, ExchangedToken>;
    /// Makes the selected profile's optional provider-side revoke attempt.
    fn revoke_token<'a>(&'a self, access_token: &'a SecretString) -> ProviderFuture<'a, ()>;
    /// Reads one own-media page for the connected provider account only.
    fn list_own_media_page<'a>(
        &'a self,
        provider_account_id: &'a str,
        access_token: &'a SecretString,
        after: Option<&'a str>,
    ) -> ProviderFuture<'a, ProviderOwnMediaPage>;
}

/// One single-use callback relay claim returned by Platform.
#[derive(Debug)]
pub struct RelayClaim {
    /// Ratatoskr owner bound at callback receipt.
    pub user_ref: uuid::Uuid,
    /// Raw OAuth state returned by Meta, never persisted by this service.
    pub state: SecretString,
    /// Authorization code returned by Meta, never persisted by this service.
    pub authorization_code: SecretString,
    /// Exact public callback URI Platform received.
    pub redirect_uri: String,
}

/// Boxed relay future keeps the code-claim port object-safe.
pub type RelayFuture<'a> =
    Pin<Box<dyn Future<Output = Result<RelayClaim, RelayError>> + Send + 'a>>;

/// Narrow one-time code relay port owned by Platform ADR-0012.
pub trait OAuthCodeRelay: Send + Sync {
    /// Claims one relay identifier exactly once.
    fn claim<'a>(&'a self, relay_id: &'a str) -> RelayFuture<'a>;
}

/// Reqwest/Rustls client for Platform's audience-bound one-time relay.
#[derive(Debug, Clone)]
pub struct ReqwestOAuthCodeRelay {
    client: reqwest::Client,
    claim_url: reqwest::Url,
    bearer_token: SecretString,
    max_response_bytes: usize,
}

impl ReqwestOAuthCodeRelay {
    /// Builds the finite Platform relay client.
    ///
    /// # Errors
    ///
    /// Returns [`RelayError::ResponseRefused`] for an invalid claim URL or client configuration.
    pub fn new(
        claim_url: reqwest::Url,
        bearer_token: SecretString,
        timeout: std::time::Duration,
        max_response_bytes: usize,
    ) -> Result<Self, RelayError> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| RelayError::ResponseRefused)?;
        Ok(Self {
            client,
            claim_url,
            bearer_token,
            max_response_bytes,
        })
    }
}

impl OAuthCodeRelay for ReqwestOAuthCodeRelay {
    fn claim<'a>(&'a self, relay_id: &'a str) -> RelayFuture<'a> {
        Box::pin(async move {
            if relay_id.is_empty() || relay_id.len() > 256 {
                return Err(RelayError::Unavailable);
            }
            let mut response = self
                .client
                .post(self.claim_url.clone())
                .bearer_auth(self.bearer_token.expose_secret())
                .json(&RelayClaimRequest { relay_id })
                .send()
                .await
                .map_err(|_| RelayError::Transport)?;
            if !response.status().is_success() {
                return Err(if response.status().is_server_error() {
                    RelayError::Transport
                } else {
                    RelayError::Unavailable
                });
            }
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| RelayError::Transport)? {
                let Some(new_len) = body.len().checked_add(chunk.len()) else {
                    return Err(RelayError::ResponseRefused);
                };
                if new_len > self.max_response_bytes {
                    return Err(RelayError::ResponseRefused);
                }
                body.extend_from_slice(&chunk);
            }
            let claim: RelayClaimResponse =
                serde_json::from_slice(&body).map_err(|_| RelayError::ResponseRefused)?;
            Ok(RelayClaim {
                user_ref: claim.user_ref,
                state: SecretString::from(claim.state),
                authorization_code: SecretString::from(claim.authorization_code),
                redirect_uri: claim.redirect_uri,
            })
        })
    }
}

#[derive(Serialize)]
struct RelayClaimRequest<'a> {
    relay_id: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RelayClaimResponse {
    user_ref: uuid::Uuid,
    state: String,
    authorization_code: String,
    redirect_uri: String,
}

/// Redacted Platform relay failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RelayError {
    /// Relay does not exist, expired, mismatched audience, or was already claimed.
    #[error("OAuth relay is unavailable")]
    Unavailable,
    /// Platform transport failed transiently.
    #[error("OAuth relay transport failed")]
    Transport,
    /// Platform returned a body outside the strict contract.
    #[error("OAuth relay response was refused")]
    ResponseRefused,
}

/// Official provider failure class, safe for logs and HTTP errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderFailureClass {
    /// Token or authorization is unusable.
    Authentication,
    /// Provider rejected request grammar.
    Validation,
    /// Provider rate limit refused the request.
    RateLimited,
    /// Provider returned a server failure.
    Server,
    /// Transport did not yield a response.
    Network,
    /// Response exceeded limits or violated strict schema.
    ResponseRefused,
    /// Selected provider profile documents no such operation.
    Unsupported,
}

/// Redacted provider error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("official Instagram provider request failed: {class:?}")]
pub struct ProviderError {
    /// Closed failure class.
    pub class: ProviderFailureClass,
    /// Safe numeric response status when one existed.
    pub http_status: Option<u16>,
}

/// Reqwest/Rustls adapter for the fixed production provider hosts.
#[derive(Debug, Clone)]
pub struct ReqwestInstagramProvider {
    client: reqwest::Client,
    client_id: String,
    client_secret: SecretString,
    redirect_uri: String,
    code_exchange_endpoint: reqwest::Url,
    graph_origin: reqwest::Url,
    max_response_bytes: usize,
}

impl ReqwestInstagramProvider {
    /// Builds the production adapter with fixed official hosts and finite timeouts.
    ///
    /// # Errors
    ///
    /// Returns a redacted [`ProviderError`] if fixed constants or client setup fail.
    pub fn new(
        client_id: String,
        client_secret: SecretString,
        redirect_uri: String,
        connect_timeout: std::time::Duration,
        request_timeout: std::time::Duration,
        max_response_bytes: usize,
    ) -> Result<Self, ProviderError> {
        let client = reqwest::Client::builder()
            .connect_timeout(connect_timeout)
            .timeout(request_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| refused(ProviderFailureClass::Validation, None))?;
        let code_exchange_endpoint = reqwest::Url::parse(CODE_EXCHANGE_ENDPOINT)
            .map_err(|_| refused(ProviderFailureClass::Validation, None))?;
        let graph_origin = reqwest::Url::parse(GRAPH_API_ORIGIN)
            .map_err(|_| refused(ProviderFailureClass::Validation, None))?;
        Ok(Self {
            client,
            client_id,
            client_secret,
            redirect_uri,
            code_exchange_endpoint,
            graph_origin,
            max_response_bytes,
        })
    }

    /// Test-only origin injection. Production constructors cannot override provider hosts.
    ///
    /// # Errors
    ///
    /// Returns a redacted [`ProviderError`] when the finite client cannot be built.
    #[cfg(feature = "test-support")]
    pub fn for_test(
        client_id: String,
        client_secret: SecretString,
        redirect_uri: String,
        code_exchange_endpoint: reqwest::Url,
        graph_origin: reqwest::Url,
        max_response_bytes: usize,
    ) -> Result<Self, ProviderError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| refused(ProviderFailureClass::Validation, None))?;
        Ok(Self {
            client,
            client_id,
            client_secret,
            redirect_uri,
            code_exchange_endpoint,
            graph_origin,
            max_response_bytes,
        })
    }

    /// Builds the documented form-encoded single-use exchange request.
    ///
    /// # Errors
    ///
    /// Returns a redacted error when request construction fails.
    pub fn exchange_request(&self, code: &SecretString) -> Result<reqwest::Request, ProviderError> {
        self.client
            .post(self.code_exchange_endpoint.clone())
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.expose_secret()),
                ("grant_type", "authorization_code"),
                ("redirect_uri", self.redirect_uri.as_str()),
                ("code", code.expose_secret()),
            ])
            .build()
            .map_err(|_| refused(ProviderFailureClass::Validation, None))
    }

    /// Builds account discovery with a bearer header, never a token query parameter.
    ///
    /// # Errors
    ///
    /// Returns a redacted error when request construction fails.
    pub fn account_request(
        &self,
        access_token: &SecretString,
    ) -> Result<reqwest::Request, ProviderError> {
        let mut url = self
            .graph_origin
            .join(&format!("{GRAPH_API_VERSION}/me"))
            .map_err(|_| refused(ProviderFailureClass::Validation, None))?;
        url.query_pairs_mut()
            .append_pair("fields", ACCOUNT_DISCOVERY_FIELDS);
        self.client
            .get(url)
            .bearer_auth(access_token.expose_secret())
            .build()
            .map_err(|_| refused(ProviderFailureClass::Validation, None))
    }

    /// Builds an own-media request for the connected provider account.
    ///
    /// # Errors
    ///
    /// Returns a redacted error when request construction fails.
    pub fn own_media_request(
        &self,
        provider_account_id: &str,
        access_token: &SecretString,
        after: Option<&str>,
    ) -> Result<reqwest::Request, ProviderError> {
        if provider_account_id.is_empty() || provider_account_id.contains('/') {
            return Err(refused(ProviderFailureClass::Validation, None));
        }
        let mut url = self
            .graph_origin
            .join(&format!("{GRAPH_API_VERSION}/{provider_account_id}/media"))
            .map_err(|_| refused(ProviderFailureClass::Validation, None))?;
        url.query_pairs_mut()
            .append_pair("fields", OWN_MEDIA_FIELDS);
        if let Some(cursor) = after {
            if cursor.is_empty() || cursor.len() > 2048 {
                return Err(refused(ProviderFailureClass::Validation, None));
            }
            url.query_pairs_mut().append_pair("after", cursor);
        }
        self.client
            .get(url)
            .bearer_auth(access_token.expose_secret())
            .build()
            .map_err(|_| refused(ProviderFailureClass::Validation, None))
    }

    fn permissions_request(
        &self,
        access_token: &SecretString,
    ) -> Result<reqwest::Request, ProviderError> {
        let url = self
            .graph_origin
            .join(&format!("{GRAPH_API_VERSION}/me/permissions"))
            .map_err(|_| refused(ProviderFailureClass::Validation, None))?;
        self.client
            .get(url)
            .bearer_auth(access_token.expose_secret())
            .build()
            .map_err(|_| refused(ProviderFailureClass::Validation, None))
    }

    fn refresh_request(
        &self,
        access_token: &SecretString,
    ) -> Result<reqwest::Request, ProviderError> {
        let mut url = self
            .graph_origin
            .join(&format!("{GRAPH_API_VERSION}/refresh_access_token"))
            .map_err(|_| refused(ProviderFailureClass::Validation, None))?;
        url.query_pairs_mut()
            .append_pair("grant_type", "ig_refresh_token");
        self.client
            .get(url)
            .bearer_auth(access_token.expose_secret())
            .build()
            .map_err(|_| refused(ProviderFailureClass::Validation, None))
    }

    /// Strictly parses one capped account-discovery body.
    ///
    /// # Errors
    ///
    /// Returns response-refused for oversize, unknown fields, or unknown account type.
    pub fn parse_account(&self, body: &[u8]) -> Result<ProviderAccount, ProviderError> {
        self.ensure_bounded(body)?;
        let response: AccountResponse = serde_json::from_slice(body)
            .map_err(|_| refused(ProviderFailureClass::ResponseRefused, None))?;
        let account_type = match response.account_type.as_str() {
            "BUSINESS" => AccountType::Business,
            "CREATOR" => AccountType::Creator,
            "PERSONAL" => AccountType::Personal,
            _ => return Err(refused(ProviderFailureClass::ResponseRefused, None)),
        };
        Ok(ProviderAccount {
            provider_account_id: response.id,
            username: response.username,
            account_type,
        })
    }

    /// Strictly parses one capped permission-discovery body.
    ///
    /// # Errors
    ///
    /// Returns response-refused for oversize, unknown fields, duplicates, or malformed statuses.
    pub fn parse_permissions(&self, body: &[u8]) -> Result<ProviderPermissions, ProviderError> {
        self.ensure_bounded(body)?;
        let response: PermissionsResponse = serde_json::from_slice(body)
            .map_err(|_| refused(ProviderFailureClass::ResponseRefused, None))?;
        let mut statuses = BTreeMap::new();
        for entry in response.data {
            let status = match entry.status.as_str() {
                "granted" => PermissionStatus::Granted,
                "declined" => PermissionStatus::Declined,
                "expired" => PermissionStatus::Expired,
                "absent" => PermissionStatus::Absent,
                _ => PermissionStatus::Unknown,
            };
            if statuses.insert(entry.permission, status).is_some() {
                return Err(refused(ProviderFailureClass::ResponseRefused, None));
            }
        }
        Ok(ProviderPermissions { statuses })
    }

    /// Parses one bounded own-media page.
    ///
    /// # Errors
    ///
    /// Returns response-refused when the body cannot be decoded.
    pub fn parse_own_media_page(
        &self,
        body: &[u8],
        provider_account_id: &str,
    ) -> Result<ProviderOwnMediaPage, ProviderError> {
        self.ensure_bounded(body)?;
        let response: OwnMediaResponse = serde_json::from_slice(body)
            .map_err(|_| refused(ProviderFailureClass::ResponseRefused, None))?;
        let mut seen = BTreeSet::new();
        let mut items = Vec::with_capacity(response.data.len());
        for item in response.data {
            if item.owner.id != provider_account_id
                || item.id.is_empty()
                || !seen.insert(item.id.clone())
                || !matches!(
                    item.media_type.as_str(),
                    "IMAGE" | "VIDEO" | "CAROUSEL_ALBUM"
                )
                || !matches!(item.media_product_type.as_str(), "FEED" | "REELS")
                || reqwest::Url::parse(&item.permalink)
                    .ok()
                    .is_none_or(|url| url.scheme() != "https")
            {
                return Err(refused(ProviderFailureClass::ResponseRefused, None));
            }
            items.push(ProviderOwnMediaItem {
                provider_media_id: item.id,
                owner_provider_account_id: item.owner.id,
                media_type: item.media_type,
                media_product_type: item.media_product_type,
                permalink: item.permalink,
                caption: item.caption,
                published_at: parse_own_media_timestamp(&item.timestamp)
                    .ok_or_else(|| refused(ProviderFailureClass::ResponseRefused, None))?,
                media_url: item.media_url,
                thumbnail_url: item.thumbnail_url,
            });
        }
        let next_cursor = response
            .paging
            .and_then(|paging| paging.cursors.map(|cursors| cursors.after));
        Ok(ProviderOwnMediaPage {
            items,
            next_cursor,
            raw_body: body.to_vec(),
        })
    }

    /// Whether this request class and failure may use another attempt.
    #[must_use]
    pub const fn should_retry(request_class: RequestClass, failure: ProviderFailureClass) -> bool {
        matches!(
            request_class,
            RequestClass::AccountDiscovery
                | RequestClass::PermissionDiscovery
                | RequestClass::OwnMediaPage
        ) && matches!(
            failure,
            ProviderFailureClass::Network
                | ProviderFailureClass::RateLimited
                | ProviderFailureClass::Server
        )
    }

    /// Maps one HTTP status to a closed safe failure class.
    #[must_use]
    pub const fn classify_status(status: u16) -> ProviderFailureClass {
        match status {
            401 | 403 => ProviderFailureClass::Authentication,
            429 => ProviderFailureClass::RateLimited,
            400..=499 => ProviderFailureClass::Validation,
            500..=599 => ProviderFailureClass::Server,
            _ => ProviderFailureClass::ResponseRefused,
        }
    }

    fn ensure_bounded(&self, body: &[u8]) -> Result<(), ProviderError> {
        if body.len() > self.max_response_bytes {
            return Err(refused(ProviderFailureClass::ResponseRefused, None));
        }
        Ok(())
    }

    async fn execute(&self, request: reqwest::Request) -> Result<Vec<u8>, ProviderError> {
        let mut response = self
            .client
            .execute(request)
            .await
            .map_err(|_| refused(ProviderFailureClass::Network, None))?;
        let status = response.status().as_u16();
        if !response.status().is_success() {
            return Err(refused(Self::classify_status(status), Some(status)));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| refused(ProviderFailureClass::Network, Some(status)))?
        {
            let Some(new_len) = body.len().checked_add(chunk.len()) else {
                return Err(refused(ProviderFailureClass::ResponseRefused, Some(status)));
            };
            if new_len > self.max_response_bytes {
                return Err(refused(ProviderFailureClass::ResponseRefused, Some(status)));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

impl InstagramProvider for ReqwestInstagramProvider {
    fn exchange_code<'a>(&'a self, code: &'a SecretString) -> ProviderFuture<'a, ExchangedToken> {
        Box::pin(async move {
            let request = self.exchange_request(code)?;
            let body = self.execute(request).await?;
            self.ensure_bounded(&body)?;
            let response: ExchangeResponse = serde_json::from_slice(&body)
                .map_err(|_| refused(ProviderFailureClass::ResponseRefused, None))?;
            let user_id = match response.user_id {
                serde_json::Value::String(value) => value,
                serde_json::Value::Number(value) => value.to_string(),
                _ => return Err(refused(ProviderFailureClass::ResponseRefused, None)),
            };
            Ok(ExchangedToken {
                access_token: SecretString::from(response.access_token),
                user_id,
                permissions: response.permissions,
                expires_in_seconds: response.expires_in,
            })
        })
    }

    fn discover_account<'a>(
        &'a self,
        access_token: &'a SecretString,
    ) -> ProviderFuture<'a, ProviderAccount> {
        Box::pin(async move {
            let request = self.account_request(access_token)?;
            let body = self.execute(request).await?;
            self.parse_account(&body)
        })
    }

    fn discover_permissions<'a>(
        &'a self,
        access_token: &'a SecretString,
    ) -> ProviderFuture<'a, ProviderPermissions> {
        Box::pin(async move {
            let request = self.permissions_request(access_token)?;
            let body = self.execute(request).await?;
            self.parse_permissions(&body)
        })
    }

    fn refresh_token<'a>(
        &'a self,
        access_token: &'a SecretString,
    ) -> ProviderFuture<'a, ExchangedToken> {
        Box::pin(async move {
            let request = self.refresh_request(access_token)?;
            let body = self.execute(request).await?;
            self.ensure_bounded(&body)?;
            let response: RefreshResponse = serde_json::from_slice(&body)
                .map_err(|_| refused(ProviderFailureClass::ResponseRefused, None))?;
            Ok(ExchangedToken {
                access_token: SecretString::from(response.access_token),
                user_id: String::new(),
                permissions: Vec::new(),
                expires_in_seconds: response.expires_in,
            })
        })
    }

    fn revoke_token<'a>(&'a self, _access_token: &'a SecretString) -> ProviderFuture<'a, ()> {
        Box::pin(async { Err(refused(ProviderFailureClass::Unsupported, None)) })
    }

    fn list_own_media_page<'a>(
        &'a self,
        provider_account_id: &'a str,
        access_token: &'a SecretString,
        after: Option<&'a str>,
    ) -> ProviderFuture<'a, ProviderOwnMediaPage> {
        Box::pin(async move {
            let request = self.own_media_request(provider_account_id, access_token, after)?;
            let body = self.execute(request).await?;
            self.parse_own_media_page(&body, provider_account_id)
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AccountResponse {
    id: String,
    username: String,
    account_type: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PermissionsResponse {
    data: Vec<PermissionEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PermissionEntry {
    permission: String,
    status: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExchangeResponse {
    access_token: String,
    user_id: serde_json::Value,
    #[serde(default)]
    permissions: Vec<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RefreshResponse {
    access_token: String,
    #[allow(dead_code, reason = "strictly accepted provider contract field")]
    token_type: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnMediaResponse {
    data: Vec<OwnMediaEntry>,
    paging: Option<OwnMediaPaging>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnMediaEntry {
    id: String,
    owner: OwnMediaOwner,
    caption: Option<String>,
    media_type: String,
    media_product_type: String,
    media_url: Option<String>,
    permalink: String,
    thumbnail_url: Option<String>,
    timestamp: String,
    #[allow(dead_code, reason = "strictly accepted mutable provider display field")]
    username: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnMediaOwner {
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnMediaPaging {
    cursors: Option<OwnMediaCursors>,
    #[allow(
        dead_code,
        reason = "cursor is the authority; provider URL can contain credentials"
    )]
    next: Option<String>,
    #[allow(
        dead_code,
        reason = "cursor is the authority; provider URL can contain credentials"
    )]
    previous: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnMediaCursors {
    #[allow(dead_code, reason = "strictly accepted provider page-control field")]
    before: String,
    after: String,
}

fn parse_own_media_timestamp(value: &str) -> Option<OffsetDateTime> {
    if let Ok(timestamp) = OffsetDateTime::parse(value, &Rfc3339) {
        return Some(timestamp);
    }
    if !value.is_ascii() {
        return None;
    }
    let split = value.len().checked_sub(5)?;
    let (prefix, compact_offset) = value.split_at_checked(split)?;
    let mut offset_chars = compact_offset.chars();
    if !matches!(offset_chars.next(), Some('+' | '-'))
        || !offset_chars.all(|character| character.is_ascii_digit())
    {
        return None;
    }
    let (hours, minutes) = compact_offset.split_at_checked(3)?;
    let normalized = format!("{prefix}{hours}:{minutes}");
    OffsetDateTime::parse(&normalized, &Rfc3339).ok()
}

fn refused(class: ProviderFailureClass, http_status: Option<u16>) -> ProviderError {
    ProviderError { class, http_status }
}
