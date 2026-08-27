//! Public resolution of supported permalinks through the approved surface.
//!
//! One approved embed/oEmbed-style seam answers for a canonical permalink;
//! every successful answer is preserved byte for byte as content-addressed
//! evidence and appended as a new immutable, parser-versioned revision before
//! any normalization. Failed attempts record their availability kind verbatim
//! and fabricate nothing: no media row, no revision, no invented deletion.

use std::future::Future;

use sha2::Digest as _;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::capability::AvailabilityObservationKind;
use crate::database::Database;
use crate::permalink::{self, CanonicalPermalink, PermalinkError};

/// The parser version stamped on every revision this module produces.
///
/// A changed grammar bumps this constant: revisions remain interpretable
/// because each records which parser wrote it.
pub const OEMBED_PARSER_VERSION: &str = "instagram.oembed.v1";

/// What the approved public surface answered for one permalink.
///
/// Every failure classification is provider-side truth or an honest admission
/// about our own attempt; none may be rewritten to another kind downstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceOutcome {
    /// The surface answered with a payload document.
    Payload {
        /// The response body, preserved byte for byte.
        body: String,
    },
    /// The provider stated or implied the source no longer exists.
    Deleted,
    /// The source exists but denies anonymous access.
    Private,
    /// The attempt failed transiently; retrying later may succeed.
    TemporarilyUnavailable,
    /// The attempt failed without a proven cause.
    Unavailable,
    /// This object type is not resolvable through the approved surface.
    Unsupported,
    /// The attempt failed before any classification was possible.
    TransportFailure,
}

impl SurfaceOutcome {
    /// The availability observation kind this outcome proves, verbatim.
    #[must_use]
    pub fn observation(&self) -> AvailabilityObservationKind {
        match self {
            Self::Payload { .. } => AvailabilityObservationKind::Available,
            Self::Deleted => AvailabilityObservationKind::Deleted,
            Self::Private => AvailabilityObservationKind::Private,
            Self::TemporarilyUnavailable => AvailabilityObservationKind::TemporarilyUnavailable,
            Self::Unavailable | Self::TransportFailure => {
                AvailabilityObservationKind::ResolutionFailed
            }
            Self::Unsupported => AvailabilityObservationKind::Unsupported,
        }
    }
}

/// One stored successful resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredResolution {
    /// The normalized media row carrying this resolution's projection.
    pub media_id: Uuid,
    /// The immutable revision appended by this attempt.
    pub revision_id: Uuid,
    /// The content-addressed raw evidence holding the payload bytes.
    pub raw_record_id: Uuid,
}

/// The outcome of one resolution attempt against a capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolutionOutcome {
    /// A payload answered; evidence, revision, and normalized source exist.
    Resolved(StoredResolution),
    /// No payload existed; the verbatim observation kind was recorded.
    Unavailable(AvailabilityObservationKind),
}

/// Why intake or resolution refused to act.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ResolutionError {
    /// No capture exists under the given id.
    #[error("no capture exists under this id")]
    UnknownCapture,
    /// The stored canonical URL is no longer a supported permalink.
    #[error("the stored canonical URL is not a supported Instagram permalink")]
    InvalidStoredPermalink(#[source] PermalinkError),
    /// The payload exceeds what can be sized for storage.
    #[error("the payload exceeds storable size")]
    PayloadTooLarge,
    /// An archive-owned query failed.
    #[error("an instagram_archive database query failed")]
    Persistence(#[source] sqlx::Error),
    /// Building the publication fact for this outcome failed truthfulness.
    #[error("building the social-source fact failed")]
    Publishing(#[from] crate::publishing::PublishError),
}

/// The approved public-resolution surface.
///
/// Production implementations speak the official embed/oEmbed-style endpoint;
/// tests replay recorded fixtures. Implementations own endpoint construction,
/// credentials, retries, and timeouts — everything upstream of the answer.
pub trait PublicSurface: Send + Sync {
    /// Fetches the approved-surface answer for one canonical permalink.
    fn fetch(&self, permalink: &CanonicalPermalink) -> impl Future<Output = SurfaceOutcome> + Send;
}

/// The normalized projection written into the media row for one payload.
///
/// Only what the payload grammar exposes and the store models: the media type
/// implied by the permalink kind, and title text when the grammar carries one.
/// Everything else survives solely inside the raw revision bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedMedia {
    /// The media type implied by the permalink kind.
    pub media_type: &'static str,
    /// The title text carried by the payload, when present and non-empty.
    pub caption: Option<String>,
}

/// Why a payload could not be normalized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum NormalizeError {
    /// The payload is not an oEmbed-style JSON object.
    #[error("the payload is not an oEmbed-style JSON object")]
    Malformed,
}

