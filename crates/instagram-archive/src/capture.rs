//! Explicit capture intake persistence.
//!
//! Capture identity is the pair `(user_ref, canonical_url)`, enforced by a
//! database uniqueness constraint, so duplicate deliveries of one share
//! converge on the original record. The unavailable fallback appends an
//! availability observation and moves the capture to `unavailable`, touching
//! nothing else: no media row, no content, no rewritten timestamps.

use time::OffsetDateTime;
use uuid::Uuid;

use crate::capability::AvailabilityObservationKind;
use crate::database::Database;
use crate::permalink::{self, PermalinkError};

/// Which Ratatoskr client delivered the capture.
///
/// The inventory mirrors `captures_client_source_check`. A variant may exist
/// here while having no wire acquisition method in the contract grammar yet;
/// such deliveries are refused until a reviewed change extends that grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClientSource {
    /// The iOS Share Extension.
    IosShareExtension,
    /// The Android Share Target.
    AndroidShareTarget,
    /// The browser extension.
    BrowserExtension,
    /// Telegram forwarding — no contract acquisition method exists for it yet.
    Telegram,
}

impl ClientSource {
    /// Every client source, exactly the declared inventory.
    pub const ALL: [ClientSource; 4] = [
        ClientSource::IosShareExtension,
        ClientSource::AndroidShareTarget,
        ClientSource::BrowserExtension,
        ClientSource::Telegram,
    ];

    /// Parses the wire value delivered by the platform grammar.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "ios_share_extension" => Some(Self::IosShareExtension),
            "android_share_target" => Some(Self::AndroidShareTarget),
            "browser_extension" => Some(Self::BrowserExtension),
            "telegram" => Some(Self::Telegram),
            _ => None,
        }
    }

    /// The `snake_case` wire value stored in `captures.client_source`.
    #[must_use]
    pub const fn wire_value(self) -> &'static str {
        match self {
            Self::IosShareExtension => "ios_share_extension",
            Self::AndroidShareTarget => "android_share_target",
            Self::BrowserExtension => "browser_extension",
            Self::Telegram => "telegram",
        }
    }

    /// The wire acquisition method this client source implies, or `None` when
    /// the contract grammar has no honest value for it yet.
    #[must_use]
    pub const fn acquisition_wire_method(self) -> Option<&'static str> {
        match self {
            Self::IosShareExtension | Self::AndroidShareTarget => Some("share_extension"),
            Self::BrowserExtension => Some("browser_extension"),
            Self::Telegram => None,
        }
    }
}

/// The lifecycle status of a capture, mirroring `captures_status_check`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CaptureStatus {
    /// Intake accepted the capture; resolution has not concluded.
    Accepted,
    /// Public resolution produced a representation (plan item 4).
    Resolved,
    /// Resolution failed and the attempt is preserved as unavailable.
    Unavailable,
    /// Intake or processing failed after creation.
    Failed,
    /// The user or retention policy removed the local preserved source.
    Tombstoned,
}

impl CaptureStatus {
    /// Parses the wire value stored in `captures.status`.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "accepted" => Some(Self::Accepted),
            "resolved" => Some(Self::Resolved),
            "unavailable" => Some(Self::Unavailable),
            "failed" => Some(Self::Failed),
            "tombstoned" => Some(Self::Tombstoned),
            _ => None,
        }
    }

    /// The `snake_case` wire value stored in `captures.status`.
    #[must_use]
    pub const fn wire_value(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Resolved => "resolved",
            Self::Unavailable => "unavailable",
            Self::Failed => "failed",
            Self::Tombstoned => "tombstoned",
        }
    }
}

/// One explicit capture submission before persistence.
#[derive(Debug, Clone)]
pub struct CaptureRequest {
    /// The Ratatoskr user acting; minted by the platform, never guessed here.
    pub user_ref: Uuid,
    /// The URL exactly as the client delivered it.
    pub url: String,
    /// When the user performed the save, per the submitting client.
    pub captured_at: OffsetDateTime,
    /// Which Ratatoskr client delivered the capture.
    pub client_source: ClientSource,
    /// Optional private user note.
    pub note: Option<String>,
    /// The platform operation key, stored for correlation only.
    pub client_idempotency_key: Option<String>,
}

