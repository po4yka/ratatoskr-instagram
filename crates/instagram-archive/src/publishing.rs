//! Social-source publishing: snapshot construction from stored rows,
//! transactional outbox appends, and the at-least-once publisher loop.
//!
//! Every published field is read back from storage; nothing is carried over
//! from request payloads, so a published snapshot can never claim more than
//! the archive proves. User notes never enter this module's output.

use ratatoskr_error_contracts::{ErrorCode, WarningEnvelope};
use ratatoskr_event_envelope::{EventEnvelope, EventPayload};
use ratatoskr_identifiers::{
    BlobOwner, BlobRef, ContentDigest, DigestAlgorithm, DigestHex, EntityLocalId, Extensions,
    IdentifierError, MediaType, SafeMessage, TenantRef, WireTimestamp,
};
use ratatoskr_social_contracts::{
    AcquisitionMethod, CaptureCompleteness, Platform, PostPermalink, PostText, RemovalReason,
    SavedAuthority, SocialSourceCaptured, SocialSourceRemoved, SocialSourceSnapshot,
    SocialSourceUpdated, SyncCheckpointCursor, UpstreamAvailability,
};
use sha2::Digest as _;
use sqlx::PgConnection;
use uuid::Uuid;

mod data_export;

pub(crate) use data_export::append_fact as append_data_export_fact;

/// The platform token every published snapshot carries.
pub const SOCIAL_PLATFORM: &str = "instagram";

/// The producer name stamped into every envelope.
pub const PRODUCER_NAME: &str = "ratatoskr-instagram";

/// Namespace for the derived per-`(owner, permalink)` source identity. A fixed
/// value; changing it changes every identity and is therefore a deliberate,
/// migration-grade decision.
const IDENTITY_NAMESPACE: Uuid = Uuid::from_bytes([
    0x72, 0x61, 0x74, 0x6f, 0x73, 0x6b, 0x72, 0x49, 0x6e, 0x73, 0x74, 0x61, 0x67, 0x72, 0x61, 0x6d,
]);
type OwnMediaPublishRow = (
    Uuid,
    String,
    String,
    Option<String>,
    time::OffsetDateTime,
    String,
    i64,
);