/// The media type implied by a permalink kind, as the schema CHECK allows.
///
/// Reels and IGTV videos carry their type in the permalink itself. A plain
/// post does not reveal through this grammar whether it is an image, video,
/// or carousel, so it stores `unknown` rather than a guess.
#[must_use]
pub const fn kind_media_type(kind: permalink::PermalinkKind) -> &'static str {
    match kind {
        permalink::PermalinkKind::Post => "unknown",
        permalink::PermalinkKind::Reel => "reel",
        permalink::PermalinkKind::Igtv => "video",
    }
}

/// The payload fields this parser reads, per the documented surface grammar.
///
/// Deliberately minimal: everything outside these fields stays in the raw
/// revision bytes and never reaches a normalized column.
#[derive(serde::Deserialize)]
struct PayloadGrammar {
    /// Title text carried by the surface, when present.
    #[serde(default)]
    title: Option<String>,
}

/// Normalizes one payload document into its stored projection.
///
/// Deterministic by construction: a pure function of payload bytes and the
/// permalink kind.
///
/// # Errors
///
/// Returns [`NormalizeError`] when the payload is not an oEmbed-style JSON
/// object.
pub fn normalize(
    payload: &str,
    kind: permalink::PermalinkKind,
) -> Result<NormalizedMedia, NormalizeError> {
    let grammar: PayloadGrammar =
        serde_json::from_str(payload).map_err(|_| NormalizeError::Malformed)?;
    let caption = grammar
        .title
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty());
    Ok(NormalizedMedia {
        media_type: kind_media_type(kind),
        caption,
    })
}

/// The wire values this lane writes, equal to the schema CHECK vocabularies.
const PUBLIC_RESOLUTION_METHOD: &str = "public_resolution";
/// Public resolution observes upstream content; it never observes saved state.
const RESOLUTION_AUTHORITY: &str = "explicit_user_capture";

impl Database {
    /// Resolves the canonical permalink of one capture through the approved
    /// public surface.
    ///
    /// A payload answer is preserved byte for byte as content-addressed
    /// evidence, appended as a new immutable revision, and normalized into the
    /// media row linked to that revision. Any other outcome records its
    /// availability kind verbatim and fabricates nothing.
    ///
    /// # Errors
    ///
    /// Returns [`ResolutionError`] when the capture does not exist, its stored
    /// URL is no longer a supported permalink, or a query fails.
    pub async fn resolve_capture_permalink(
        &self,
        capture_id: Uuid,
        surface: &impl PublicSurface,
        resolved_at: OffsetDateTime,
    ) -> Result<ResolutionOutcome, ResolutionError> {
        let permalink = self.stored_capture_permalink(capture_id).await?;
        match surface.fetch(&permalink).await {
            SurfaceOutcome::Payload { body } => {
                // An unreadable payload is this service's failed attempt,
                // never a statement about the source.
                let Ok(normalized) = normalize(&body, permalink.kind) else {
                    return self
                        .record_failed_resolution(
                            capture_id,
                            AvailabilityObservationKind::ResolutionFailed,
                            resolved_at,
                        )
                        .await;
                };
                let byte_size =
                    i64::try_from(body.len()).map_err(|_| ResolutionError::PayloadTooLarge)?;
                Ok(ResolutionOutcome::Resolved(
                    self.store_resolution(
                        capture_id,
                        &permalink,
                        &normalized,
                        body.as_bytes(),
                        byte_size,
                        resolved_at,
                    )
                    .await?,
                ))
            }
            other => {
                self.record_failed_resolution(capture_id, other.observation(), resolved_at)
                    .await
            }
        }
    }

