//! One acknowledged Instagram browser-capture broker delivery.

use std::time::Duration;

use async_nats::jetstream;
use ratatoskr_instagram_archive::{CommandCaptureError, Database};

/// How long the broker waits before redelivering a command that failed transiently.
const RETRY_DELAY: Duration = Duration::from_secs(2);

/// Persists and acknowledges one prefiltered Instagram command delivery.
///
/// The outcome is acknowledged only after it is durable. A transient failure is negatively
/// acknowledged with a short delay for redelivery. A permanently invalid command is terminated
/// (`Term`) so a malformed command can never poison the provider's durable, and only the error
/// class is logged, never the command content (XR-021 CONTRACTS.md S02 rule 7).
pub async fn consume_one(database: &Database, message: &jetstream::Message) {
    match database
        .ingest_browser_capture_command("cmd.instagram.capture.requested.v1", &message.payload)
        .await
    {
        Ok(_) => {
            if let Err(error) = message.ack().await {
                tracing::warn!(%error, "the Instagram command delivery could not be acknowledged");
            }
        }
        Err(
            CommandCaptureError::Capture(_)
            | CommandCaptureError::Persistence(_)
            | CommandCaptureError::Report(_),
        ) => {
            if let Err(error) = message
                .ack_with(jetstream::AckKind::Nak(Some(RETRY_DELAY)))
                .await
            {
                tracing::warn!(%error, "the Instagram command delivery could not be retried");
            }
        }
        Err(error) => {
            tracing::warn!(%error, "an invalid Instagram command was terminated without processing");
            if let Err(error) = message.ack_with(jetstream::AckKind::Term).await {
                tracing::warn!(%error, "the invalid Instagram command could not be terminated");
            }
        }
    }
}
