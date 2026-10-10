//! Platform operation reports for explicit captures (XR-021 CONTRACTS.md S10 CD1, CD2, CD7).
//!
//! A report is the complete canonical `platform.operation.reported.v1` envelope stored in the
//! outbox in the caller's open transaction, so it commits or aborts with the state change it
//! describes. The envelope's `event_id` is the outbox row id, which the relay uses as the
//! `Nats-Msg-Id` header.

use ratatoskr_event_envelope::EventEnvelope;
use ratatoskr_identifiers::{CommandId, EntityRef, EventId, OperationId, UserId};
use ratatoskr_operation_contracts::OperationReported;
use ratatoskr_social_contracts::report_envelope;
use sqlx::PgConnection;
use uuid::Uuid;

use super::{PRODUCER_NAME, PublishError, text_violation};

/// The wire type of every report this module stores.
pub(crate) const OPERATION_REPORTED_EVENT_TYPE: &str = "platform.operation.reported.v1";

/// Appends one operation report to the outbox inside the caller's transaction.
///
/// `capture_id` names the capture the operation belongs to; `None` is the one case where no
/// capture exists, a command whose permalink this service cannot map, and the report then
/// aggregates on the operation itself. The outbox correlation is the operation id and its
/// causation is the command id, matching the envelope (`operation:<id>` / `command:<id>`).
///
/// # Errors
///
/// [`PublishError`] when the report breaks its contract or the insert fails.
pub(crate) async fn append_operation_report(
    transaction: &mut PgConnection,
    capture_id: Option<Uuid>,
    owner: Uuid,
    command_id: Uuid,
    report: &OperationReported,
) -> Result<Uuid, PublishError> {
    let operation_id = report.operation_id.0;
    let (aggregate_type, aggregate_id, aggregate) = match capture_id {
        Some(capture) => (
            "capture",
            capture,
            EntityRef::parse(&format!("capture:{capture}"))
                .map_err(|error| text_violation(capture, error))?,
        ),
        None => (
            "operation",
            operation_id,
            report.operation_id.as_entity_ref(),
        ),
    };
    let event_id = Uuid::now_v7();
    let envelope = report_envelope(
        PRODUCER_NAME,
        UserId(owner),
        OperationId(operation_id),
        aggregate,
        CommandId(command_id),
        EventId(event_id),
        report,
    )
    .map_err(|error| text_violation(aggregate_id, error))?;
    let payload = canonical_value(&envelope, aggregate_id)?;
    sqlx::query(
        "insert into instagram_archive.outbox_events \
         (event_id, event_type, aggregate_type, aggregate_id, payload, correlation_id, \
          causation_id, occurred_at) \
         values ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(event_id)
    .bind(OPERATION_REPORTED_EVENT_TYPE)
    .bind(aggregate_type)
    .bind(aggregate_id)
    .bind(payload)
    .bind(operation_id)
    .bind(command_id)
    .bind(time::OffsetDateTime::now_utc())
    .execute(&mut *transaction)
    .await?;
    Ok(event_id)
}

/// Renders the envelope through its canonical form so the stored bytes are what a consumer
/// would re-encode.
fn canonical_value(
    envelope: &EventEnvelope,
    aggregate_id: Uuid,
) -> Result<serde_json::Value, PublishError> {
    let canonical = envelope
        .to_canonical_json()
        .map_err(|error| text_violation(aggregate_id, error))?;
    Ok(serde_json::from_str(&canonical)?)
}