    /// Reads back and re-canonicalizes the capture's stored permalink.
    async fn stored_capture_permalink(
        &self,
        capture_id: Uuid,
    ) -> Result<CanonicalPermalink, ResolutionError> {
        let canonical_url: Option<(String,)> = sqlx::query_as(
            "select canonical_url from instagram_archive.captures where capture_id = $1",
        )
        .bind(capture_id)
        .fetch_optional(self.pool())
        .await
        .map_err(ResolutionError::Persistence)?;
        let Some((canonical_url,)) = canonical_url else {
            return Err(ResolutionError::UnknownCapture);
        };
        permalink::canonicalize(&canonical_url).map_err(ResolutionError::InvalidStoredPermalink)
    }

    /// Preserves one payload answer: raw evidence, a new immutable revision,
    /// the normalized media projection linked to it, an `available`
    /// observation, and the capture linkage — in one transaction.
    ///
    /// Re-resolution updates only this lane's projection columns and appends;
    /// provenance columns are immutable once written, and history is never
    /// rewritten.
    #[allow(clippy::too_many_arguments)]
    async fn store_resolution(
        &self,
        capture_id: Uuid,
        permalink: &CanonicalPermalink,
        normalized: &NormalizedMedia,
        payload_bytes: &[u8],
        byte_size: i64,
        resolved_at: OffsetDateTime,
    ) -> Result<StoredResolution, ResolutionError> {
        let mut transaction = self
            .pool()
            .begin()
            .await
            .map_err(ResolutionError::Persistence)?;

        let raw_record_id = Uuid::now_v7();
        let digest = sha2::Sha256::digest(payload_bytes);
        sqlx::query(
            "insert into instagram_archive.raw_records \
             (raw_record_id, record_kind, blob_ref, content_hash, byte_size, body, observed_at) \
             values ($1, 'oembed_response', $2, $3, $4, $5, $6)",
        )
        .bind(raw_record_id)
        .bind(hex_encode(&digest))
        .bind(digest.to_vec())
        .bind(byte_size)
        .bind(payload_bytes)
        .bind(resolved_at)
        .execute(&mut *transaction)
        .await
        .map_err(ResolutionError::Persistence)?;

        let media_id = self
            .upsert_media_projection(&mut transaction, permalink, normalized, resolved_at)
            .await?;

        // First preservation publishes `captured`; every later revision of the
        // same source publishes `updated`. Counted before this transaction
        // appends its own revision.
        let (prior_count,): (i64,) = sqlx::query_as(
            "select count(*) from instagram_archive.media_revisions r \
             join instagram_archive.media m on m.media_id = r.media_id where m.permalink = $1",
        )
        .bind(&permalink.url)
        .fetch_one(&mut *transaction)
        .await
        .map_err(ResolutionError::Persistence)?;

        let revision_id = Uuid::now_v7();
        sqlx::query(
            "insert into instagram_archive.media_revisions \
             (revision_id, media_id, raw_record_id, parser_version, resolved_at) \
             values ($1, $2, $3, $4, $5)",
        )
        .bind(revision_id)
        .bind(media_id)
        .bind(raw_record_id)
        .bind(OEMBED_PARSER_VERSION)
        .bind(resolved_at)
        .execute(&mut *transaction)
        .await
        .map_err(ResolutionError::Persistence)?;

        sqlx::query(
            "update instagram_archive.media set current_revision_id = $2 where media_id = $1",
        )
        .bind(media_id)
        .bind(revision_id)
        .execute(&mut *transaction)
        .await
        .map_err(ResolutionError::Persistence)?;

        sqlx::query(
            "insert into instagram_archive.availability_observations \
             (observation_id, media_id, capture_id, availability, resolver_version, observed_at) \
             values ($1, $2, $3, 'available', $4, $5)",
        )
        .bind(Uuid::now_v7())
        .bind(media_id)
        .bind(capture_id)
        .bind(OEMBED_PARSER_VERSION)
        .bind(resolved_at)
        .execute(&mut *transaction)
        .await
        .map_err(ResolutionError::Persistence)?;

        // A successful resolution concludes the intake; a failed capture stays
        // failed because its processing error is not resolved by this lane.
        sqlx::query(
            "update instagram_archive.captures \
             set status = 'resolved', media_id = $2 \
             where capture_id = $1 and status <> 'failed'",
        )
        .bind(capture_id)
        .bind(media_id)
        .execute(&mut *transaction)
        .await
        .map_err(ResolutionError::Persistence)?;

        let fact_kind = if prior_count == 0 {
            crate::publishing::FactKind::Captured
        } else {
            crate::publishing::FactKind::Updated
        };
        crate::publishing::append_fact(&mut transaction, fact_kind, capture_id).await?;

        transaction
            .commit()
            .await
            .map_err(ResolutionError::Persistence)?;

        Ok(StoredResolution {
            media_id,
            revision_id,
            raw_record_id,
        })
    }

