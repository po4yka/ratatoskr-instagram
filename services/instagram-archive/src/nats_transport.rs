//! The `JetStream` carrier behind the outbox seam (XR-021 CONTRACTS.md S02 rules 1 to 4).
//!
//! [`NatsEventTransport`] publishes a stored canonical envelope to `evt.<event_type>` with the
//! event id as `Nats-Msg-Id`, and returns `Ok` only after the `JetStream` acknowledgement, so the
//! outbox marks a row published only when the broker has stored it. Only the four event types
//! this service is allowed to publish have a subject; any other type is a programming error and
//! is refused rather than guessed.

use async_nats::jetstream;
use ratatoskr_event_envelope::EventEnvelope;
use ratatoskr_instagram_archive::BusConfig;
use ratatoskr_instagram_archive::publishing::{EventTransport, TransportError, UndeliverableClass};
use uuid::Uuid;

/// The inbox prefix of the Instagram identity: the one subject tree its stanza in the deployed ACL
/// lets it subscribe to (`subscribe.allow = ["_INBOX_instagram.>"]`). Every `JetStream` reply reaches
/// the service on a subject under it, so a connection without the prefix never receives one.
pub const INBOX_PREFIX: &str = "_INBOX_instagram";

/// Why the broker connection could not be opened. The text is safe to log: it never carries the
/// seed or the URL.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BusConnectError {
    /// The nkey seed file named by the configuration could not be read.
    #[error("the NATS nkey seed could not be read")]
    Seed,
    /// The broker refused the configured identity.
    #[error("the NATS broker rejected the configured identity")]
    Rejected,
    /// The broker could not be reached.
    #[error("the NATS broker could not be reached")]
    Unreachable,
}

/// Opens the service's one broker connection (XR-021 CONTRACTS.md R3-01).
///
/// # Errors
///
/// Returns [`BusConnectError::Seed`] when the nkey seed file cannot be read,
/// [`BusConnectError::Rejected`] when the broker refuses the identity and
/// [`BusConnectError::Unreachable`] when there is no anonymous broker to reach.
pub async fn connect_bus(bus: &BusConfig) -> Result<async_nats::Client, BusConnectError> {
    let (options, failure) = match bus.nkey_seed_path.as_deref() {
        Some(seed_path) => {
            let seed = std::fs::read_to_string(seed_path).map_err(|_| BusConnectError::Seed)?;
            (
                async_nats::ConnectOptions::with_nkey(seed.trim().to_owned()),
                BusConnectError::Rejected,
            )
        }
        None => (
            async_nats::ConnectOptions::new(),
            BusConnectError::Unreachable,
        ),
    };
    options
        .custom_inbox_prefix(INBOX_PREFIX)
        .connect(&bus.url)
        .await
        .map_err(|_| failure)
}

/// The closed set of event types this service publishes, each to `evt.<type>`.
pub const ALLOWED_EVENT_TYPES: [&str; 4] = [
    "platform.operation.reported.v1",
    "social.source.captured.v1",
    "social.source.updated.v1",
    "social.source.removed.v1",
];

/// Bytes kept free below the server's `max_payload` for the message headers (XR-021
/// CONTRACTS.md R2-04).
const PAYLOAD_HEADROOM: usize = 1024;

/// Publishes outbox envelopes to the shared `JetStream` events stream.
#[derive(Debug, Clone)]
pub struct NatsEventTransport {
    context: jetstream::Context,
}

impl NatsEventTransport {
    /// A transport over an already connected `JetStream` context.
    #[must_use]
    pub fn new(context: jetstream::Context) -> Self {
        Self { context }
    }
}

/// The subject of a stored envelope, or why it has none.
fn subject_for(event_id: Uuid, envelope_json: &str) -> Result<String, TransportError> {
    let envelope = EventEnvelope::from_json(envelope_json.as_bytes()).map_err(|_| {
        TransportError::Transient("the stored envelope is not a canonical event".to_owned())
    })?;
    if envelope.event_id.0 != event_id {
        return Err(TransportError::Transient(
            "the stored envelope id differs from its outbox row id".to_owned(),
        ));
    }
    let event_type = envelope.event_type.to_string();
    if !ALLOWED_EVENT_TYPES.contains(&event_type.as_str()) {
        return Err(TransportError::Transient(
            "the event type has no allowed publish subject".to_owned(),
        ));
    }
    Ok(format!("evt.{event_type}"))
}

impl EventTransport for NatsEventTransport {
    async fn deliver(&self, event_id: Uuid, envelope_json: &str) -> Result<(), TransportError> {
        let subject = subject_for(event_id, envelope_json)?;
        // A body the server will never accept is refused on every attempt, so retrying it
        // only keeps the relay failing; say so once, before the publish.
        let limit = self
            .context
            .client()
            .max_payload()
            .saturating_sub(PAYLOAD_HEADROOM);
        if envelope_json.len() > limit {
            return Err(TransportError::Undeliverable(
                UndeliverableClass::PayloadTooLarge,
            ));
        }
        let mut headers = async_nats::HeaderMap::new();
        headers.insert(async_nats::header::NATS_MESSAGE_ID, event_id.to_string());
        // A denied publish is invisible to the client: it surfaces only as a missing
        // acknowledgement, so the failure text points at the broker log.
        let acknowledgement = self
            .context
            .publish_with_headers(subject, headers, envelope_json.as_bytes().to_vec().into())
            .await
            .map_err(|_| unacknowledged())?;
        acknowledgement.await.map_err(|_| unacknowledged())?;
        Ok(())
    }
}

fn unacknowledged() -> TransportError {
    TransportError::Transient(
        "the broker did not acknowledge the publish; check the NATS server log for a Publish Violation"
            .to_owned(),
    )
}
