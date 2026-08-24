#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Process boundary for Ratatoskr Instagram Archive: the runtime lifecycle
//! facts and the loopback operator router.
//!
//! - [`RuntimeState`] — the facts readiness is computed from, written by the
//!   boot sequence and its prober, read by every readiness request.
//! - [`admin_router`] — liveness, readiness, metrics and version, on the
//!   operator listener only.
//!
//! Every admin response carries `Cache-Control: no-store`: a cached "ready"
//! is a routing decision made from stale data.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Serialize;

/// No database is configured for this process.
const DATABASE_ABSENT: u8 = 0;
/// The last probe answered.
const DATABASE_UP: u8 = 1;
/// The last probe did not answer.
const DATABASE_DOWN: u8 = 2;

/// The deployable role this process serves, one of one.
pub const ROLE: &str = "archive";

/// Shared process lifecycle used by readiness computation.
///
/// Readiness itself is startup and drain only; a dependency that flaps must
/// not flap a process that is still accepting work correctly. What the last
/// database probe found is REPORTED in the check list instead.
#[derive(Debug)]
pub struct RuntimeState {
    startup_complete: AtomicBool,
    draining: AtomicBool,
    database: AtomicU8,
}

impl RuntimeState {
    /// A process that has bound nothing yet: readiness fails, liveness does not.
    #[must_use]
    pub fn new() -> Self {
        Self {
            startup_complete: AtomicBool::new(false),
            draining: AtomicBool::new(false),
            database: AtomicU8::new(DATABASE_ABSENT),
        }
    }

    /// Configuration validated, telemetry installed, every configured
    /// listener bound. Set exactly once.
    pub fn mark_startup_complete(&self) {
        self.startup_complete.store(true, Ordering::Release);
    }

    /// A shutdown signal arrived. Readiness fails immediately; the listener
    /// stays open through the drain window.
    pub fn begin_draining(&self) {
        self.draining.store(true, Ordering::Release);
    }

    /// Record what the latest database probe found.
    ///
    /// Called by the prober, not by a request: a readiness probe must never
    /// open a connection, or a saturated pool would make the health check the
    /// thing that finishes it off.
    pub fn set_database_reachable(&self, reachable: bool) {
        self.database.store(
            if reachable {
                DATABASE_UP
            } else {
                DATABASE_DOWN
            },
            Ordering::Release,
        );
    }

    /// Whether new work may be routed to this process.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.startup_complete.load(Ordering::Acquire) && !self.draining.load(Ordering::Acquire)
    }

    /// The readiness checks, sorted by name so two consecutive bodies are
    /// byte-identical and `diff` stays usable at 03:00.
    ///
    /// A database that was never configured reports no database check at all:
    /// a passing check for something that does not exist is the readiness
    /// equivalent of an always-zero metric.
    #[must_use]
    pub fn checks(&self) -> Vec<Check> {
        let draining = self.draining.load(Ordering::Acquire);
        let started = self.startup_complete.load(Ordering::Acquire);
        let mut checks = vec![
            Check {
                name: CheckName::Drain,
                state: pass(!draining),
                reason: draining.then_some(CheckReason::ShutdownRequested),
            },
            Check {
                name: CheckName::Startup,
                state: pass(started),
                reason: (!started).then_some(CheckReason::StartupIncomplete),
            },
        ];

        if self.database.load(Ordering::Acquire) != DATABASE_ABSENT {
            let up = self.database.load(Ordering::Acquire) == DATABASE_UP;
            checks.push(Check {
                name: CheckName::Database,
                state: pass(up),
                reason: (!up).then_some(CheckReason::DependencyUnavailable),
            });
        }

        checks.sort_unstable_by_key(|check| check.name);
        checks
    }
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self::new()
    }
}

fn pass(subject: bool) -> CheckState {
    if subject {
        CheckState::Pass
    } else {
        CheckState::Fail
    }
}

/// One readiness check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    /// The logical name of the subject.
    pub name: CheckName,
    /// Whether the subject passes.
    pub state: CheckState,
    /// Why it does not, when it does not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<CheckReason>,
}

