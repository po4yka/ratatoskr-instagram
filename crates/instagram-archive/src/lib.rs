#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Domain library for the Ratatoskr Instagram bounded context.
//!
//! The foundation owns process configuration, telemetry bootstrap, and
//! application of the first-version `instagram_archive` schema. Account
//! connection, explicit captures, public resolution, and Data Export imports
//! arrive with later implementation plan items.

/// The capability matrix: acquisition modes, support status, authority
/// ceilings, and the upstream-versus-preservation boundary.
pub mod capability;
pub mod config;
/// The owned `PostgreSQL` pool and the embedded `instagram_archive` schema.
pub mod database;
pub mod telemetry;

pub use capability::{
    AcquisitionMode, AvailabilityObservationKind, ModeCapability, NATIVE_SAVED_LIST_SYNC,
    NativeSavedSupport, PreservationState, SavedAuthority, SupportStatus, UpstreamStatus,
    retention_after_observation,
};
pub use config::{AdminConfig, Config, ConfigError, Limits, StorageConfig, TelemetryConfig};
pub use database::{Database, PersistenceError};
pub use telemetry::{TelemetryError, TelemetryGuard, init_telemetry};

#[cfg(feature = "test-support")]
pub mod test_support;
