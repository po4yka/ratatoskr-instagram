//! The broker-facing tasks of the process: the Platform command consumer, the outbox relay and
//! the capture resolver (XR-021 CONTRACTS.md S02 rules 3 to 6, S10 CD2).
//!
//! Everything fallible is prepared by [`BusLane::connect`] before readiness can be reported: the
//! broker connection, the Edge-provisioned durable and the public-resolution surface. Once the
//! tasks run, none of them may end quietly: the first one to return, or to panic, flips the
//! bus readiness fact and tells the servers to stop, so the process exits non-zero instead of
//! staying ready while it can no longer finish captures.

use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use futures_util::StreamExt as _;
use ratatoskr_instagram_archive::capture_resolution::{CaptureResolver, ResolutionPolicy};
use ratatoskr_instagram_archive::public_surface::HttpPublicSurface;
use ratatoskr_instagram_archive::{BusConfig, Config, Database, PublicResolutionConfig};
use ratatoskr_instagram_archive_service::RuntimeState;
use ratatoskr_instagram_archive_service::nats_transport::NatsEventTransport;
use tokio::sync::watch;
use tokio::task::JoinSet;

/// The Edge-provisioned durable this service consumes Platform commands from.
const COMMAND_DURABLE: &str = "ratatoskr_instagram_browser_capture";
/// The stream that durable lives on.
const COMMAND_STREAM: &str = "ratatoskr_commands";
/// The only subject the durable may filter.
const COMMAND_SUBJECT: &str = "cmd.instagram.capture.requested.v1";

/// Why the broker lane could not be prepared, mapped to the process exit class.
#[derive(Debug)]
pub(crate) enum LaneError {
    /// The configuration cannot work (exit 78).
    Configuration(String),
    /// The broker or the durable is unavailable (exit 1).
    Runtime(String),
}

/// A connected broker lane that has not started its tasks yet.
pub(crate) struct BusLane {
    context: jetstream::Context,
    messages: jetstream::consumer::pull::Stream,
    surface: HttpPublicSurface,
}

impl BusLane {
    /// Reads the access token, builds the surface, connects and verifies the durable.
    ///
    /// A configured broker is mandatory rather than best-effort: accepting a process as ready
    /// while its explicit browser-capture path cannot finish work is an operational lie. The
    /// Platform-owned command stream and the fixed consumer must already exist.
    pub(crate) async fn connect(config: &Config, bus: &BusConfig) -> Result<Self, LaneError> {
        let resolution = &config.public_resolution;
        let token = resolution
            .load_access_token()
            .map_err(|error| LaneError::Configuration(error.to_string()))?;
        let surface = HttpPublicSurface::new(&resolution.endpoint, token)
            .map_err(|error| LaneError::Configuration(error.to_string()))?;

        let client = match bus.nkey_seed_path.as_deref() {
            Some(seed_path) => {
                let seed = std::fs::read_to_string(seed_path).map_err(|_| {
                    LaneError::Runtime("the NATS nkey seed could not be read".to_owned())
                })?;
                async_nats::ConnectOptions::with_nkey(seed.trim().to_owned())
                    .connect(&bus.url)
                    .await
                    .map_err(|_| {
                        LaneError::Runtime(
                            "the NATS broker rejected the configured identity".to_owned(),
                        )
                    })?
            }
            None => async_nats::connect(&bus.url).await.map_err(|_| {
                LaneError::Runtime("the NATS broker could not be reached".to_owned())
            })?,
        };
        let context = jetstream::new(client);
        let consumer: jetstream::consumer::PullConsumer = context
            .get_consumer_from_stream(COMMAND_DURABLE, COMMAND_STREAM)
            .await
            .map_err(|_| {
                LaneError::Runtime(
                    "the preprovisioned Instagram durable command consumer is unavailable"
                        .to_owned(),
                )
            })?;
        validate_command_consumer(&consumer).map_err(LaneError::Runtime)?;
        let messages = consumer.messages().await.map_err(|_| {
            LaneError::Runtime(
                "the Instagram command consumer cannot receive deliveries".to_owned(),
            )
        })?;
        Ok(Self {
            context,
            messages,
            surface,
        })
    }