/// Why an event could not be built or appended.
#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    /// The capture has no preserved normalized content to publish truthfully.
    #[error("capture {0} has no publishable normalized record")]
    NothingToPublish(Uuid),
    /// The stored permalink is not a canonical supported shape.
    #[error("capture {0} stores a non-canonical permalink")]
    InvalidStoredPermalink(Uuid),
    /// An identifier, text, or code value failed contract validation.
    #[error("contract validation failed while publishing capture {0}: {1}")]
    ContractViolation(Uuid, String),
    /// The database refused a read or write.
    #[error("database failure while publishing")]
    Persistence(#[from] sqlx::Error),
    /// The fact or its envelope could not be serialized.
    #[error("serialization failure while publishing")]
    Serialization(#[from] serde_json::Error),
}

fn violation(capture_id: Uuid, error: &IdentifierError) -> PublishError {
    PublishError::ContractViolation(capture_id, error.to_string())
}

fn text_violation(capture_id: Uuid, error: impl std::fmt::Display) -> PublishError {
    PublishError::ContractViolation(capture_id, error.to_string())
}

/// Derives the stable Ratatoskr identity for one `(owner, source)` pair.
///
/// Deterministic by design: the same user capturing the same canonical URL
/// forever maps to one identity across captured and updated facts, two owners
/// of one URL stay distinct, and no schema column is needed.
#[must_use]
pub fn source_identity(owner: Uuid, canonical_permalink: &str) -> Uuid {
    let mut material = Vec::with_capacity(owner.as_bytes().len() + canonical_permalink.len() + 1);
    material.extend_from_slice(owner.as_bytes());
    material.push(0);
    material.extend_from_slice(canonical_permalink.as_bytes());
    Uuid::new_v5(&IDENTITY_NAMESPACE, &material)
}

/// Derives stable identity for one official own-media provider identity.
#[must_use]
pub fn own_media_source_identity(owner: Uuid, provider_media_id: &str) -> Uuid {
    let mut material = Vec::with_capacity(owner.as_bytes().len() + provider_media_id.len() + 10);
    material.extend_from_slice(owner.as_bytes());
    material.extend_from_slice(b"\0official\0");
    material.extend_from_slice(provider_media_id.as_bytes());
    Uuid::new_v5(&IDENTITY_NAMESPACE, &material)
}

/// Appends changed official own-media snapshots from one completing run.
///
/// # Errors
///
/// Returns [`PublishError`] if stored metadata violates the shared contract or
/// the caller-owned completion transaction cannot append the outbox rows.
#[expect(
    clippy::too_many_lines,
    reason = "one contract builder keeps every official metadata and BlobRef claim visible"
)]
pub async fn append_own_media_facts(
    transaction: &mut PgConnection,
    run_id: Uuid,
    owner: Uuid,
    checkpoint: Option<&str>,
    captured_at: time::OffsetDateTime,
) -> Result<u32, PublishError> {
    let rows: Vec<OwnMediaPublishRow> = sqlx::query_as(
        "select m.media_id, i.provider_media_id, i.permalink, i.caption, i.published_at,
                r.blob_ref, r.byte_size
         from instagram_archive.own_media_sync_items i
         join instagram_archive.media m on m.provider_media_id = i.provider_media_id
         join instagram_archive.raw_records r on r.raw_record_id = i.raw_record_id
         where i.run_id = $1 order by i.provider_media_id",
    )
    .bind(run_id)
    .fetch_all(&mut *transaction)
    .await?;
    let mut appended = 0_u32;
    for (media_id, provider_media_id, permalink, caption, published_at, blob_ref, byte_size) in rows
    {
        let digest = own_media_content_digest(
            &provider_media_id,
            &permalink,
            caption.as_deref(),
            published_at,
        )?;
        let digest_hex = digest.hex.to_string();
        let prior: Vec<(String, Option<String>)> = sqlx::query_as(
            "select event_type,
                    payload #>> '{payload,source,content_digest,hex}' as content_digest
             from instagram_archive.outbox_events
             where aggregate_type = 'media' and aggregate_id = $1
             order by occurred_at, event_id",
        )
        .bind(media_id)
        .fetch_all(&mut *transaction)
        .await?;
        if prior
            .iter()
            .any(|(_, prior_digest)| prior_digest.as_deref() == Some(&digest_hex))
        {
            continue;
        }
        let kind = if prior.is_empty() {
            FactKind::Captured
        } else {
            FactKind::Updated
        };
        let source_id = own_media_source_identity(owner, &provider_media_id);
        let snapshot = SocialSourceSnapshot {
            social_source_id: parse_source_id(source_id, media_id)?,
            platform: Platform::parse(SOCIAL_PLATFORM)
                .map_err(|error| violation(media_id, &error))?,
            external_post_id: EntityLocalId::parse(&provider_media_id)
                .map_err(|error| violation(media_id, &error))?,
            permalink: Some(
                PostPermalink::parse(&permalink).map_err(|error| violation(media_id, &error))?,
            ),
            owner: owner_ref(owner, media_id)?,
            author: None,
            published_at: Some(instant_from_time(published_at, media_id)?),
            captured_at: instant_from_time(captured_at, media_id)?,
            text: match caption.as_deref() {
                Some(text) if !text.is_empty() => {
                    Some(PostText::parse(text).map_err(|error| text_violation(media_id, error))?)
                }
                _ => None,
            },
            media: Vec::new(),
            relations: Vec::new(),
            folders: Vec::new(),
            content_digest: digest,
            raw_blob: Some(BlobRef {
                owner_service: BlobOwner::parse(PRODUCER_NAME)
                    .map_err(|error| violation(media_id, &error))?,
                digest: ContentDigest {
                    algorithm: DigestAlgorithm::Sha256,
                    hex: DigestHex::parse(&blob_ref)
                        .map_err(|error| violation(media_id, &error))?,
                },
                media_type: MediaType::parse("application/json")
                    .map_err(|error| violation(media_id, &error))?,
                length_bytes: u64::try_from(byte_size)
                    .map_err(|error| text_violation(media_id, error))?,
            }),
            acquisition: AcquisitionMethod::OfficialApi,
            saved_authority: SavedAuthority::AuthoritativePlatformState,
            completeness: CaptureCompleteness::Partial,
            upstream_availability: UpstreamAvailability::Available,
            checkpoint: checkpoint
                .map(SyncCheckpointCursor::parse)
                .transpose()
                .map_err(|error| text_violation(media_id, error))?,
            warnings: vec![own_media_warning()],
            extensions: Extensions::default(),
        };
        snapshot
            .validate()
            .map_err(|error| text_violation(media_id, error))?;
        let source_uuid = snapshot.social_source_id.to_string();
        let owner_value = owner.to_string();
        let payload_value = if kind == FactKind::Captured {
            envelope_value_at(
                &SocialSourceCaptured {
                    source: snapshot,
                    extensions: Extensions::default(),
                },
                &source_uuid,
                &owner_value,
                instant_from_time(captured_at, media_id)?,
            )?
        } else {
            envelope_value_at(
                &SocialSourceUpdated {
                    source: snapshot,
                    extensions: Extensions::default(),
                },
                &source_uuid,
                &owner_value,
                instant_from_time(captured_at, media_id)?,
            )?
        };
        let event_id = Uuid::now_v7();
        let result = sqlx::query(
            "insert into instagram_archive.outbox_events
             (event_id, event_type, aggregate_type, aggregate_id, payload,
              correlation_id, causation_id, occurred_at)
             values ($1, $2, 'media', $3, $4, $5, null, $6)
             on conflict do nothing",
        )
        .bind(event_id)
        .bind(kind.event_type())
        .bind(media_id)
        .bind(payload_value)
        .bind(event_id)
        .bind(captured_at)
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() == 1 {
            appended = appended.saturating_add(1);
        }
    }
    Ok(appended)
}

