//! One acknowledged Instagram browser-capture broker delivery.

use async_nats::jetstream;
use ratatoskr_instagram_archive::{CommandCaptureError, Database};

/// Persists and acknowledges one prefiltered Instagram command delivery.
///
/// Persistence failures are negatively acknowledged for redelivery. Invalid
/// deliveries are acknowledged after rejection because Platform must not let a
/// malformed command poison the provider's durable forever.
pub async fn consume_one(database: &Database, message: &jetstream::Message) {
    match database
        .ingest_browser_capture_command("cmd.instagram.capture.requested.v1", &message.payload)
        .await
    {
        Ok(_) => {
            acknowledge(
                message,
                "the Instagram command delivery could not be acknowledged",
            )
            .await;
        }
        Err(CommandCaptureError::Capture(_) | CommandCaptureError::Persistence(_)) => {
            if let Err(error) = message.ack_with(jetstream::AckKind::Nak(None)).await {
                tracing::warn!(%error, "the Instagram command delivery could not be retried");
            }
        }
        Err(error) => {
            tracing::warn!(%error, "an invalid Instagram command was acknowledged without processing");
            acknowledge(
                message,
                "the invalid Instagram command could not be acknowledged",
            )
            .await;
        }
    }
}

async fn acknowledge(message: &jetstream::Message, detail: &'static str) {
    if let Err(error) = message.ack().await {
        tracing::warn!(%error, "{detail}");
    }
}
