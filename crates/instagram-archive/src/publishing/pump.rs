//! The at-least-once publisher pass over the outbox and the seam it delivers through.

use uuid::Uuid;

use super::PRODUCER_NAME;

/// Metric names emitted by the publisher pass; rendered on `/metrics`.
/// Counter of facts delivered and marked published.
pub const OUTBOX_DELIVERED_TOTAL: &str = "instagram_outbox_delivered_total";
/// Counter of delivery attempts that failed and stayed unpublished.
pub const OUTBOX_FAILED_TOTAL: &str = "instagram_outbox_failed_total";
/// Counter of deliveries that were redeliveries of a previously failed fact.
pub const OUTBOX_REDELIVERED_TOTAL: &str = "instagram_outbox_redelivered_total";
/// Counter of facts marked undeliverable, labelled by the closed `class` token.
pub const OUTBOX_UNDELIVERABLE_TOTAL: &str = "instagram_outbox_undeliverable_total";
/// Counter of passes that found the publisher failing, labelled by the closed `class` token.
pub const OUTBOX_PUBLISHER_FAILING_TOTAL: &str = "instagram_outbox_publisher_failing_total";
/// Gauge of outbox rows still waiting for their first successful delivery.
pub const OUTBOX_UNPUBLISHED_DEPTH: &str = "instagram_outbox_unpublished_depth";

/// The longest `last_error` kept on a failed outbox row, in characters.
const LAST_ERROR_CHARS: usize = 200;

/// Why a delivery can never succeed, however often it is retried (XR-021 CONTRACTS.md R2-04).
///
/// The rendered name is stored as the row's `last_error` and used as the `class` metric label,
/// so it is a closed token and never free text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum UndeliverableClass {
    /// The serialized envelope is larger than the connected server accepts.
    #[error("payload_too_large")]
    PayloadTooLarge,
}

impl UndeliverableClass {
    /// The closed token stored in `last_error` and used as the metric label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PayloadTooLarge => "payload_too_large",
        }
    }
}

/// Why a delivery attempt could not complete. The message is safe for logs:
/// it describes transport behaviour, never payload content.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// The carrier could not take the fact now (unreachable broker, publish or acknowledgement
    /// failure, acknowledgement timeout). The row stays, backs off and is retried.
    #[error("event delivery failed: {0}")]
    Transient(String),
    /// No retry can deliver this fact. The row is marked undeliverable at once.
    #[error("event is undeliverable: {0}")]
    Undeliverable(UndeliverableClass),
}

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

/// The attempt count from which a failing publish is reported as a failing publisher.
const FAILING_ATTEMPTS: i32 = 3;
/// The age in seconds from which an unpublished row is reported as a failing publisher.
const FAILING_AGE_SECONDS: i32 = 300;
/// The `class` label of [`OUTBOX_PUBLISHER_FAILING_TOTAL`]: the relay is running but its
/// oldest publishable row keeps failing, which on a deployed broker usually means a Publish
/// Violation in the NATS server log.
const PUBLISH_FAILING_CLASS: &str = "publish_failing";

/// What one pass says about the health of the publisher (XR-021 CONTRACTS.md R2-03).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PassVerdict {
    /// Nothing is stuck: every publishable row was delivered, or the oldest one has neither
    /// failed three times nor aged past 300 seconds.
    #[default]
    Healthy,
    /// At least one publish failed in this pass and the oldest publishable row has failed three
    /// times or is older than 300 seconds.
    Failing,
    /// No publish failed, but the oldest publishable row is stuck and waiting out its backoff:
    /// the pass learned nothing new, so the previous verdict stands. Without this a failing
    /// relay would read as healthy on every pass between two attempts of one row.
    Waiting,
}

/// One publisher pass over the unpublished outbox rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PassSummary {
    /// Facts delivered and marked published.
    pub delivered: u32,
    /// Facts whose delivery failed; they stay unpublished for redelivery.
    pub failed: u32,
    /// Facts that can never be delivered; they were marked undeliverable and left the queue.
    pub undeliverable: u32,
    /// Publishable facts still waiting after this pass (undeliverable rows excluded).
    pub remaining: u64,
    /// What this pass says about the publisher's health.
    pub verdict: PassVerdict,
}