    /// Starts the consumer, the relay and the resolver under one supervisor.
    ///
    /// The supervisor returns when the first of them does. It marks the bus failed on the
    /// runtime and raises `failed`, which stops both servers.
    pub(crate) fn spawn(
        self,
        database: Database,
        config: &Config,
        runtime: Arc<RuntimeState>,
        failed: watch::Sender<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let mut tasks: JoinSet<&'static str> = JoinSet::new();
        let consumer_database = database.clone();
        let messages = self.messages;
        tasks.spawn(async move {
            consume_browser_captures(consumer_database, messages).await;
            "command consumer"
        });
        let publisher_database = database.clone();
        let transport = NatsEventTransport::new(self.context);
        let interval = Duration::from_millis(config.publisher.poll_interval_ms);
        let batch_size = config.publisher.batch_size;
        tasks.spawn(async move {
            relay_outbox(publisher_database, transport, interval, batch_size).await;
            "outbox relay"
        });
        let resolution = config.public_resolution.clone();
        let surface = self.surface;
        tasks.spawn(async move {
            resolve_captures(database, surface, resolution).await;
            "capture resolver"
        });
        runtime.set_bus_running();
        tokio::spawn(async move {
            let stopped = match tasks.join_next().await {
                Some(Ok(name)) => name,
                Some(Err(_)) => "a bus task panicked",
                None => "no bus task",
            };
            tracing::error!(
                error_class = "bus_task_stopped",
                task = stopped,
                "a bus task stopped before shutdown; the process is no longer ready and exits"
            );
            runtime.set_bus_failed();
            let _ = failed.send(true);
            // Dropping the set aborts the remaining tasks.
        })
    }
}

/// Refuses a broker durable that would broaden delivery or acknowledgement semantics.
fn validate_command_consumer(consumer: &jetstream::consumer::PullConsumer) -> Result<(), String> {
    let info = consumer.cached_info();
    let config = &info.config;
    if info.stream_name != COMMAND_STREAM
        || info.name != COMMAND_DURABLE
        || config.durable_name.as_deref() != Some(COMMAND_DURABLE)
        || config.filter_subject != COMMAND_SUBJECT
        || config.deliver_subject.is_some()
        || config.ack_policy != jetstream::consumer::AckPolicy::Explicit
    {
        return Err("the preprovisioned Instagram consumer configuration is unsafe".to_owned());
    }
    Ok(())
}

/// Applies one broker delivery only after the archive inbox has recorded it.
async fn consume_browser_captures(
    database: Database,
    mut messages: jetstream::consumer::pull::Stream,
) {
    while let Some(delivery) = messages.next().await {
        let Ok(message) = delivery else {
            tracing::warn!("the Instagram JetStream delivery could not be read");
            continue;
        };
        ratatoskr_instagram_archive_service::command_consumer::consume_one(&database, &message)
            .await;
    }
}

/// Drains the outbox forever, one bounded pass per interval.
async fn relay_outbox(
    database: Database,
    transport: NatsEventTransport,
    interval: Duration,
    batch_size: u32,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        match ratatoskr_instagram_archive::publishing::run_once(
            database.pool(),
            &transport,
            batch_size,
        )
        .await
        {
            Ok(summary) if summary.failed > 0 => {
                tracing::warn!(
                    failed = summary.failed,
                    remaining = summary.remaining,
                    "outbox pass completed with failures"
                );
            }
            Ok(_) => {}
            Err(error) => tracing::error!(%error, "outbox pass could not run"),
        }
    }
}

/// Resolves due captures forever, one bounded pass per interval.
async fn resolve_captures(
    database: Database,
    surface: HttpPublicSurface,
    config: PublicResolutionConfig,
) {
    let resolver = CaptureResolver::new(database);
    let policy = ResolutionPolicy {
        max_attempts: config.max_attempts,
        batch_size: config.batch_size,
    };
    let mut ticker = tokio::time::interval(Duration::from_millis(config.poll_interval_ms));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        match resolver
            .run_due_once(&surface, time::OffsetDateTime::now_utc(), policy)
            .await
        {
            Ok(summary) if summary.claimed > 0 => tracing::info!(
                claimed = summary.claimed,
                fetched = summary.fetched,
                retried = summary.retried,
                reported = summary.reported,
                failed = summary.failed,
                "capture resolution pass completed"
            ),
            Ok(_) => {}
            Err(error) => tracing::error!(
                error_class = "capture_resolution_claim",
                %error,
                "capture resolution pass could not run"
            ),
        }
    }
}
