//! The capture resolution worker: bounded retry and exactly-once terminal reports
//! (XR-021 CONTRACTS.md S10 CD2, CD7).
//!
//! [`CaptureResolver::run_due_once`] is one pass. It claims captures that carry an unreported
//! Platform operation, fetches the public surface for the ones still `accepted`, concludes each
//! capture, and reports every pending operation of a concluded capture exactly once.
//!
//! - A capture that is `accepted` and due is leased for [`LEASE`] before the network call, so a
//!   crashed worker's capture becomes due again and a second worker does not take it meanwhile.
//! - A transient outcome before the last attempt only moves `next_resolution_at` forward by
//!   [`retry_delay`]; nothing is recorded about the source. The last attempt concludes the
//!   capture as temporarily unavailable.
//! - A capture that is already concluded (resolved, unavailable, failed, tombstoned) is never
//!   fetched again; its pending operations are only reported. This is also the crash recovery
//!   path, because the report is derived from stored state and never from the in-memory outcome.
//! - The terminal report is guarded by `UPDATE capture_operations SET reported_at ... WHERE
//!   reported_at IS NULL RETURNING`: the outbox row is inserted only for a row that came back.

use ratatoskr_identifiers::{OperationId, SocialSourceId};
use ratatoskr_operation_contracts::OperationReported;
use ratatoskr_social_contracts::{
    SocialContractError, SourceUnavailability, preserved_report, unavailable_report,
};
use sqlx::PgConnection;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::capability::AvailabilityObservationKind;
use crate::database::Database;
use crate::permalink;
use crate::publishing::{PublishError, append_operation_report, source_identity};
use crate::resolution::{PublicSurface, ResolutionError, SurfaceOutcome};

/// How long a claimed capture stays invisible to other passes while its fetch is in flight.
pub const LEASE: Duration = Duration::seconds(120);

/// The longest delay between two attempts.
const MAX_RETRY_DELAY_SECONDS: i64 = 30 * 60;

/// The knobs of one pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolutionPolicy {
    /// Attempts before a transient failure becomes terminal.
    pub max_attempts: u32,
    /// Captures claimed by one pass.
    pub batch_size: u32,
}

/// What one pass did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResolutionSummary {
    /// Captures claimed by the pass.
    pub claimed: u32,
    /// Public-surface fetches made.
    pub fetched: u32,
    /// Captures rescheduled after a transient failure.
    pub retried: u32,
    /// Terminal operation reports appended.
    pub reported: u32,
    /// Captures whose processing failed and will be retried after their lease.
    pub failed: u32,
}