/// Runs exactly one claiming pass: oldest first, bounded by `batch`.
///
/// A row whose delivery failed waits out its `next_attempt_at` backoff and is not selected
/// again until it is due, so a failing head row cannot starve the rows behind it. A row the
/// transport classes undeliverable is marked at once, counted, and never selected again; the
/// pass continues with the next rows.
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
         where published_at is null and undeliverable_at is null \
           and (next_attempt_at is null or next_attempt_at <= now()) \
         order by occurred_at, event_id limit $1",
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
                mark_published(pool, event_id).await?;
                summary.delivered += 1;
            }
            Err(TransportError::Undeliverable(class)) => {
                tracing::warn!(event = %event_id, class = class.as_str(), "outbox row is undeliverable");
                mark_undeliverable(pool, event_id, class).await?;
                summary.undeliverable += 1;
            }
            Err(error @ TransportError::Transient(_)) => {
                tracing::warn!(event = %event_id, reason = %error, "outbox delivery failed");
                record_transient_failure(pool, event_id, &error).await?;
                summary.failed += 1;
            }
        }
    }

    let (remaining,): (i64,) = sqlx::query_as(
        "select count(*) from instagram_archive.outbox_events \
         where published_at is null and undeliverable_at is null",
    )
    .fetch_one(pool)
    .await?;
    summary.remaining = u64::try_from(remaining).unwrap_or(u64::MAX);
    let depth = f64::from(u32::try_from(summary.remaining).unwrap_or(u32::MAX));
    metrics::gauge!(OUTBOX_UNPUBLISHED_DEPTH, "producer" => PRODUCER_NAME).set(depth);

    summary.verdict = verdict_for(pool, &summary).await?;
    if summary.verdict == PassVerdict::Failing {
        metrics::counter!(OUTBOX_PUBLISHER_FAILING_TOTAL, "class" => PUBLISH_FAILING_CLASS)
            .increment(1);
    }

    Ok(summary)
}

async fn mark_published(pool: &sqlx::PgPool, event_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update instagram_archive.outbox_events \
         set published_at = now() \
         where event_id = $1 and published_at is null",
    )
    .bind(event_id)
    .execute(pool)
    .await?;
    metrics::counter!(OUTBOX_DELIVERED_TOTAL).increment(1);
    Ok(())
}

async fn mark_undeliverable(
    pool: &sqlx::PgPool,
    event_id: Uuid,
    class: UndeliverableClass,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update instagram_archive.outbox_events \
         set attempt_count = attempt_count + 1, \
             undeliverable_at = now(), \
             last_error = $2 \
         where event_id = $1",
    )
    .bind(event_id)
    .bind(class.as_str())
    .execute(pool)
    .await?;
    metrics::counter!(OUTBOX_UNDELIVERABLE_TOTAL, "class" => class.as_str()).increment(1);
    Ok(())
}

async fn record_transient_failure(
    pool: &sqlx::PgPool,
    event_id: Uuid,
    error: &TransportError,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update instagram_archive.outbox_events \
         set attempt_count = attempt_count + 1, \
             next_attempt_at = now() + interval '60 seconds', \
             last_error = $2 \
         where event_id = $1",
    )
    .bind(event_id)
    .bind(
        error
            .to_string()
            .chars()
            .take(LAST_ERROR_CHARS)
            .collect::<String>(),
    )
    .execute(pool)
    .await?;
    metrics::counter!(OUTBOX_FAILED_TOTAL).increment(1);
    Ok(())
}

/// Whether the oldest publishable row has failed [`FAILING_ATTEMPTS`] times or aged past
/// [`FAILING_AGE_SECONDS`].
async fn oldest_row_is_stuck(pool: &sqlx::PgPool) -> Result<bool, sqlx::Error> {
    let stuck: Option<bool> = sqlx::query_scalar(
        "select attempt_count >= $1 or occurred_at < now() - make_interval(secs => $2) \
         from instagram_archive.outbox_events \
         where published_at is null and undeliverable_at is null \
         order by occurred_at, event_id limit 1",
    )
    .bind(FAILING_ATTEMPTS)
    .bind(f64::from(FAILING_AGE_SECONDS))
    .fetch_optional(pool)
    .await?;
    Ok(stuck.unwrap_or(false))
}

async fn verdict_for(
    pool: &sqlx::PgPool,
    summary: &PassSummary,
) -> Result<PassVerdict, sqlx::Error> {
    if summary.remaining == 0 {
        return Ok(PassVerdict::Healthy);
    }
    let stuck = oldest_row_is_stuck(pool).await?;
    Ok(match (summary.failed > 0, stuck) {
        (true, true) => PassVerdict::Failing,
        (false, true) => PassVerdict::Waiting,
        (_, false) => PassVerdict::Healthy,
    })
}