/// A logical token from a closed set. Never a hostname, port, DSN or driver
/// message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CheckName {
    /// The database answers. Present only when one is configured.
    Database,
    /// No shutdown signal has arrived.
    Drain,
    /// Configuration, telemetry and every configured listener are up.
    Startup,
}

/// Whether one check passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
    /// The subject is healthy.
    Pass,
    /// The subject is not healthy.
    Fail,
}

/// A closed set of failure reasons. NEVER a formatted dependency error: a
/// driver message can carry a host, a port and sometimes a password.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CheckReason {
    /// The process has not finished binding its listeners.
    StartupIncomplete,
    /// A shutdown signal arrived and this instance is draining.
    ShutdownRequested,
    /// The last probe of the database did not answer.
    DependencyUnavailable,
}

/// The Prometheus text exposition format the `metrics` crate renders.
const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4";

struct AdminState {
    runtime: Arc<RuntimeState>,
    render_metrics: Box<dyn Fn() -> String + Send + Sync>,
}

/// Builds the loopback operator router.
pub fn admin_router(
    state: Arc<RuntimeState>,
    render_metrics: impl Fn() -> String + Send + Sync + 'static,
) -> Router {
    let state = Arc::new(AdminState {
        runtime: state,
        render_metrics: Box::new(render_metrics),
    });
    Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/metrics", get(metrics))
        .route("/version", get(version))
        .with_state(state)
        .layer(middleware::from_fn(no_store))
}

/// *This process's async runtime is scheduling tasks and the server answers.*
///
/// It consults nothing external, ever, and it answers 200 from bind until
/// exit INCLUDING throughout drain. Wiring liveness to a dependency converts
/// one database blip into a restart loop.
async fn live() -> Json<Liveness> {
    Json(Liveness {
        state: "live",
        role: ROLE,
    })
}

/// *Route new work to me.*
async fn ready(State(state): State<Arc<AdminState>>) -> Response {
    let ready = state.runtime.is_ready();
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(Readiness {
            state: if ready { "ready" } else { "not_ready" },
            role: ROLE,
            checks: state.runtime.checks(),
        }),
    )
        .into_response()
}

/// Prometheus pull. One route calling the renderer closure: no second HTTP
/// server and no push gateway.
async fn metrics(State(state): State<Arc<AdminState>>) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, PROMETHEUS_CONTENT_TYPE)],
        (state.render_metrics)(),
    )
}

/// The build identity, kept on the operator plane so a build fingerprint is
/// never public.
async fn version() -> Json<Version> {
    Json(Version {
        service: ratatoskr_instagram_archive::telemetry::SERVICE_NAME,
        role: ROLE,
        version: ratatoskr_instagram_archive::telemetry::VERSION,
        git_sha: ratatoskr_instagram_archive::telemetry::GIT_SHA,
        rust_version: ratatoskr_instagram_archive::telemetry::RUST_VERSION,
    })
}

/// `Cache-Control: no-store` on every admin response, including bare 404s.
async fn no_store(request: axum::extract::Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// `GET /health/live`.
#[derive(Serialize)]
struct Liveness {
    /// Always `live`. The property is `state`, not `status`.
    state: &'static str,
    /// The deployable role.
    role: &'static str,
}

/// `GET /health/ready`.
#[derive(Serialize)]
struct Readiness {
    /// `ready` | `not_ready`.
    state: &'static str,
    /// The deployable role.
    role: &'static str,
    /// Name-sorted, never a map, so two consecutive bodies are identical.
    checks: Vec<Check>,
}

/// `GET /version`.
#[allow(
    clippy::struct_field_names,
    reason = "the member names are the operator-facing JSON shape, not a naming choice"
)]
#[derive(Serialize)]
struct Version {
    /// The one wire identity of this bounded context.
    service: &'static str,
    /// The deployable role.
    role: &'static str,
    /// The crate version.
    version: &'static str,
    /// The build's git commit, or `unknown` outside a container build.
    git_sha: &'static str,
    /// The declared toolchain.
    rust_version: &'static str,
}