fn own_media_content_digest(
    provider_media_id: &str,
    permalink: &str,
    caption: Option<&str>,
    published_at: time::OffsetDateTime,
) -> Result<ContentDigest, PublishError> {
    let published = published_at
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|error| text_violation(Uuid::nil(), error))?;
    let value = serde_json::json!({
        "provider_media_id": provider_media_id,
        "permalink": permalink,
        "caption": caption,
        "published_at": published,
        "media": []
    });
    let bytes = serde_json::to_vec(&value)?;
    let mut hex = String::with_capacity(64);
    for byte in sha2::Sha256::digest(bytes) {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(ContentDigest {
        algorithm: DigestAlgorithm::Sha256,
        hex: DigestHex::parse(&hex).map_err(|error| violation(Uuid::nil(), &error))?,
    })
}

#[expect(
    clippy::expect_used,
    reason = "static contract literals fail loudly if their grammar changes"
)]
fn own_media_warning() -> WarningEnvelope {
    WarningEnvelope {
        code: ErrorCode::parse("social.source.media_not_archived")
            .expect("static warning code parses"),
        message: SafeMessage::parse(
            "Metadata-only sync: media bytes are not archived under the current media policy.",
        )
        .expect("static warning message parses"),
        field_path: None,
        extensions: Extensions::default(),
    }
}

/// Which fact a publication represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactKind {
    /// `social.source.captured.v1`: first preservation of this source.
    Captured,
    /// `social.source.updated.v1`: the normalized record changed.
    Updated,
}

impl FactKind {
    fn event_type(self) -> &'static str {
        match self {
            Self::Captured => SocialSourceCaptured::EVENT_TYPE,
            Self::Updated => SocialSourceUpdated::EVENT_TYPE,
        }
    }
}

/// Appends one social-source fact to the outbox inside the caller's open
/// transaction, so the fact commits or aborts with the state change it
/// describes. The stored payload is the complete canonical envelope, so a
/// redelivery is byte-identical and consumers need nothing but the row.
///
/// # Errors
///
/// [`PublishError`] when the snapshot cannot be built truthfully from stored
/// rows or the insert fails.
pub async fn append_fact(
    transaction: &mut PgConnection,
    kind: FactKind,
    capture_id: Uuid,
) -> Result<Uuid, PublishError> {
    let snapshot = build_snapshot(transaction, capture_id).await?;
    let source_uuid = snapshot.social_source_id.to_string();
    // The bare owner uuid: TenantRef's own Display carries the `user:` kind,
    // and the envelope template adds it back itself.
    let owner = snapshot.owner.user_id().0.to_string();
    let payload_value = if kind == FactKind::Captured {
        envelope_value(
            &SocialSourceCaptured {
                source: snapshot,
                extensions: Extensions::default(),
            },
            &source_uuid,
            &owner,
        )?
    } else {
        envelope_value(
            &SocialSourceUpdated {
                source: snapshot,
                extensions: Extensions::default(),
            },
            &source_uuid,
            &owner,
        )?
    };

    let event_id = Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.outbox_events \
         (event_id, event_type, aggregate_type, aggregate_id, payload, correlation_id, \
          causation_id, occurred_at) \
         values ($1, $2, 'capture', $3, $4, $5, null, $6)",
    )
    .bind(event_id)
    .bind(kind.event_type())
    .bind(capture_id)
    .bind(payload_value)
    // The column is a bare uuid; the namespaced form travels inside the envelope.
    .bind(event_id)
    .bind(time::OffsetDateTime::now_utc())
    .execute(&mut *transaction)
    .await?;
    Ok(event_id)
}

