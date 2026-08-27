#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Ratatoskr Instagram Archive service process.
//!
//! Sequence, in this order and no other: load configuration, install
//! telemetry, refuse to start without a database, connect, apply the schema,
//! bind both listeners (operator and product), mark readiness — then serve
//! until SIGTERM or SIGINT and drain within the configured bound.
//!
//! Exit codes: `0` clean run; `1` runtime startup failure; `78`
//! (`EX_CONFIG`) configuration unreadable or invalid.

use std::future::IntoFuture as _;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use futures_util::StreamExt as _;
use secrecy::ExposeSecret as _;

use ratatoskr_instagram_archive::provider::{
    REFRESH_SUPPORTED, ReqwestInstagramProvider, ReqwestOAuthCodeRelay,
};
use ratatoskr_instagram_archive::publishing::TransportError;
use ratatoskr_instagram_archive::telemetry::SERVICE_NAME;
use ratatoskr_instagram_archive::{BusConfig, Config, Database, PublisherConfig};
use ratatoskr_instagram_archive_service::{OfficialAccountRuntime, RuntimeState};
use uuid::Uuid;

/// How often the prober copies the database answer into the readiness facts.
///
/// Long enough that the probe is not itself load; short enough that a
/// readiness state is never more than one scrape interval stale.
const DATABASE_PROBE_INTERVAL: Duration = Duration::from_secs(5);

fn main() -> ExitCode {
    if std::env::args().nth(1).as_deref() == Some("check-config") {
        return check_config();
    }
    match tokio_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(exit) => exit,
    }
}

/// `<binary> check-config`: load and validate without binding anything.
///
/// Both outputs go to stderr: no subscriber exists yet, and a stray line on
/// stdout could be mistaken for a log record. The effective configuration is
/// safe to render because every secret member is redacted by type.
fn check_config() -> ExitCode {
    match Config::load() {
        Ok(config) => {
            eprintln!("{SERVICE_NAME}: configuration is valid.\n{config:#?}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{SERVICE_NAME}: {error}");
            ExitCode::from(78)
        }
    }
}

#[tokio::main]
#[allow(
    clippy::too_many_lines,
    reason = "startup order is a security and readiness invariant kept linear for auditability"
)]
async fn tokio_main() -> Result<(), ExitCode> {
    let config = match Config::load() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{SERVICE_NAME}: {error}");
            return Err(ExitCode::from(78));
        }
    };

    let guard = match ratatoskr_instagram_archive::init_telemetry(&config.telemetry) {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("{SERVICE_NAME}: refusing to start; telemetry failed: {error}");
            return Err(ExitCode::FAILURE);
        }
    };
    tracing::info!(
        service_name = SERVICE_NAME,
        version = ratatoskr_instagram_archive::telemetry::VERSION,
        git_sha = ratatoskr_instagram_archive::telemetry::GIT_SHA,
        config = ?config,
        "startup"
    );

    // Refusing to start without a database is deliberate: every capability
    // this binary will ever offer reads the archive database, and a process
    // that started anyway would report itself ready and fail everything.
    let Some(database_url) = config.storage.database_url.as_ref() else {
        eprintln!("{SERVICE_NAME}: refusing to start without RATATOSKR__STORAGE__DATABASE_URL");
        return Err(ExitCode::FAILURE);
    };

    let database = Database::connect(
        database_url.expose_secret(),
        config.limits.database_connections,
        Duration::from_millis(config.limits.database_acquire_timeout_ms),
    )
    .await
    .map_err(|error| {
        tracing::error!(%error, "the database could not be reached");
        ExitCode::FAILURE
    })?;
    database.apply_schema().await.map_err(|error| {
        tracing::error!(%error, "the schema could not be applied");
        ExitCode::FAILURE
    })?;
    database
        .scrub_stranded_revocations(time::OffsetDateTime::now_utc())
        .await
        .map_err(|error| {
            tracing::error!(%error, "stranded account revocations could not be scrubbed");
            ExitCode::FAILURE
        })?;
    let official_accounts = build_official_runtime(&config).map_err(|error| {
        tracing::error!(error_class = "official_oauth_runtime", %error, "official OAuth runtime could not be built");
        ExitCode::from(78)
    })?;

    let command_consumer = start_command_consumer(&config, database.clone()).await?;

    let runtime = Arc::new(RuntimeState::new());
    let admin_listener = tokio::net::TcpListener::bind(config.admin.listen_address)
        .await
        .map_err(|error| {
            tracing::error!(
                bind = %config.admin.listen_address,
                %error,
                "the operator listener could not bind"
            );
            ExitCode::FAILURE
        })?;
    let api_listener = tokio::net::TcpListener::bind(config.api.listen_address)
        .await
        .map_err(|error| {
            tracing::error!(
                bind = %config.api.listen_address,
                %error,
                "the product listener could not bind"
            );
            ExitCode::FAILURE
        })?;

    // The first probe happens before readiness flips, so the process never
    // reports itself ready over an unverified dependency.
    let prober = spawn_database_prober(database.clone(), Arc::clone(&runtime));
    // The publisher drains the outbox at its own cadence; facts are durable
    // rows, so a slow or failed pass degrades freshness, never correctness.
    let publisher = spawn_outbox_publisher(database.clone(), &config.publisher);
    runtime.mark_startup_complete();
    tracing::info!(
        admin = %config.admin.listen_address,
        api = %config.api.listen_address,
        "startup complete"
    );

    let metrics_handle = guard.metrics_handle();
    let shutdown_bound = Duration::from_millis(config.limits.shutdown_timeout_ms);
    let (admin_result, api_result) = tokio::join!(
        serve_admin(
            admin_listener,
            Arc::clone(&runtime),
            move || metrics_handle.render(),
            shutdown_bound,
        ),
        serve_product(
            api_listener,
            database.clone(),
            official_accounts,
            shutdown_bound,
        ),
    );
    prober.abort();
    publisher.abort();
    if let Some(consumer) = command_consumer {
        consumer.abort();
    }
    database.close().await;

    match (admin_result, api_result) {
        (Ok(()), Ok(())) => {
            guard.shutdown();
            Ok(())
        }
        (admin_result, api_result) => {
            for (plane, result) in [("operator", admin_result), ("product", api_result)] {
                if let Err(error) = result {
                    tracing::error!(%error, "the {plane} server failed");
                }
            }
            Err(ExitCode::FAILURE)
        }
    }
}

