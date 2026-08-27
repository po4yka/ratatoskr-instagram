//! Owner-scoped `SocialSource` facts built from Data Export observations.

use ratatoskr_error_contracts::{ErrorCode, WarningEnvelope};
use ratatoskr_identifiers::{
    BlobRef, ContentDigest, DigestAlgorithm, DigestHex, EntityLocalId, Extensions, SafeMessage,
};
use ratatoskr_social_contracts::{
    AcquisitionMethod, CaptureCompleteness, Platform, PostPermalink, SavedAuthority,
    SocialSourceCaptured, SocialSourceSnapshot, SocialSourceUpdated, UpstreamAvailability,
};
use sqlx::PgConnection;
use uuid::Uuid;

use super::{
    FactKind, PublishError, SOCIAL_PLATFORM, envelope_value_at, instant_from_time,
    own_media_source_identity, owner_ref, parse_source_id, text_violation, violation,
};

/// Appends one owner-scoped observation unless the exact normalized digest
/// has already been published for this owner/source pair.
#[expect(
    clippy::too_many_arguments,
    reason = "the caller passes each stored evidence field explicitly into the contract builder"
)]
pub(crate) async fn append_fact(
    transaction: &mut PgConnection,
    media_id: Uuid,
    owner: Uuid,
    provider_media_id: &str,
    permalink: &str,
    semantic_digest: &str,
    archive: BlobRef,
    observed_at: time::OffsetDateTime,
) -> Result<bool, PublishError> {
    let source_id = own_media_source_identity(owner, provider_media_id);
    let source_uuid = source_id.to_string();
    let prior: Vec<Option<String>> = sqlx::query_scalar(
        "select payload #>> '{payload,source,content_digest,hex}'
         from instagram_archive.outbox_events
         where aggregate_type = 'media' and aggregate_id = $1
           and event_type in ('social.source.captured.v1', 'social.source.updated.v1')
         order by occurred_at, event_id",
    )
    .bind(source_id)
    .fetch_all(&mut *transaction)
    .await?;
    if prior
        .iter()
        .any(|digest| digest.as_deref() == Some(semantic_digest))
    {
        return Ok(false);
    }
    let kind = if prior.is_empty() {
        FactKind::Captured
    } else {
        FactKind::Updated
    };
    let snapshot = SocialSourceSnapshot {
        social_source_id: parse_source_id(source_id, media_id)?,
        platform: Platform::parse(SOCIAL_PLATFORM).map_err(|error| violation(media_id, &error))?,
        external_post_id: EntityLocalId::parse(provider_media_id)
            .map_err(|error| violation(media_id, &error))?,
        permalink: Some(
            PostPermalink::parse(permalink).map_err(|error| violation(media_id, &error))?,
        ),
        owner: owner_ref(owner, media_id)?,
        author: None,
        published_at: None,
        captured_at: instant_from_time(observed_at, media_id)?,
        text: None,
        media: Vec::new(),
        relations: Vec::new(),
        folders: Vec::new(),
        content_digest: ContentDigest {
            algorithm: DigestAlgorithm::Sha256,
            hex: DigestHex::parse(semantic_digest).map_err(|error| violation(media_id, &error))?,
        },
        raw_blob: Some(archive),
        acquisition: AcquisitionMethod::DataExport,
        saved_authority: SavedAuthority::ExportObservation,
        completeness: CaptureCompleteness::Partial,
        upstream_availability: UpstreamAvailability::Available,
        checkpoint: None,
        warnings: vec![partial_warning()],
        extensions: Extensions::default(),
    };
    snapshot
        .validate()
        .map_err(|error| text_violation(media_id, error))?;
    let owner_value = owner.to_string();
    let occurred_at = instant_from_time(observed_at, media_id)?;
    let payload_value = if kind == FactKind::Captured {
        envelope_value_at(
            &SocialSourceCaptured {
                source: snapshot,
                extensions: Extensions::default(),
            },
            &source_uuid,
            &owner_value,
            occurred_at,
        )?
    } else {
        envelope_value_at(
            &SocialSourceUpdated {
                source: snapshot,
                extensions: Extensions::default(),
            },
            &source_uuid,
            &owner_value,
            occurred_at,
        )?
    };
    let event_id = Uuid::now_v7();
    let inserted = sqlx::query(
        "insert into instagram_archive.outbox_events
         (event_id, event_type, aggregate_type, aggregate_id, payload,
          correlation_id, causation_id, occurred_at)
         values ($1, $2, 'media', $3, $4, $5, null, $6)
         on conflict do nothing",
    )
    .bind(event_id)
    .bind(kind.event_type())
    .bind(source_id)
    .bind(payload_value)
    .bind(event_id)
    .bind(observed_at)
    .execute(&mut *transaction)
    .await?;
    Ok(inserted.rows_affected() == 1)
}

#[expect(
    clippy::expect_used,
    reason = "static contract literals fail loudly if their grammar changes"
)]
fn partial_warning() -> WarningEnvelope {
    WarningEnvelope {
        code: ErrorCode::parse("social.source.export_observation_partial")
            .expect("static warning code parses"),
        message: SafeMessage::parse(
            "Data Export observation: media bytes and current upstream availability are not verified.",
        )
        .expect("static warning message parses"),
        field_path: None,
        extensions: Extensions::default(),
    }
}