/// Wraps one payload into its complete canonical envelope as JSON. The
/// template carries every required envelope member; `set_payload` binds the
/// real type and body so a mismatched event type can never be read as a
/// social payload downstream.
fn envelope_value<P>(
    payload: &P,
    source_uuid: &str,
    owner: &str,
) -> Result<serde_json::Value, PublishError>
where
    P: EventPayload + serde::Serialize + Sync,
{
    envelope_value_at(payload, source_uuid, owner, WireTimestamp::now())
}

fn envelope_value_at<P>(
    payload: &P,
    source_uuid: &str,
    owner: &str,
    occurred_at: WireTimestamp,
) -> Result<serde_json::Value, PublishError>
where
    P: EventPayload + serde::Serialize + Sync,
{
    let template = serde_json::json!({
        "event_id": Uuid::now_v7().to_string(),
        "event_type": P::EVENT_TYPE,
        "occurred_at": occurred_at.to_wire(),
        "producer": PRODUCER_NAME,
        "aggregate_id": format!("social_source:{source_uuid}"),
        "correlation_id": format!("user:{owner}"),
        "tenant_id": format!("user:{owner}"),
        "schema_version": 1,
        "payload": {}
    });
    let mut envelope = EventEnvelope::from_json(serde_json::to_vec(&template)?.as_slice())
        .map_err(|error| {
            PublishError::ContractViolation(
                Uuid::nil(),
                format!(
                    "envelope template refused: {error} ({})",
                    std::error::Error::source(&error)
                        .map_or_else(|| "no cause".to_owned(), std::string::ToString::to_string)
                ),
            )
        })?;
    envelope.set_payload(payload).map_err(|error| {
        PublishError::ContractViolation(Uuid::nil(), format!("payload refused: {error}"))
    })?;
    let canonical = envelope.to_canonical_json().map_err(|error| {
        PublishError::ContractViolation(Uuid::nil(), format!("envelope re-render failed: {error}"))
    })?;
    Ok(serde_json::from_str(&canonical)?)
}

/// Appends a local removal fact inside the caller's open transaction.
///
/// Unlike an upstream deletion observation, this says only that Ratatoskr no
/// longer preserves the source. It intentionally works for unresolved
/// captures too: local removal needs the capture's owner and permalink, not a
/// provider representation.
///
/// # Errors
///
/// Returns [`PublishError`] when the capture is unknown, malformed, or the
/// outbox insert cannot be written.
pub async fn append_removal_fact(
    transaction: &mut PgConnection,
    capture_id: Uuid,
    reason: RemovalReason,
    removed_at: WireTimestamp,
) -> Result<Uuid, PublishError> {
    let (owner, canonical_url): (Uuid, String) = sqlx::query_as(
        "select user_ref, canonical_url from instagram_archive.captures where capture_id = $1",
    )
    .bind(capture_id)
    .fetch_one(&mut *transaction)
    .await
    .map_err(PublishError::Persistence)?;
    let permalink = crate::permalink::canonicalize(&canonical_url)
        .map_err(|_| PublishError::InvalidStoredPermalink(capture_id))?;
    append_source_removal_fact(
        transaction,
        owner,
        source_identity(owner, &permalink.url),
        "capture",
        capture_id,
        reason,
        removed_at,
    )
    .await
}

