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
/// Gauge of outbox rows still waiting for their first successful delivery.
pub const OUTBOX_UNPUBLISHED_DEPTH: &str = "instagram_outbox_unpublished_depth";

/// The longest `last_error` kept on a failed outbox row, in characters.
const LAST_ERROR_CHARS: usize = 200;

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
/// A row whose delivery failed waits out its `next_attempt_at` backoff and is not selected
/// again until it is due, so a failing head row cannot starve the rows behind it.
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
         where published_at is null \
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
                         next_attempt_at = now() + interval '60 seconds', \
                         last_error = $2 \
                     where event_id = $1",
                )
                .bind(event_id)
                .bind(error.0.chars().take(LAST_ERROR_CHARS).collect::<String>())
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