/// Why a capture could not be processed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CaptureResolutionError {
    /// A query failed.
    #[error("a capture resolution query failed")]
    Persistence(#[from] sqlx::Error),
    /// Concluding one capture failed.
    #[error("a capture could not be concluded")]
    Resolution(#[from] ResolutionError),
    /// A terminal report could not be built or stored.
    #[error("a terminal report could not be recorded")]
    Report(#[from] PublishError),
}

/// The delay before the next attempt after `attempt` transient failures: 30 s, 2 min, 8 min, then
/// 30 min (`min(30 s * 4^(attempt - 1), 30 min)`).
#[must_use]
pub fn retry_delay(attempt: u32) -> Duration {
    let exponent = attempt.saturating_sub(1).min(6);
    let seconds = 30_i64
        .saturating_mul(4_i64.pow(exponent))
        .min(MAX_RETRY_DELAY_SECONDS);
    Duration::seconds(seconds)
}

/// Resolves captures that carry unreported Platform operations.
#[derive(Debug, Clone)]
pub struct CaptureResolver {
    database: Database,
}

/// A capture leased for a fetch: id, canonical URL, attempts so far.
type Leased = (Uuid, String, i32);

/// What processing one leased capture did.
#[derive(Debug, Default)]
struct Step {
    fetched: bool,
    retried: bool,
    reported: u32,
}

impl CaptureResolver {
    /// A resolver over the archive database.
    #[must_use]
    pub fn new(database: Database) -> Self {
        Self { database }
    }

    /// Runs one bounded pass at the injected instant `now`.
    ///
    /// A failure on one capture is counted and does not stop the others; the capture is retried
    /// when its lease expires.
    ///
    /// # Errors
    ///
    /// [`CaptureResolutionError`] when the claim itself fails.
    pub async fn run_due_once(
        &self,
        surface: &impl PublicSurface,
        now: OffsetDateTime,
        policy: ResolutionPolicy,
    ) -> Result<ResolutionSummary, CaptureResolutionError> {
        let mut summary = ResolutionSummary::default();
        let leased = self.claim(now, policy, &mut summary).await?;
        for (capture_id, canonical_url, attempts_so_far) in leased {
            match self
                .resolve_one(
                    surface,
                    capture_id,
                    &canonical_url,
                    attempts_so_far,
                    now,
                    policy,
                )
                .await
            {
                Ok(step) => {
                    summary.fetched += u32::from(step.fetched);
                    summary.retried += u32::from(step.retried);
                    summary.reported += step.reported;
                }
                Err(error) => {
                    tracing::warn!(
                        error_class = "capture_resolution_failed",
                        %capture_id,
                        %error,
                        "a capture could not be concluded; it is retried after its lease"
                    );
                    summary.failed += 1;
                }
            }
        }
        Ok(summary)
    }

    /// Claims due captures. Concluded captures are reported inside the claim transaction;
    /// `accepted` ones are leased and returned for a fetch.
    async fn claim(
        &self,
        now: OffsetDateTime,
        policy: ResolutionPolicy,
        summary: &mut ResolutionSummary,
    ) -> Result<Vec<Leased>, CaptureResolutionError> {
        let mut transaction = self.database.pool().begin().await?;
        let claimed: Vec<(Uuid, String, String, i32)> = sqlx::query_as(
            "select c.capture_id, c.canonical_url, c.status, c.resolution_attempts \
             from instagram_archive.captures c \
             where exists (select 1 from instagram_archive.capture_operations o \
                           where o.capture_id = c.capture_id and o.reported_at is null) \
               and ((c.status = 'accepted' and c.next_resolution_at <= $1) \
                    or c.status in ('resolved', 'unavailable', 'failed', 'tombstoned')) \
             order by c.next_resolution_at nulls first, c.capture_id \
             limit $2 \
             for update of c skip locked",
        )
        .bind(now)
        .bind(i64::from(policy.batch_size))
        .fetch_all(&mut *transaction)
        .await?;
        summary.claimed = u32::try_from(claimed.len()).unwrap_or(u32::MAX);
        let mut leased = Vec::new();
        for (capture_id, canonical_url, status, attempts) in claimed {
            if status == "accepted" {
                sqlx::query(
                    "update instagram_archive.captures set next_resolution_at = $2 \
                     where capture_id = $1",
                )
                .bind(capture_id)
                .bind(now + LEASE)
                .execute(&mut *transaction)
                .await?;
                leased.push((capture_id, canonical_url, attempts));
            } else {
                summary.reported += report_pending(&mut transaction, capture_id, now).await?;
            }
        }
        transaction.commit().await?;
        Ok(leased)
    }

    /// Fetches one leased capture, concludes or reschedules it, and reports it when concluded.
    async fn resolve_one(
        &self,
        surface: &impl PublicSurface,
        capture_id: Uuid,
        canonical_url: &str,
        attempts_so_far: i32,
        now: OffsetDateTime,
        policy: ResolutionPolicy,
    ) -> Result<Step, CaptureResolutionError> {
        let Ok(permalink) = permalink::canonicalize(canonical_url) else {
            // The stored permalink no longer maps: this service's own failure, permanent.
            self.database
                .record_failed_resolution(
                    capture_id,
                    AvailabilityObservationKind::ResolutionFailed,
                    now,
                )
                .await?;
            return Ok(Step {
                reported: self.report_concluded(capture_id, now).await?,
                ..Step::default()
            });
        };
        let outcome = surface.fetch(&permalink).await;
        let attempt = u32::try_from(attempts_so_far)
            .unwrap_or(0)
            .saturating_add(1);
        let outcome = if is_transient(&outcome) {
            if attempt < policy.max_attempts {
                self.reschedule(capture_id, attempt, now).await?;
                return Ok(Step {
                    fetched: true,
                    retried: true,
                    reported: 0,
                });
            }
            // The retry budget is spent: the provider was unreachable throughout, which is what
            // `temporarily_unavailable` states, whichever transient class the last attempt was.
            SurfaceOutcome::TemporarilyUnavailable
        } else {
            outcome
        };
        self.database
            .apply_surface_outcome(capture_id, &permalink, outcome, now)
            .await?;
        Ok(Step {
            fetched: true,
            retried: false,
            reported: self.report_concluded(capture_id, now).await?,
        })
    }

    async fn reschedule(
        &self,
        capture_id: Uuid,
        attempt: u32,
        now: OffsetDateTime,
    ) -> Result<(), CaptureResolutionError> {
        sqlx::query(
            "update instagram_archive.captures \
             set resolution_attempts = $2, next_resolution_at = $3 where capture_id = $1",
        )
        .bind(capture_id)
        .bind(i32::try_from(attempt).unwrap_or(i32::MAX))
        .bind(now + retry_delay(attempt))
        .execute(self.database.pool())
        .await?;
        Ok(())
    }

    async fn report_concluded(
        &self,
        capture_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<u32, CaptureResolutionError> {
        let mut transaction = self.database.pool().begin().await?;
        let reported = report_pending(&mut transaction, capture_id, now).await?;
        transaction.commit().await?;
        Ok(reported)
    }
}

/// Whether a fetch result may succeed on a later attempt.
const fn is_transient(outcome: &SurfaceOutcome) -> bool {
    matches!(
        outcome,
        SurfaceOutcome::TemporarilyUnavailable | SurfaceOutcome::TransportFailure
    )
}

/// How a concluded capture is reported.
enum Conclusion {
    Preserved(Uuid),
    Unavailable(SourceUnavailability),
}

/// Reports every unreported operation of a concluded capture, once each.
///
/// Returns 0 when the capture is still `accepted`. The conclusion is derived from stored state
/// only, so a crash after the capture concluded and before its report is repaired by the next
/// pass without a second fetch.
async fn report_pending(
    transaction: &mut PgConnection,
    capture_id: Uuid,
    now: OffsetDateTime,
) -> Result<u32, CaptureResolutionError> {
    let (user_ref, canonical_url, status): (Uuid, String, String) = sqlx::query_as(
        "select user_ref, canonical_url, status from instagram_archive.captures \
         where capture_id = $1 for update",
    )
    .bind(capture_id)
    .fetch_one(&mut *transaction)
    .await?;
    let conclusion = match status.as_str() {
        "resolved" => Conclusion::Preserved(source_identity(user_ref, &canonical_url)),
        "unavailable" => {
            Conclusion::Unavailable(latest_unavailability(transaction, capture_id).await?)
        }
        "failed" | "tombstoned" => Conclusion::Unavailable(SourceUnavailability::Inaccessible),
        _ => return Ok(0),
    };
    // The guard: only a row this statement flips from unreported to reported gets a report.
    let pending: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "update instagram_archive.capture_operations set reported_at = $2 \
         where capture_id = $1 and reported_at is null returning operation_id, command_id",
    )
    .bind(capture_id)
    .bind(now)
    .fetch_all(&mut *transaction)
    .await?;
    let mut reported = 0_u32;
    for (operation_id, command_id) in pending {
        let report = terminal_report(&conclusion, operation_id)
            .map_err(|error| PublishError::ContractViolation(capture_id, error.to_string()))?;
        append_operation_report(transaction, Some(capture_id), user_ref, command_id, &report)
            .await?;
        reported += 1;
    }
    Ok(reported)
}

fn terminal_report(
    conclusion: &Conclusion,
    operation_id: Uuid,
) -> Result<OperationReported, SocialContractError> {
    match conclusion {
        Conclusion::Preserved(source) => {
            preserved_report(OperationId(operation_id), SocialSourceId(*source))
        }
        Conclusion::Unavailable(reason) => unavailable_report(OperationId(operation_id), *reason),
    }
}

/// Maps the newest availability observation of a capture onto the reported reason: deleted is
/// `Deleted`, temporarily unavailable is `Transient`, and everything else (private, unsupported,
/// a failed resolution, or no observation at all) is `Inaccessible`.
async fn latest_unavailability(
    transaction: &mut PgConnection,
    capture_id: Uuid,
) -> Result<SourceUnavailability, sqlx::Error> {
    let latest: Option<String> = sqlx::query_scalar(
        "select availability from instagram_archive.availability_observations \
         where capture_id = $1 order by observed_at desc, observation_id desc limit 1",
    )
    .bind(capture_id)
    .fetch_optional(&mut *transaction)
    .await?;
    Ok(match latest.as_deref() {
        Some(kind) if kind == AvailabilityObservationKind::Deleted.wire_value() => {
            SourceUnavailability::Deleted
        }
        Some(kind) if kind == AvailabilityObservationKind::TemporarilyUnavailable.wire_value() => {
            SourceUnavailability::Transient
        }
        _ => SourceUnavailability::Inaccessible,
    })
}