/// Appends a canonical removal request for an already-derived social source.
///
/// This narrow internal boundary lets privacy deletion publish official-media
/// removals after owner/source identity is established and before local rows
/// are erased.
pub(crate) async fn append_source_removal_fact(
    transaction: &mut PgConnection,
    owner: Uuid,
    source_id: Uuid,
    aggregate_type: &'static str,
    aggregate_id: Uuid,
    reason: RemovalReason,
    removed_at: WireTimestamp,
) -> Result<Uuid, PublishError> {
    let source_uuid = source_id.to_string();
    let owner_value = owner.to_string();
    let payload = SocialSourceRemoved {
        social_source_id: parse_source_id(
            Uuid::parse_str(&source_uuid).map_err(|error| text_violation(aggregate_id, error))?,
            aggregate_id,
        )?,
        owner: owner_ref(owner, aggregate_id)?,
        reason,
        removed_at,
        extensions: Extensions::default(),
    };
    let payload_value = envelope_value_at(&payload, &source_uuid, &owner_value, removed_at)?;
    let event_id = Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.outbox_events \
         (event_id, event_type, aggregate_type, aggregate_id, payload, correlation_id, \
          causation_id, occurred_at) \
         values ($1, $2, $3, $4, $5, $6, null, $7)",
    )
    .bind(event_id)
    .bind(SocialSourceRemoved::EVENT_TYPE)
    .bind(aggregate_type)
    .bind(aggregate_id)
    .bind(payload_value)
    .bind(event_id)
    .bind(
        time::OffsetDateTime::parse(
            &payload.removed_at.to_wire(),
            &time::format_description::well_known::Rfc3339,
        )
        .map_err(|error| text_violation(aggregate_id, error))?,
    )
    .execute(&mut *transaction)
    .await?;
    Ok(event_id)
}

/// Reads stored rows back into one truthful snapshot.
async fn build_snapshot(
    connection: &mut PgConnection,
    capture_id: Uuid,
) -> Result<SocialSourceSnapshot, PublishError> {
    let (owner, canonical_url, acquisition_wire, captured_at): (
        Uuid,
        String,
        String,
        time::OffsetDateTime,
    ) = sqlx::query_as(
        "select user_ref, canonical_url, acquisition_method, captured_at \
         from instagram_archive.captures where capture_id = $1",
    )
    .bind(capture_id)
    .fetch_one(&mut *connection)
    .await
    .map_err(PublishError::Persistence)?;

    let permalink = crate::permalink::canonicalize(&canonical_url)
        .map_err(|_| PublishError::InvalidStoredPermalink(capture_id))?;
    let acquisition = match acquisition_wire.as_str() {
        "official_api" => AcquisitionMethod::OfficialApi,
        "share_extension" => AcquisitionMethod::ShareExtension,
        "browser_extension" => AcquisitionMethod::BrowserExtension,
        "public_resolution" => AcquisitionMethod::PublicResolution,
        "data_export" => AcquisitionMethod::DataExport,
        "legacy_import" => AcquisitionMethod::LegacyImport,
        other => {
            return Err(PublishError::ContractViolation(
                capture_id,
                format!("unknown acquisition token {other}"),
            ));
        }
    };

    // Normalized content of the newest revision; nothing here is invented.
    // Media bytes are never archived by this lane yet, so media stays empty
    // and completeness declares partial with the warning naming the gap.
    let Some((caption, raw_blob_hex, byte_size, upstream_status)) =
        sqlx::query_as::<_, (Option<String>, String, i64, Option<String>)>(
            "select m.caption, b.blob_ref, b.byte_size, m.upstream_status \
         from instagram_archive.captures c \
         join instagram_archive.media m on m.media_id = c.media_id \
         join instagram_archive.media_revisions r on r.revision_id = m.current_revision_id \
         join instagram_archive.raw_records b on b.raw_record_id = r.raw_record_id \
         where c.capture_id = $1",
        )
        .bind(capture_id)
        .fetch_optional(&mut *connection)
        .await
        .map_err(PublishError::Persistence)?
    else {
        return Err(PublishError::NothingToPublish(capture_id));
    };

    let text = match caption {
        Some(ref body) if !body.is_empty() => Some(PostText::parse(body).map_err(|error| {
            text_violation(capture_id, format!("stored caption rejected: {error}"))
        })?),
        _ => None,
    };
    let upstream = match upstream_status.as_deref() {
        Some("available") | None => UpstreamAvailability::Available,
        Some("deleted") => UpstreamAvailability::DeletedUpstream,
        Some(_) => UpstreamAvailability::Unavailable,
    };

    Ok(SocialSourceSnapshot {
        social_source_id: parse_source_id(source_identity(owner, &permalink.url), capture_id)?,
        platform: Platform::parse(SOCIAL_PLATFORM)
            .map_err(|error| violation(capture_id, &error))?,
        external_post_id: EntityLocalId::parse(&permalink.shortcode)
            .map_err(|error| violation(capture_id, &error))?,
        permalink: Some(
            PostPermalink::parse(&permalink.url).map_err(|error| violation(capture_id, &error))?,
        ),
        owner: owner_ref(owner, capture_id)?,
        author: None,
        published_at: None,
        captured_at: instant_from_time(captured_at, capture_id)?,
        text,
        media: Vec::new(),
        relations: Vec::new(),
        folders: Vec::new(),
        content_digest: content_digest(),
        raw_blob: Some(BlobRef {
            owner_service: BlobOwner::parse(PRODUCER_NAME)
                .map_err(|error| violation(capture_id, &error))?,
            digest: ContentDigest {
                algorithm: DigestAlgorithm::Sha256,
                hex: DigestHex::parse(&raw_blob_hex)
                    .map_err(|error| violation(capture_id, &error))?,
            },
            media_type: MediaType::parse("application/json")
                .map_err(|error| violation(capture_id, &error))?,
            length_bytes: u64::try_from(byte_size)
                .map_err(|error| text_violation(capture_id, error))?,
        }),
        acquisition,
        saved_authority: SavedAuthority::ExplicitUserCapture,
        completeness: CaptureCompleteness::Partial,
        upstream_availability: upstream,
        checkpoint: None,
        warnings: vec![media_warning()],
        extensions: Extensions::default(),
    })
}