/// One persisted explicit capture with its provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureRecord {
    /// The locally minted identity of the capture.
    pub capture_id: Uuid,
    /// The canonical permalink the submission resolved to.
    pub canonical_url: String,
    /// The wire acquisition method implied by the client source.
    pub acquisition_method: String,
    /// Always `explicit_user_capture`: what an explicit capture proves.
    pub saved_authority: String,
    /// The wire value of the delivering client source.
    pub client_source: String,
    /// The current lifecycle status.
    pub status: CaptureStatus,
    /// The private user note, if any was supplied.
    pub note: Option<String>,
    /// When the user performed the save.
    pub captured_at: OffsetDateTime,
}

/// The outcome of one intake submission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureSubmission {
    /// No capture existed for `(user_ref, canonical_url)`; one was created.
    Created(CaptureRecord),
    /// An earlier capture already held the identity; it is reused untouched.
    Reused(CaptureRecord),
}

impl CaptureSubmission {
    /// The capture record either way.
    #[must_use]
    pub const fn record(&self) -> &CaptureRecord {
        match self {
            Self::Created(record) | Self::Reused(record) => record,
        }
    }

    /// Whether this submission reused an existing capture instead of creating.
    #[must_use]
    pub const fn is_reuse(&self) -> bool {
        matches!(self, Self::Reused(_))
    }
}

