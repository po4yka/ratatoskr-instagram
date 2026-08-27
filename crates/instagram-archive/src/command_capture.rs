//! Provider-specific validation for explicit browser capture commands.

use ratatoskr_event_envelope::CommandEnvelope;
use ratatoskr_identifiers::ContentDigest;
use ratatoskr_social_contracts::{
    AcquisitionMethod, SavedAuthority, SocialCaptureProvider, SocialCaptureRequested,
};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::Database;
use crate::capture::{
    CaptureError, CaptureRequest, CaptureSubmission, ClientSource, submit_capture_in_transaction,
};

/// Stable inbox identity for Platform's Instagram browser-capture command.
const BROWSER_CAPTURE_CONSUMER: &str = "ratatoskr-instagram-browser-capture";

/// A validated browser capture that can be persisted by the Instagram owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserCaptureCommand {
    /// The broker delivery identity used for durable inbox deduplication.
    pub command_id: Uuid,
    /// The Ratatoskr user that owns the capture.
    pub user_ref: Uuid,
    /// The Platform operation to progress after preservation.
    pub operation_id: Uuid,
    /// The original public Instagram permalink.
    pub original_permalink: String,
    /// The moment of the explicit browser action.
    pub captured_at: OffsetDateTime,
    /// The stable Platform intent digest, retained only for correlation.
    pub idempotency_key: ContentDigest,
    /// The explicitly declared client source.
    pub client_source: ClientSource,
}

/// Why an incoming social command cannot be handled by Instagram.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CommandCaptureError {
    /// The provider-specific subject was not the Instagram capture subject.
    #[error("the command subject is not the Instagram browser-capture subject")]
    WrongSubject,
    /// The command envelope or its typed payload was invalid.
    #[error("the command envelope is invalid")]
    InvalidEnvelope,
    /// A Platform command did not name a user owner.
    #[error("the command does not name an owning user")]
    MissingOwner,
    /// The typed payload named another social owner.
    #[error("the command provider is not Instagram")]
    WrongProvider,
    /// The command did not carry the closed browser-extension acquisition.
    #[error("the command acquisition is not browser_extension")]
    WrongAcquisition,
    /// The command did not carry the closed explicit-user-capture authority.
    #[error("the command saved authority is not explicit_user_capture")]
    WrongSavedAuthority,
    /// The command timestamp did not parse as an RFC 3339 instant.
    #[error("the command captured_at timestamp is invalid")]
    InvalidCapturedAt,
    /// The explicit capture could not be preserved in the archive.
    #[error("the browser capture could not be persisted")]
    Capture(#[from] CaptureError),
    /// The service could not record the durable inbox claim.
    #[error("the browser capture inbox could not be persisted")]
    Persistence(#[from] sqlx::Error),
}

/// The durable result of accepting one at-least-once command delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserCaptureIngested {
    /// A previously unseen delivery preserved or reused one explicit capture.
    Preserved(CaptureSubmission),
    /// The command delivery had already completed in this consumer's inbox.
    Duplicate,
}

/// Validates one Platform command delivered to the Instagram subject.
///
/// # Errors
///
/// Returns [`CommandCaptureError`] when the subject, envelope, provider, or
/// closed browser provenance does not belong to this consumer.
pub fn decode_browser_capture_command(
    subject: &str,
    envelope_json: &[u8],
) -> Result<BrowserCaptureCommand, CommandCaptureError> {
    if subject != "cmd.instagram.capture.requested.v1" {
        return Err(CommandCaptureError::WrongSubject);
    }
    let envelope = CommandEnvelope::from_json(envelope_json)
        .map_err(|_| CommandCaptureError::InvalidEnvelope)?;
    let payload = envelope
        .payload_as::<SocialCaptureRequested>()
        .map_err(|_| CommandCaptureError::InvalidEnvelope)?;
    if payload.provider != SocialCaptureProvider::Instagram {
        return Err(CommandCaptureError::WrongProvider);
    }
    if payload.acquisition != AcquisitionMethod::BrowserExtension {
        return Err(CommandCaptureError::WrongAcquisition);
    }
    if payload.saved_authority != SavedAuthority::ExplicitUserCapture {
        return Err(CommandCaptureError::WrongSavedAuthority);
    }
    let owner = envelope
        .tenant_id
        .ok_or(CommandCaptureError::MissingOwner)?;
    let captured_at = OffsetDateTime::parse(
        &payload.captured_at.to_wire(),
        &time::format_description::well_known::Rfc3339,
    )
    .map_err(|_| CommandCaptureError::InvalidCapturedAt)?;

    Ok(BrowserCaptureCommand {
        command_id: envelope.command_id.0,
        user_ref: owner.user_id().0,
        operation_id: payload.operation_id.0,
        original_permalink: payload.original_permalink.as_str().to_owned(),
        captured_at,
        idempotency_key: payload.idempotency_key,
        client_source: ClientSource::BrowserExtension,
    })
}

impl Database {
    /// Persists one validated Instagram browser-capture command exactly once.
    ///
    /// The inbox claim is held open until the capture is stored, so a failed
    /// capture transaction does not suppress a broker redelivery. The capture
    /// itself remains idempotent by its owned `(user_ref, canonical_url)` key.
    ///
    /// # Errors
    ///
    /// Returns [`CommandCaptureError`] for an invalid command or a failed
    /// archive transaction.
    pub async fn ingest_browser_capture_command(
        &self,
        subject: &str,
        envelope_json: &[u8],
    ) -> Result<BrowserCaptureIngested, CommandCaptureError> {
        let command = decode_browser_capture_command(subject, envelope_json)?;
        let mut transaction = self.pool().begin().await?;
        let claimed: Option<(i32,)> = sqlx::query_as(
            "insert into instagram_archive.inbox_events \
             (consumer_name, event_id, consumed_at, handler_outcome) \
             values ($1, $2, now(), 'processed') \
             on conflict do nothing returning 1",
        )
        .bind(BROWSER_CAPTURE_CONSUMER)
        .bind(command.command_id)
        .fetch_optional(&mut *transaction)
        .await?;
        if claimed.is_none() {
            transaction.commit().await?;
            return Ok(BrowserCaptureIngested::Duplicate);
        }

        let submission = submit_capture_in_transaction(
            &mut transaction,
            &CaptureRequest {
                user_ref: command.user_ref,
                url: command.original_permalink,
                captured_at: command.captured_at,
                client_source: command.client_source,
                note: None,
                client_idempotency_key: Some(command.idempotency_key.hex.to_string()),
            },
        )
        .await?;
        sqlx::query(
            "update instagram_archive.inbox_events set handler_outcome = 'processed' \
             where consumer_name = $1 and event_id = $2",
        )
        .bind(BROWSER_CAPTURE_CONSUMER)
        .bind(command.command_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(BrowserCaptureIngested::Preserved(submission))
    }
}
