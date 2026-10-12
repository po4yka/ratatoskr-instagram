//! The acknowledgement of a deletion fact (XR-021 CONTRACTS.md R5-11 rule 4).

use std::time::Duration;

use uuid::Uuid;

use super::pump::{EventTransport, record_transient_failure};

/// The one event type Knowledge acknowledges: a removal fact is kept and published again until
/// the acknowledgement arrives.
pub const REMOVAL_EVENT_TYPE: &str = "social.source.removed.v1";

/// Counter of removals published again because Knowledge had not acknowledged them.
pub const TOMBSTONE_RESENT_TOTAL: &str = "instagram_tombstone_resent_total";
/// Counter of removals Knowledge refused.
pub const TOMBSTONE_REFUSED_TOTAL: &str = "instagram_tombstone_refused_total";
/// Gauge of removals Knowledge has not acknowledged yet.
pub const UNACKNOWLEDGED_TOMBSTONES: &str = "instagram_unacknowledged_tombstones";
/// Gauge of the age in seconds of the oldest removal Knowledge has not acknowledged.
pub const OLDEST_UNACKNOWLEDGED_TOMBSTONE_AGE_SECONDS: &str =
    "instagram_oldest_unacknowledged_tombstone_age_seconds";

/// How long a published removal waits for Knowledge before it is published again, and the
/// longest the wait between two copies grows to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResendPolicy {
    /// The delay before the first republish; each later delay doubles.
    pub after: Duration,
    /// The longest delay between two republishes.
    pub cap: Duration,
}

impl Default for ResendPolicy {
    /// The documented defaults: 600 seconds before the first republish, at most 21600 between two.
    fn default() -> Self {
        Self::new(Duration::from_mins(10), Duration::from_hours(6))
    }
}

impl ResendPolicy {
    /// A policy from the delay and the cap.
    #[must_use]
    pub const fn new(after: Duration, cap: Duration) -> Self {
        Self { after, cap }
    }
}

/// What Knowledge did with a removal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckOutcome {
    /// The removal was applied, or repeated one that had been.
    Applied,
    /// Knowledge refused it; the data stays there and an operator must look.
    Refused,
}

impl AckOutcome {
    /// The value stored in `ack_outcome`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Refused => "refused",
        }
    }
}

/// The removals Knowledge has not acknowledged yet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnacknowledgedRemovals {
    /// How many there are.
    pub count: i64,
    /// Seconds since the oldest was created, or zero when there is none.
    pub oldest_age_seconds: f64,
}

/// Arms a published removal for its first republish: `next_resend_at` becomes the publish time
/// plus `policy.after`. Rows of every other event type get no schedule.
///
/// # Errors
///
/// [`sqlx::Error`] when the update fails.
pub(super) async fn mark_published(
    pool: &sqlx::PgPool,
    event_id: Uuid,
    policy: ResendPolicy,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update instagram_archive.outbox_events \
         set published_at = now(), \
             next_resend_at = case when event_type = $2 \
                                   then now() + make_interval(secs => $3) end \
         where event_id = $1 and published_at is null",
    )
    .bind(event_id)
    .bind(REMOVAL_EVENT_TYPE)
    .bind(policy.after.as_secs_f64())
    .execute(pool)
    .await?;
    Ok(())
}

/// Records one acknowledged republish of an unacknowledged removal: `resend_count` grows by one
/// and `next_resend_at` becomes now plus the least of `policy.after * 2^resend_count` and
/// `policy.cap`.
///
/// The exponent is clamped at 30, far beyond the point where the cap wins, so a removal that
/// waits for years cannot overflow the arithmetic.
async fn record_resend(
    pool: &sqlx::PgPool,
    event_id: Uuid,
    policy: ResendPolicy,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update instagram_archive.outbox_events \
         set resend_count = resend_count + 1, attempt_count = 0, last_error = null, \
             next_resend_at = now() + make_interval(secs => \
                 least($3::float8 * power(2::float8, least(resend_count, 30)::float8), \
                       $4::float8)) \
         where event_id = $1 and event_type = $2 and published_at is not null \
           and acknowledged_at is null",
    )
    .bind(event_id)
    .bind(REMOVAL_EVENT_TYPE)
    .bind(policy.after.as_secs_f64())
    .bind(policy.cap.as_secs_f64())
    .execute(pool)
    .await?;
    Ok(())
}