#[expect(
    clippy::expect_used,
    reason = "static contract literals; a typo fails this test loudly instead of publishing"
)]
fn media_warning() -> WarningEnvelope {
    WarningEnvelope {
        code: ErrorCode::parse("social.source.media_not_archived")
            .expect("a static code literal parses"),
        message: SafeMessage::parse(
            "Metadata-only capture: media bytes are not archived under the current media policy.",
        )
        .expect("a static message literal parses"),
        field_path: None,
        extensions: Extensions::default(),
    }
}

/// Digest over the normalized content shape this producer publishes today:
/// metadata-only oEmbed normalization with no archived media bytes. Stable
/// across emissions so consumer-side recomputation mismatches mean corruption.
#[expect(
    clippy::expect_used,
    reason = "the hex is self-produced SHA-256; it parses or the hasher is broken"
)]
pub(crate) fn content_digest() -> ContentDigest {
    let mut hex = String::with_capacity(64);
    for byte in sha2::Sha256::digest(b"instagram/oembed-metadata-only/v1") {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    ContentDigest {
        algorithm: DigestAlgorithm::Sha256,
        hex: DigestHex::parse(&hex).expect("a self-produced SHA-256 hex parses"),
    }
}

fn owner_ref(owner: Uuid, capture_id: Uuid) -> Result<TenantRef, PublishError> {
    TenantRef::parse(&format!("user:{owner}")).map_err(|error| text_violation(capture_id, error))
}

fn parse_source_id(
    value: Uuid,
    capture_id: Uuid,
) -> Result<ratatoskr_identifiers::SocialSourceId, PublishError> {
    ratatoskr_identifiers::SocialSourceId::parse(&value.to_string())
        .map_err(|error| text_violation(capture_id, error))
}

/// Converts the storage clock type into the contract instant via its canonical
/// wire form.
pub(crate) fn instant_from_time(
    value: time::OffsetDateTime,
    capture_id: Uuid,
) -> Result<WireTimestamp, PublishError> {
    let rendered = value
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|error| text_violation(capture_id, error))?;
    let canonical = rendered.replace("+00:00", "Z");
    WireTimestamp::parse(&canonical).map_err(|error| text_violation(capture_id, error))
}

// ---------------------------------------------------------------------------
// Publisher loop
// ---------------------------------------------------------------------------

