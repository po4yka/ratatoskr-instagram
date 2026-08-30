//! Acknowledged JetStream delivery for the Instagram transactional outbox.

use std::time::Duration;

use async_nats::jetstream::message::PublishMessage;
use ratatoskr_instagram_archive::publishing::{
    EventTransport, TransportError, subject_for_event_type,
};
use uuid::Uuid;

/// Publishes validated social-source envelopes and waits for `JetStream` storage acknowledgement.
#[derive(Debug, Clone)]
pub struct JetStreamTransport {
    context: async_nats::jetstream::Context,
    acknowledgement_timeout: Duration,
}

impl JetStreamTransport {
    /// Builds a transport over an already-authenticated shared client.
    #[must_use]
    pub fn new(client: async_nats::Client, acknowledgement_timeout: Duration) -> Self {
        Self {
            context: async_nats::jetstream::new(client),
            acknowledgement_timeout,
        }
    }
}

impl EventTransport for JetStreamTransport {
    async fn deliver(
        &self,
        event_id: Uuid,
        event_type: &str,
        envelope_json: &str,
    ) -> Result<(), TransportError> {
        let subject = subject_for_event_type(event_type).ok_or(TransportError::Rejected)?;
        let message = PublishMessage::build()
            .payload(envelope_json.as_bytes().to_vec().into())
            .message_id(event_id.to_string());
        tokio::time::timeout(self.acknowledgement_timeout, async {
            let acknowledgement = self
                .context
                .send_publish(subject, message)
                .await
                .map_err(|_| TransportError::Rejected)?;
            acknowledgement
                .await
                .map_err(|_| TransportError::Rejected)?;
            Ok::<(), TransportError>(())
        })
        .await
        .map_err(|_| TransportError::AcknowledgementTimeout)?
    }
}