/// What one resend phase did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct ResendReport {
    /// Removals published again.
    pub(super) resent: u32,
    /// Republishes the transport refused; each backed off.
    pub(super) failed: u32,
}

/// Publishes the stored bytes of every due, unacknowledged removal again, oldest due first, and
/// schedules the next copy. Stops at the first refusal, as the fresh rows do; the refusal is
/// backed off on `next_attempt_at` and leaves the resend schedule and `published_at` alone.
pub(super) async fn resend_due<T: EventTransport>(
    pool: &sqlx::PgPool,
    transport: &T,
    batch: u32,
    policy: ResendPolicy,
) -> Result<ResendReport, sqlx::Error> {
    let due: Vec<(Uuid, serde_json::Value)> = sqlx::query_as(
        "select event_id, payload from instagram_archive.outbox_events \
         where event_type = $1 and published_at is not null and acknowledged_at is null \
           and next_resend_at <= now() \
           and (next_attempt_at is null or next_attempt_at <= now()) \
         order by next_resend_at, event_id limit $2",
    )
    .bind(REMOVAL_EVENT_TYPE)
    .bind(i32::try_from(batch).unwrap_or(i32::MAX))
    .fetch_all(pool)
    .await?;
    let mut report = ResendReport::default();
    for (event_id, payload) in due {
        let body = serde_json::to_string(&payload)
            .map_err(|error| sqlx::Error::Encode(Box::new(error)))?;
        match transport.deliver(event_id, &body).await {
            Ok(()) => {
                record_resend(pool, event_id, policy).await?;
                metrics::counter!(TOMBSTONE_RESENT_TOTAL).increment(1);
                report.resent += 1;
            }
            Err(error) => {
                tracing::warn!(event = %event_id, reason = %error, "a removal could not be published again");
                // Backs off on `next_attempt_at`; the row stays published.
                record_transient_failure(pool, event_id, &error).await?;
                report.failed += 1;
                break;
            }
        }
    }
    Ok(report)
}

/// Records Knowledge's answer on the removal `event_id` of `tenant` (the wire form of the
/// acknowledgement's owner, `user:<uuid>`), and reports whether a row changed.
///
/// An unknown id, another tenant's row, a row that is not a removal and an already acknowledged
/// row change nothing.
///
/// # Errors
///
/// [`sqlx::Error`] when the update fails.
pub async fn acknowledge(
    pool: &sqlx::PgPool,
    event_id: Uuid,
    tenant: &str,
    outcome: AckOutcome,
) -> Result<bool, sqlx::Error> {
    let changed = sqlx::query(
        "update instagram_archive.outbox_events \
         set acknowledged_at = now(), ack_outcome = $4 \
         where event_id = $1 and event_type = $2 and payload ->> 'tenant_id' = $3 \
           and acknowledged_at is null",
    )
    .bind(event_id)
    .bind(REMOVAL_EVENT_TYPE)
    .bind(tenant)
    .bind(outcome.as_str())
    .execute(pool)
    .await?;
    Ok(changed.rows_affected() == 1)
}

/// Counts the unacknowledged removals and ages the oldest.
///
/// # Errors
///
/// [`sqlx::Error`] when the read fails.
pub async fn unacknowledged_removals(
    pool: &sqlx::PgPool,
) -> Result<UnacknowledgedRemovals, sqlx::Error> {
    let (count, oldest_age_seconds): (i64, f64) = sqlx::query_as(
        "select count(*), coalesce(extract(epoch from now() - min(occurred_at)), 0)::float8 \
         from instagram_archive.outbox_events \
         where event_type = $1 and acknowledged_at is null",
    )
    .bind(REMOVAL_EVENT_TYPE)
    .fetch_one(pool)
    .await?;
    Ok(UnacknowledgedRemovals {
        count,
        oldest_age_seconds,
    })
}

/// Sets the two gauges that tell an operator Knowledge has not answered. They never feed
/// readiness: a Knowledge outage must not make the service unready.
pub(super) async fn refresh_gauges(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
    let waiting = unacknowledged_removals(pool).await?;
    let count = u32::try_from(waiting.count).map_or(f64::from(u32::MAX), f64::from);
    metrics::gauge!(UNACKNOWLEDGED_TOMBSTONES).set(count);
    metrics::gauge!(OLDEST_UNACKNOWLEDGED_TOMBSTONE_AGE_SECONDS).set(waiting.oldest_age_seconds);
    Ok(())
}