/// Starts the optional broker lane and turns a configured-bus failure into a
/// startup failure before readiness can be exposed.
async fn start_command_consumer(
    config: &Config,
    database: Database,
) -> Result<Option<tokio::task::JoinHandle<()>>, ExitCode> {
    let Some(bus) = config.bus.as_ref() else {
        return Ok(None);
    };
    spawn_browser_capture_consumer(database, bus)
        .await
        .map(Some)
        .map_err(|error| {
            tracing::error!(%error, "the JetStream command consumer could not start");
            ExitCode::FAILURE
        })
}

/// Connects the provider-specific durable consumer before the service is ready.
///
/// A configured broker is mandatory rather than best-effort: accepting a
/// process as ready while its explicit browser-capture path cannot consume is
/// an operational lie. The Platform-owned command stream must already exist.
async fn spawn_browser_capture_consumer(
    database: Database,
    bus: &BusConfig,
) -> Result<tokio::task::JoinHandle<()>, String> {
    let client = match bus.nkey_seed_path.as_deref() {
        Some(seed_path) => {
            let seed = std::fs::read_to_string(seed_path)
                .map_err(|_| "the NATS nkey seed could not be read".to_owned())?;
            async_nats::ConnectOptions::with_nkey(seed.trim().to_owned())
                .connect(&bus.url)
                .await
                .map_err(|_| "the NATS broker rejected the configured consumer".to_owned())?
        }
        None => async_nats::connect(&bus.url)
            .await
            .map_err(|_| "the NATS broker could not be reached".to_owned())?,
    };
    let context = jetstream::new(client);
    let consumer: jetstream::consumer::PullConsumer = context
        .get_consumer_from_stream("ratatoskr_instagram_browser_capture", "ratatoskr_commands")
        .await
        .map_err(|_| {
            "the preprovisioned Instagram durable command consumer is unavailable".to_owned()
        })?;
    validate_browser_capture_consumer(&consumer)?;
    let messages = consumer
        .messages()
        .await
        .map_err(|_| "the Instagram command consumer cannot receive deliveries".to_owned())?;
    Ok(tokio::spawn(async move {
        consume_browser_captures(database, messages).await;
    }))
}

/// Refuses a broker durable that would broaden delivery or acknowledgement semantics.
fn validate_browser_capture_consumer(
    consumer: &jetstream::consumer::PullConsumer,
) -> Result<(), String> {
    let info = consumer.cached_info();
    let config = &info.config;
    if info.stream_name != "ratatoskr_commands"
        || info.name != "ratatoskr_instagram_browser_capture"
        || config.durable_name.as_deref() != Some("ratatoskr_instagram_browser_capture")
        || config.filter_subject != "cmd.instagram.capture.requested.v1"
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

/// Serves one plane until its server stops or a signal arrives, draining
/// within the bound either way. The shared pool is closed by the caller.
async fn serve_plane(
    plane: &'static str,
    listener: tokio::net::TcpListener,
    router: axum::Router,
    on_signal: impl Fn(),
    shutdown_timeout: Duration,
) -> Result<(), String> {
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let server = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let _ignored = shutdown_rx.await;
        })
        .into_future();
    tokio::pin!(server);
    tokio::select! {
        result = &mut server => {
            result.map_err(|error| error.to_string())
        }
        result = shutdown_signal() => {
            result.map_err(|error| error.to_string())?;
            on_signal();
            let _ignored = shutdown_tx.send(());
            if tokio::time::timeout(shutdown_timeout, &mut server).await.is_err() {
                return Err(format!("the {plane} server did not stop within the shutdown bound"));
            }
            Ok(())
        }
    }
}

