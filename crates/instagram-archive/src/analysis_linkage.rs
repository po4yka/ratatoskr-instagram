//! Inbox handling for Knowledge analysis-completion facts.
//!
//! Knowledge owns the analysis result. This module stores only the durable
//! fact that a specific captured revision completed analysis, which lets the
//! capture remain the local record of that linkage without copying analysis
//! data across bounded contexts.

use ratatoskr_event_envelope::EventEnvelope;
use ratatoskr_social_contracts::SocialSourceAnalysisCompleted;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::Database;
use crate::publishing::{content_digest, source_identity};

/// Stable inbox consumer identity for Knowledge completion facts.
const ANALYSIS_CONSUMER: &str = "ratatoskr-instagram-analysis";

/// Result of accepting one at-least-once Knowledge completion delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisCompletionOutcome {
    /// The fact linked one local capture to its completed analysis revision.
    Linked,
    /// This service has already handled the delivery idempotently.
    Duplicate,
    /// The fact is valid but does not describe a live local capture revision.
    Skipped,
}

/// Why a completion delivery could not be handled.
#[derive(Debug, thiserror::Error)]
pub enum AnalysisCompletionError {
    /// The input was not a valid event envelope.
    #[error("the completion delivery is not a valid event envelope: {0}")]
    InvalidEnvelope(String),
    /// The envelope did not hold the typed Knowledge completion payload.
    #[error("the completion delivery does not hold a Knowledge completion payload: {0}")]
    InvalidPayload(String),
    /// The envelope event identity was not a UUID.
    #[error("the completion delivery event id is invalid: {0}")]
    InvalidEventId(String),
    /// The completion instant was not a canonical timestamp.
    #[error("the completion delivery timestamp is invalid: {0}")]
    InvalidTimestamp(String),
    /// An archive-owned query failed.
    #[error("the completion linkage could not be stored")]
    Persistence(#[from] sqlx::Error),
}

impl Database {
    /// Consumes one typed `knowledge.analysis.completed.v1` delivery.
    ///
    /// The inbox claim and linkage write share one transaction. A replay with
    /// the same envelope id returns [`AnalysisCompletionOutcome::Duplicate`];
    /// a valid fact for another source, stale digest, or tombstoned capture is
    /// recorded as skipped so redelivery cannot repeatedly search the archive.
    ///
    /// # Errors
    ///
    /// Returns [`AnalysisCompletionError`] when the envelope is malformed or
    /// this service cannot read or write its owned schema.
    pub async fn ingest_analysis_completed(
        &self,
        envelope_json: &[u8],
    ) -> Result<AnalysisCompletionOutcome, AnalysisCompletionError> {
        let envelope = EventEnvelope::from_json(envelope_json)
            .map_err(|error| AnalysisCompletionError::InvalidEnvelope(error.to_string()))?;
        let event_id = Uuid::parse_str(&envelope.event_id.to_string())
            .map_err(|error| AnalysisCompletionError::InvalidEventId(error.to_string()))?;
        let completion = envelope
            .payload_as::<SocialSourceAnalysisCompleted>()
            .map_err(|error| AnalysisCompletionError::InvalidPayload(error.to_string()))?;
        let completed_at = time::OffsetDateTime::parse(
            &completion.completed_at.to_wire(),
            &time::format_description::well_known::Rfc3339,
        )
        .map_err(|error| AnalysisCompletionError::InvalidTimestamp(error.to_string()))?;

        let mut transaction = self.pool().begin().await?;
        let claimed: Option<(i32,)> = sqlx::query_as(
            "insert into instagram_archive.inbox_events \
             (consumer_name, event_id, consumed_at, handler_outcome) \
             values ($1, $2, now(), 'processed') \
             on conflict do nothing returning 1",
        )
        .bind(ANALYSIS_CONSUMER)
        .bind(event_id)
        .fetch_optional(&mut *transaction)
        .await?;
        if claimed.is_none() {
            transaction.commit().await?;
            return Ok(AnalysisCompletionOutcome::Duplicate);
        }

        let outcome = link_matching_capture(&mut transaction, &completion, completed_at).await?;
        sqlx::query(
            "update instagram_archive.inbox_events set handler_outcome = $3 \
             where consumer_name = $1 and event_id = $2",
        )
        .bind(ANALYSIS_CONSUMER)
        .bind(event_id)
        .bind(if outcome == AnalysisCompletionOutcome::Linked {
            "processed"
        } else {
            "skipped"
        })
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(outcome)
    }
}

async fn link_matching_capture(
    transaction: &mut PgConnection,
    completion: &SocialSourceAnalysisCompleted,
    completed_at: time::OffsetDateTime,
) -> Result<AnalysisCompletionOutcome, sqlx::Error> {
    if completion.content_digest != content_digest() {
        return Ok(AnalysisCompletionOutcome::Skipped);
    }
    let owner = completion.owner.user_id().0;
    let wanted_source = completion.social_source_id.to_string();
    let candidates: Vec<(Uuid, String)> = sqlx::query_as(
        "select c.capture_id, c.canonical_url from instagram_archive.captures c \
         where c.user_ref = $1 and c.media_id is not null and c.status <> 'tombstoned' \
         and not exists (select 1 from instagram_archive.capture_tombstones t \
                         where t.capture_id = c.capture_id)",
    )
    .bind(owner)
    .fetch_all(&mut *transaction)
    .await?;
    let Some((capture_id, _)) = candidates.into_iter().find(|(_, canonical_url)| {
        source_identity(owner, canonical_url).to_string() == wanted_source
    }) else {
        return Ok(AnalysisCompletionOutcome::Skipped);
    };

    sqlx::query(
        "insert into instagram_archive.capture_analysis_links \
         (capture_id, content_digest, completed_at) values ($1, $2, $3) \
         on conflict do nothing",
    )
    .bind(capture_id)
    .bind(completion.content_digest.hex.to_string())
    .bind(completed_at)
    .execute(&mut *transaction)
    .await?;
    Ok(AnalysisCompletionOutcome::Linked)
}