/// Why intake or fallback refused to act.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CaptureError {
    /// The submitted URL is not a supported Instagram permalink.
    #[error("the submitted URL is not a supported Instagram permalink")]
    InvalidUrl(#[source] PermalinkError),
    /// The client source has no supported acquisition method in the contract
    /// grammar yet.
    #[error("the client source has no supported acquisition method yet")]
    UnsupportedClientSource,
    /// No capture exists under the given id.
    #[error("no capture exists under this id")]
    UnknownCapture,
    /// An archive-owned query failed.
    #[error("an instagram_archive database query failed")]
    Persistence(#[source] sqlx::Error),
}

/// The authority every explicit capture carries, as the stored wire value.
const EXPLICIT_AUTHORITY: &str = "explicit_user_capture";

impl Database {
    /// Submits one explicit capture.
    ///
    /// The URL is canonicalized first; the client source must map onto a
    /// contract acquisition method. Persistence runs in one transaction:
    /// the insert takes the uniqueness constraint on
    /// `(user_ref, canonical_url)`, and a lost race reads the winner back.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError`] when the URL is not a permalink, the client
    /// source has no acquisition method, or a query fails.
    pub async fn submit_capture(
        &self,
        request: &CaptureRequest,
    ) -> Result<CaptureSubmission, CaptureError> {
        let mut transaction = self
            .pool()
            .begin()
            .await
            .map_err(CaptureError::Persistence)?;
        let submission = submit_capture_in_transaction(&mut transaction, request).await?;
        transaction
            .commit()
            .await
            .map_err(CaptureError::Persistence)?;
        Ok(submission)
    }
}

/// Applies explicit capture persistence within a caller-owned transaction.
///
/// The caller owns commit or rollback, which lets a broker inbox claim and its
/// capture mutation become one atomic at-least-once delivery.
pub(crate) async fn submit_capture_in_transaction(
    transaction: &mut sqlx::PgConnection,
    request: &CaptureRequest,
) -> Result<CaptureSubmission, CaptureError> {
    let permalink = permalink::canonicalize(&request.url).map_err(CaptureError::InvalidUrl)?;
    let Some(acquisition_method) = request.client_source.acquisition_wire_method() else {
        return Err(CaptureError::UnsupportedClientSource);
    };
    let inserted = sqlx::query(
        "insert into instagram_archive.captures \
             (capture_id, user_ref, canonical_url, acquisition_method, saved_authority, \
              client_source, status, note, client_idempotency_key, captured_at) \
             values ($1, $2, $3, $4, $5, $6, 'accepted', $7, $8, $9) \
             on conflict on constraint captures_user_canonical_key do nothing",
    )
    .bind(Uuid::now_v7())
    .bind(request.user_ref)
    .bind(&permalink.url)
    .bind(acquisition_method)
    .bind(EXPLICIT_AUTHORITY)
    .bind(request.client_source.wire_value())
    .bind(request.note.as_deref())
    .bind(request.client_idempotency_key.as_deref())
    .bind(request.captured_at)
    .execute(&mut *transaction)
    .await
    .map_err(CaptureError::Persistence)?;

    // One read path serves both branches: on a won race the winner is the
    // row just inserted; on a lost race it is the earlier record.
    let record = read_capture(transaction, request.user_ref, &permalink.url).await?;
    let created = inserted.rows_affected() == 1;

    Ok(if created {
        CaptureSubmission::Created(record)
    } else {
        CaptureSubmission::Reused(record)
    })
}

impl Database {
    /// Records that resolution of a captured source failed.
    ///
    /// Appends one availability observation bound to the capture and moves
    /// accepted or resolved captures to [`CaptureStatus::Unavailable`]. The
    /// canonical URL, captured time, and note are never touched, and no media
    /// row is created: the fallback preserves the attempt, not content.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError`] when the capture does not exist or a query
    /// fails.
    pub async fn record_capture_unavailable(
        &self,
        capture_id: Uuid,
        observed: AvailabilityObservationKind,
        observed_at: OffsetDateTime,
    ) -> Result<CaptureStatus, CaptureError> {
        let mut transaction = self
            .pool()
            .begin()
            .await
            .map_err(CaptureError::Persistence)?;

        // Existence first, so an unknown id answers UnknownCapture instead of
        // surfacing as a foreign-key refusal behind an inserted observation.
        let current_status: Option<String> = sqlx::query_scalar(
            "select status from instagram_archive.captures where capture_id = $1",
        )
        .bind(capture_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(CaptureError::Persistence)?;
        let Some(current_status) = current_status else {
            return Err(CaptureError::UnknownCapture);
        };

        sqlx::query(
            "insert into instagram_archive.availability_observations \
             (observation_id, media_id, capture_id, availability, observed_at) \
             values ($1, null, $2, $3, $4)",
        )
        .bind(Uuid::now_v7())
        .bind(capture_id)
        .bind(observed.wire_value())
        .bind(observed_at)
        .execute(&mut *transaction)
        .await
        .map_err(CaptureError::Persistence)?;

        if matches!(current_status.as_str(), "accepted" | "resolved") {
            sqlx::query(
                "update instagram_archive.captures set status = 'unavailable' \
                 where capture_id = $1",
            )
            .bind(capture_id)
            .execute(&mut *transaction)
            .await
            .map_err(CaptureError::Persistence)?;
        }

        transaction
            .commit()
            .await
            .map_err(CaptureError::Persistence)?;
        let final_status = if matches!(current_status.as_str(), "accepted" | "resolved") {
            "unavailable".to_owned()
        } else {
            current_status
        };
        CaptureStatus::parse(&final_status).ok_or_else(db_inconsistent)
    }
}

fn db_inconsistent() -> CaptureError {
    CaptureError::Persistence(sqlx::Error::ColumnDecode {
        index: "status".to_owned(),
        source: "'unknown' status wire value".into(),
    })
}

/// Reads one existing capture by identity pair inside the open transaction.
async fn read_capture(
    transaction: &mut sqlx::PgConnection,
    user_ref: Uuid,
    canonical_url: &str,
) -> Result<CaptureRecord, CaptureError> {
    let (capture_id, acquisition_method, saved_authority, client_source, status, note, captured_at): (
        Uuid,
        String,
        String,
        String,
        String,
        Option<String>,
        OffsetDateTime,
    ) = sqlx::query_as(
        "select capture_id, acquisition_method, saved_authority, client_source, status, note, \
         captured_at from instagram_archive.captures \
         where user_ref = $1 and canonical_url = $2",
    )
    .bind(user_ref)
    .bind(canonical_url)
    .fetch_one(transaction)
    .await
    .map_err(|error| match error {
        sqlx::Error::RowNotFound => CaptureError::UnknownCapture,
        other => CaptureError::Persistence(other),
    })?;

    if saved_authority != EXPLICIT_AUTHORITY {
        return Err(db_inconsistent());
    }
    Ok(CaptureRecord {
        capture_id,
        canonical_url: canonical_url.to_owned(),
        acquisition_method,
        saved_authority,
        client_source,
        status: CaptureStatus::parse(&status).ok_or_else(db_inconsistent)?,
        note,
        captured_at,
    })
}