/// The operator plane: health, readiness, metrics, version.
async fn serve_admin(
    listener: tokio::net::TcpListener,
    runtime: Arc<RuntimeState>,
    render_metrics: impl Fn() -> String + Send + Sync + 'static,
    shutdown_timeout: Duration,
) -> Result<(), String> {
    let router =
        ratatoskr_instagram_archive_service::admin_router(Arc::clone(&runtime), render_metrics);
    serve_plane(
        "operator",
        listener,
        router,
        // Readiness fails immediately; the listener stays open through the
        // drain window so in-flight requests finish.
        || runtime.begin_draining(),
        shutdown_timeout,
    )
    .await
}

/// The product plane: capture intake for platform callers.
async fn serve_product(
    listener: tokio::net::TcpListener,
    database: Database,
    official_accounts: Option<OfficialAccountRuntime>,
    shutdown_timeout: Duration,
) -> Result<(), String> {
    let router = ratatoskr_instagram_archive_service::product_router_with_official_accounts(
        database,
        official_accounts,
    );
    serve_plane("product", listener, router, || (), shutdown_timeout).await
}

fn build_official_runtime(config: &Config) -> Result<Option<OfficialAccountRuntime>, String> {
    if !config.oauth.enabled {
        return Ok(None);
    }
    let keyring = config
        .oauth
        .credential_keyring()
        .map_err(|_| "credential keyring is invalid".to_owned())?
        .ok_or_else(|| "credential keyring is absent".to_owned())?;
    let client_id = config
        .oauth
        .client_id
        .clone()
        .ok_or_else(|| "client id is absent".to_owned())?;
    let client_secret = config
        .oauth
        .client_secret
        .clone()
        .ok_or_else(|| "client secret is absent".to_owned())?;
    let redirect_uri = config
        .oauth
        .redirect_uri
        .clone()
        .ok_or_else(|| "redirect URI is absent".to_owned())?;
    let relay_url = config
        .oauth
        .platform_relay_url
        .as_deref()
        .ok_or_else(|| "relay URL is absent".to_owned())?
        .parse::<reqwest::Url>()
        .map_err(|_| "relay URL is invalid".to_owned())?;
    let relay_token = config
        .oauth
        .platform_relay_token
        .clone()
        .ok_or_else(|| "relay token is absent".to_owned())?;
    let request_timeout = Duration::from_millis(config.oauth.request_timeout_ms);
    let provider = ReqwestInstagramProvider::new(
        client_id.clone(),
        client_secret,
        redirect_uri.clone(),
        Duration::from_millis(config.oauth.connect_timeout_ms),
        request_timeout,
        config.oauth.max_response_bytes,
    )
    .map_err(|_| "provider client could not be built".to_owned())?;
    let relay = ReqwestOAuthCodeRelay::new(
        relay_url,
        relay_token,
        request_timeout,
        config.oauth.max_response_bytes,
    )
    .map_err(|_| "relay client could not be built".to_owned())?;
    Ok(Some(OfficialAccountRuntime::new(
        keyring,
        Arc::new(provider),
        Arc::new(relay),
        client_id,
        redirect_uri,
        Duration::from_secs(config.oauth.flow_ttl_seconds),
        config.oauth.call_budget,
        config.oauth.discovery_retries,
        false,
        REFRESH_SUPPORTED,
    )))
}

/// Copies the database answer into readiness forever.
///
/// A separate loop because it answers a different question at a different
/// cadence than any request: this keeps `/health/ready` honest while adding
/// almost no load — one `select 1` per interval.
fn spawn_database_prober(
    database: Database,
    runtime: Arc<RuntimeState>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(DATABASE_PROBE_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            runtime.set_database_reachable(database.ping().await.is_ok());
        }
    })
}

/// The logging carrier behind the transport seam: facts are handed to the
/// structured log until a broker lane lands. Delivery always succeeds, which
/// is honest for a log line and keeps at-least-once semantics intact — rows
/// are marked published only after this returns `Ok`.
struct LoggingTransport;

impl ratatoskr_instagram_archive::publishing::EventTransport for LoggingTransport {
    async fn deliver(&self, event_id: Uuid, _envelope_json: &str) -> Result<(), TransportError> {
        tracing::info!(event = %event_id, "social-source fact delivered to logging transport");
        Ok(())
    }
}

/// Drains the outbox forever, one bounded pass per interval.
fn spawn_outbox_publisher(
    database: Database,
    publisher: &PublisherConfig,
) -> tokio::task::JoinHandle<()> {
    let interval = std::time::Duration::from_millis(publisher.poll_interval_ms);
    let batch_size = publisher.batch_size;
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await; // consume the immediate tick; publish on cadence
        let transport = LoggingTransport;
        loop {
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
            ticker.tick().await;
        }
    })
}

#[cfg(unix)]
async fn shutdown_signal() -> std::io::Result<()> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        _ = terminate.recv() => Ok(()),
        result = tokio::signal::ctrl_c() => result,
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() -> std::io::Result<()> {
    tokio::signal::ctrl_c().await
}
