#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Ratatoskr Instagram Archive service process.
//!
//! Sequence, in this order and no other: load configuration, install
//! telemetry, refuse to start without a database, connect, apply the schema,
//! bind the operator listener, mark readiness — then serve until SIGTERM or
//! SIGINT and drain within the configured bound.
//!
//! Exit codes: `0` clean run; `1` runtime startup failure; `78`
//! (`EX_CONFIG`) configuration unreadable or invalid.

use std::future::IntoFuture as _;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use secrecy::ExposeSecret as _;

use ratatoskr_instagram_archive::telemetry::SERVICE_NAME;
use ratatoskr_instagram_archive::{Config, Database};
use ratatoskr_instagram_archive_service::RuntimeState;

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

    let runtime = Arc::new(RuntimeState::new());
    let listener = tokio::net::TcpListener::bind(config.admin.listen_address)
        .await
        .map_err(|error| {
            tracing::error!(
                bind = %config.admin.listen_address,
                %error,
                "the operator listener could not bind"
            );
            ExitCode::FAILURE
        })?;

    // The first probe happens before readiness flips, so the process never
    // reports itself ready over an unverified dependency.
    let prober = spawn_database_prober(database.clone(), Arc::clone(&runtime));
    runtime.mark_startup_complete();
    tracing::info!(admin = %config.admin.listen_address, "startup complete");

    let metrics_handle = guard.metrics_handle();
    let serve_result = serve_admin(
        listener,
        Arc::clone(&runtime),
        database,
        move || metrics_handle.render(),
        Duration::from_millis(config.limits.shutdown_timeout_ms),
    )
    .await;

    prober.abort();

    match serve_result {
        Ok(()) => {
            guard.shutdown();
            Ok(())
        }
        Err(error) => {
            tracing::error!(%error, "the operator server failed");
            Err(ExitCode::FAILURE)
        }
    }
}

async fn serve_admin(
    listener: tokio::net::TcpListener,
    runtime: Arc<RuntimeState>,
    database: Database,
    render_metrics: impl Fn() -> String + Send + Sync + 'static,
    shutdown_timeout: Duration,
) -> Result<(), String> {
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let server = axum::serve(
        listener,
        ratatoskr_instagram_archive_service::admin_router(runtime.clone(), render_metrics),
    )
    .with_graceful_shutdown(async move {
        let _ignored = shutdown_rx.await;
    })
    .into_future();
    tokio::pin!(server);
    tokio::select! {
        result = &mut server => {
            database.close().await;
            result.map_err(|error| error.to_string())
        }
        result = shutdown_signal() => {
            result.map_err(|error| error.to_string())?;
            // Readiness fails immediately; the listener stays open through
            // the drain window so in-flight requests finish.
            runtime.begin_draining();
            let _ignored = shutdown_tx.send(());
            if tokio::time::timeout(shutdown_timeout, &mut server).await.is_err() {
                database.close().await;
                return Err("the operator server did not stop within the shutdown bound".to_owned());
            }
            database.close().await;
            Ok(())
        }
    }
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
