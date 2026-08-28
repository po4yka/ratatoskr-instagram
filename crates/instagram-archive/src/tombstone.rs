//! Local capture tombstones and their downstream removal facts.

use ratatoskr_social_contracts::RemovalReason;
use uuid::Uuid;

use crate::Database;
use crate::publishing::{PublishError, append_removal_fact, instant_from_time, source_identity};

/// Result of applying a local capture tombstone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TombstoneOutcome {
    /// A new tombstone and one removal fact were committed.
    Tombstoned,
    /// The capture was already tombstoned; no duplicate removal fact was made.
    Duplicate,
}

/// Why a local tombstone could not be recorded.
#[derive(Debug, thiserror::Error)]
pub enum TombstoneError {
    /// The capture does not exist.
    #[error("no capture exists under this id")]
    UnknownCapture,
    /// The removal fact could not be built or appended.
    #[error(transparent)]
    Publish(#[from] PublishError),
    /// An archive-owned query failed.
    #[error("the capture tombstone could not be stored")]
    Persistence(#[from] sqlx::Error),
}

impl Database {
    /// Tombstones a local capture and appends its downstream removal fact.
    ///
    /// This changes only Ratatoskr's preservation state. It does not assert
    /// anything about the provider's Saved list or upstream availability.
    ///
    /// # Errors
    ///
    /// Returns [`TombstoneError`] when the capture is unknown or the atomic
    /// tombstone/outbox transaction cannot complete.
    pub async fn tombstone_capture(
        &self,
        capture_id: Uuid,
        reason: RemovalReason,
        removed_at: time::OffsetDateTime,
    ) -> Result<TombstoneOutcome, TombstoneError> {
        let mut transaction = self.pool().begin().await?;
        let capture: Option<(Uuid, String)> = sqlx::query_as(
            "select user_ref, canonical_url from instagram_archive.captures \
             where capture_id = $1 for update",
        )
        .bind(capture_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((user_ref, canonical_url)) = capture else {
            return Err(TombstoneError::UnknownCapture);
        };
        let social_source_id = source_identity(user_ref, &canonical_url);
        let present: Option<(Uuid,)> = sqlx::query_as(
            "select capture_id from instagram_archive.local_source_removals \
             where user_ref = $1 and social_source_id = $2",
        )
        .bind(user_ref)
        .bind(social_source_id)
        .fetch_optional(&mut *transaction)
        .await?;
        if present.is_some() {
            transaction.commit().await?;
            return Ok(TombstoneOutcome::Duplicate);
        }

        sqlx::query(
            "insert into instagram_archive.deletion_operations \
             (operation_id, user_ref, target_kind, target_id, reason, state, \
              requested_at, updated_at, finished_at) \
             values ($1, $2, 'capture', $3, $4, 'complete', $5, $5, $5) \
             on conflict (operation_id) do nothing",
        )
        .bind(capture_id)
        .bind(user_ref)
        .bind(capture_id)
        .bind(removal_reason_wire(reason))
        .bind(removed_at)
        .execute(&mut *transaction)
        .await?;
        let removed_wire = instant_from_time(removed_at, capture_id)?;
        append_removal_fact(&mut transaction, capture_id, reason, removed_wire).await?;
        sqlx::query(
            "insert into instagram_archive.local_source_removals \
             (user_ref, social_source_id, capture_id, operation_id, reason, removed_at) \
             values ($1, $2, $3, $3, $4, $5)",
        )
        .bind(user_ref)
        .bind(social_source_id)
        .bind(capture_id)
        .bind(removal_reason_wire(reason))
        .bind(removed_at)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "update instagram_archive.captures set status = 'tombstoned' where capture_id = $1",
        )
        .bind(capture_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(TombstoneOutcome::Tombstoned)
    }
}

fn removal_reason_wire(reason: RemovalReason) -> &'static str {
    match reason {
        RemovalReason::UserRequested => "user_requested",
        RemovalReason::RetentionPolicy => "retention_policy",
        _ => "unknown",
    }
}