    /// Finds the source by its permalink identity or creates it; updates touch
    /// only this lane's projection columns, never provenance.
    async fn upsert_media_projection(
        &self,
        transaction: &mut sqlx::PgConnection,
        permalink: &CanonicalPermalink,
        normalized: &NormalizedMedia,
        resolved_at: OffsetDateTime,
    ) -> Result<Uuid, ResolutionError> {
        let existing: Option<(Uuid,)> =
            sqlx::query_as("select media_id from instagram_archive.media where permalink = $1")
                .bind(&permalink.url)
                .fetch_optional(&mut *transaction)
                .await
                .map_err(ResolutionError::Persistence)?;
        if let Some((media_id,)) = existing {
            sqlx::query(
                "update instagram_archive.media \
                 set media_type = $2, caption = $3, upstream_status = 'available', \
                     updated_at = $4 \
                 where media_id = $1",
            )
            .bind(media_id)
            .bind(normalized.media_type)
            .bind(normalized.caption.clone())
            .bind(resolved_at)
            .execute(&mut *transaction)
            .await
            .map_err(ResolutionError::Persistence)?;
            return Ok(media_id);
        }

        let media_id = Uuid::now_v7();
        sqlx::query(
            "insert into instagram_archive.media \
             (media_id, permalink, media_type, caption, acquisition_method, saved_authority, \
              upstream_status) \
             values ($1, $2, $3, $4, $5, $6, 'available')",
        )
        .bind(media_id)
        .bind(&permalink.url)
        .bind(normalized.media_type)
        .bind(normalized.caption.clone())
        .bind(PUBLIC_RESOLUTION_METHOD)
        .bind(RESOLUTION_AUTHORITY)
        .execute(&mut *transaction)
        .await
        .map_err(ResolutionError::Persistence)?;
        Ok(media_id)
    }