/// Metric names emitted by the publisher pass; rendered on `/metrics`.
/// Counter of facts delivered and marked published.
pub const OUTBOX_DELIVERED_TOTAL: &str = "instagram_outbox_delivered_total";
/// Counter of delivery attempts that failed and stayed unpublished.
pub const OUTBOX_FAILED_TOTAL: &str = "instagram_outbox_failed_total";
/// Counter of deliveries that were redeliveries of a previously failed fact.
pub const OUTBOX_REDELIVERED_TOTAL: &str = "instagram_outbox_redelivered_total";
/// Gauge of outbox rows still waiting for their first successful delivery.
pub const OUTBOX_UNPUBLISHED_DEPTH: &str = "instagram_outbox_unpublished_depth";

/// Why a delivery attempt could not complete. The message is safe for logs:
/// it describes transport behaviour, never payload content.
#[derive(Debug, thiserror::Error)]
#[error("event delivery failed: {0}")]
pub struct TransportError(pub String);

/// The seam between the outbox and whatever carries facts to consumers.
///
/// Implementations must treat one call as at-least-once permission: the row is
/// only marked published after `deliver` returns `Ok`, so a crash in between
/// redelivers the identical stored bytes.
pub trait EventTransport: Send + Sync {
    /// Delivers one canonical envelope body.
    ///
    /// # Errors
    ///
    /// [`TransportError`] when the fact did not reach its carrier.
    fn deliver(
        &self,
        event_id: Uuid,
        envelope_json: &str,
    ) -> impl std::future::Future<Output = Result<(), TransportError>> + Send;
}

/// One publisher pass over the unpublished outbox rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PassSummary {
    /// Facts delivered and marked published.
    pub delivered: u32,
    /// Facts whose delivery failed; they stay unpublished for redelivery.
    pub failed: u32,
    /// Facts still waiting after this pass.
    pub remaining: u64,
}

/// Runs exactly one claiming pass: oldest first, bounded by `batch`.
///
/// Delivery happens outside any transaction; only the mark or the failure
/// bookkeeping touches storage afterwards, so a slow carrier never holds row
/// locks and a crash between delivery and marking yields a byte-identical
/// redelivery.
///
/// # Errors
///
/// [`sqlx::Error`] when a claim, mark, or metrics read fails.
pub async fn run_once<T: EventTransport>(
    pool: &sqlx::PgPool,
    transport: &T,
    batch: u32,
) -> Result<PassSummary, sqlx::Error> {
    let mut summary = PassSummary::default();

    let rows: Vec<(Uuid, serde_json::Value, i32)> = sqlx::query_as(
        "select event_id, payload, attempt_count from instagram_archive.outbox_events \
         where published_at is null order by event_id limit $1",
    )
    .bind(i32::try_from(batch).unwrap_or(i32::MAX))
    .fetch_all(pool)
    .await?;

    for (event_id, payload_value, attempt_count) in rows {
        let body = serde_json::to_string(&payload_value)
            .map_err(|error| sqlx::Error::Encode(Box::new(error)))?;
        if attempt_count > 0 {
            metrics::counter!(OUTBOX_REDELIVERED_TOTAL).increment(1);
        }
        match transport.deliver(event_id, &body).await {
            Ok(()) => {
                sqlx::query(
                    "update instagram_archive.outbox_events \
                     set published_at = now() \
                     where event_id = $1 and published_at is null",
                )
                .bind(event_id)
                .execute(pool)
                .await?;
                metrics::counter!(OUTBOX_DELIVERED_TOTAL).increment(1);
                summary.delivered += 1;
            }
            Err(error) => {
                tracing::warn!(event = %event_id, reason = %error, "outbox delivery failed");
                sqlx::query(
                    "update instagram_archive.outbox_events \
                     set attempt_count = attempt_count + 1, \
                         next_attempt_at = now() + interval '60 seconds' \
                     where event_id = $1",
                )
                .bind(event_id)
                .execute(pool)
                .await?;
                metrics::counter!(OUTBOX_FAILED_TOTAL).increment(1);
                summary.failed += 1;
            }
        }
    }

    let (remaining,): (i64,) = sqlx::query_as(
        "select count(*) from instagram_archive.outbox_events where published_at is null",
    )
    .fetch_one(pool)
    .await?;
    summary.remaining = u64::try_from(remaining).unwrap_or(u64::MAX);
    let depth = f64::from(u32::try_from(summary.remaining).unwrap_or(u32::MAX));
    metrics::gauge!(OUTBOX_UNPUBLISHED_DEPTH, "producer" => PRODUCER_NAME).set(depth);

    Ok(summary)
}