    /// Records a failed resolution attempt verbatim and fabricates nothing.
    ///
    /// Every classified failure appends its own availability kind against the
    /// capture — provider-stated deletion stays `deleted`, denied access stays
    /// `private`, transient trouble stays `temporarily_unavailable`, and an
    /// attempt that failed before classification stays `resolution_failed` —
    /// and concludes the intake as `unavailable`. No media row and no revision
    /// exist afterwards, and nothing is ever rewritten to another kind.
    async fn record_failed_resolution(
        &self,
        capture_id: Uuid,
        observed: AvailabilityObservationKind,
        resolved_at: OffsetDateTime,
    ) -> Result<ResolutionOutcome, ResolutionError> {
        let mut transaction = self
            .pool()
            .begin()
            .await
            .map_err(ResolutionError::Persistence)?;
        sqlx::query(
            "insert into instagram_archive.availability_observations \
             (observation_id, media_id, capture_id, availability, resolver_version, observed_at) \
             values ($1, null, $2, $3, $4, $5)",
        )
        .bind(Uuid::now_v7())
        .bind(capture_id)
        .bind(observed.wire_value())
        .bind(OEMBED_PARSER_VERSION)
        .bind(resolved_at)
        .execute(&mut *transaction)
        .await
        .map_err(ResolutionError::Persistence)?;
        sqlx::query(
            "update instagram_archive.captures set status = 'unavailable' \
             where capture_id = $1 and status in ('accepted', 'resolved')",
        )
        .bind(capture_id)
        .execute(&mut *transaction)
        .await
        .map_err(ResolutionError::Persistence)?;

        // An observed upstream deletion of an already-preserved source is a
        // normalized-record change: collapse the media status and republish
        // the preserved content untouched under `deleted_upstream`. Every
        // other unavailable outcome publishes nothing — the published
        // snapshot cannot represent an authorless record truthfully yet.
        if matches!(observed, AvailabilityObservationKind::Deleted) {
            let linked: Option<(Uuid,)> = sqlx::query_as(
                "select media_id from instagram_archive.captures \
                 where capture_id = $1 and media_id is not null",
            )
            .bind(capture_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(ResolutionError::Persistence)?;
            if let Some((media_id,)) = linked {
                // `Deleted` collapses to the media status of the same name;
                // every other kind was already excluded by the guard above.
                sqlx::query(
                    "update instagram_archive.media set upstream_status = 'deleted', \
                     updated_at = $2 where media_id = $1",
                )
                .bind(media_id)
                .bind(resolved_at)
                .execute(&mut *transaction)
                .await
                .map_err(ResolutionError::Persistence)?;
                crate::publishing::append_fact(
                    &mut transaction,
                    crate::publishing::FactKind::Updated,
                    capture_id,
                )
                .await?;
            }
        }

        transaction
            .commit()
            .await
            .map_err(ResolutionError::Persistence)?;

        Ok(ResolutionOutcome::Unavailable(observed))
    }
}

/// Lowercase hex encoding for content addresses.
fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic, redacted oEmbed-style payload; no live call produced it.
    const REEL_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/oembed/reel_public.json"
    ));

    #[test]
    fn normalizing_the_recorded_reel_fixture_yields_its_documented_values() {
        let normalized = normalize(REEL_FIXTURE, permalink::PermalinkKind::Reel)
            .expect("the recorded fixture must parse");
        assert_eq!(
            normalized.media_type, "reel",
            "a reel permalink carries its media type"
        );
        assert_eq!(
            normalized.caption.as_deref(),
            Some("A public reel about composition"),
            "the grammar's title text is the caption"
        );
    }

    #[test]
    fn a_post_permalink_stores_unknown_media_type_instead_of_guessing() {
        let payload = r#"{"title":"maybe a video"}"#;
        let normalized =
            normalize(payload, permalink::PermalinkKind::Post).expect("a minimal object parses");
        assert_eq!(
            normalized.media_type, "unknown",
            "the grammar cannot reveal a plain post's media type"
        );
    }

    #[test]
    fn an_igtv_permalink_carries_video_from_its_kind() {
        let normalized = normalize(REEL_FIXTURE, permalink::PermalinkKind::Igtv)
            .expect("the recorded fixture must parse");
        assert_eq!(normalized.media_type, "video");
    }

    #[test]
    fn normalizing_the_same_payload_twice_is_identical() {
        let first = normalize(REEL_FIXTURE, permalink::PermalinkKind::Reel);
        let second = normalize(REEL_FIXTURE, permalink::PermalinkKind::Reel);
        assert_eq!(
            first, second,
            "determinism: equal inputs produce equal normalized values"
        );
    }

    #[test]
    fn normalizing_is_independent_of_undocumented_fields() {
        let mut value: serde_json::Value =
            serde_json::from_str(REEL_FIXTURE).expect("the recorded fixture is valid JSON");
        value["undocumented_field"] = serde_json::Value::String("noise".into());
        let with_extra = serde_json::to_string(&value).expect("serialization cannot fail");

        assert_eq!(
            normalize(REEL_FIXTURE, permalink::PermalinkKind::Reel),
            normalize(&with_extra, permalink::PermalinkKind::Reel),
            "fields outside the grammar must not influence the projection"
        );
    }

    #[test]
    fn blank_or_missing_title_normalizes_to_no_caption() {
        for payload in ["{}", "{\"title\":\"\"}", "{\"title\":\"   \"}"] {
            let normalized = normalize(payload, permalink::PermalinkKind::Post)
                .expect("an object without usable title still parses");
            assert_eq!(
                normalized.caption, None,
                "{payload} carries no caption text"
            );
        }
    }

    #[test]
    fn malformed_payloads_are_refused() {
        for payload in ["", "not json", "[1,2,3]", "\"a string\""] {
            let error = normalize(payload, permalink::PermalinkKind::Post)
                .expect_err("non-object payloads must be refused");
            assert!(
                matches!(error, NormalizeError::Malformed),
                "{payload} must refuse as malformed"
            );
        }
    }
}
