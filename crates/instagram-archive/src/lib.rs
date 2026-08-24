#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Domain library for the Ratatoskr Instagram bounded context.
//!
//! The foundation owns process configuration, telemetry bootstrap, and
//! application of the first-version `instagram_archive` schema. Account
//! connection, explicit captures, public resolution, and Data Export imports
//! arrive with later implementation plan items.

pub mod config;
/// The owned `PostgreSQL` pool and the embedded `instagram_archive` schema.
pub mod database;
pub mod telemetry;

pub use config::{AdminConfig, Config, ConfigError, Limits, StorageConfig, TelemetryConfig};
pub use database::{Database, PersistenceError};
pub use telemetry::{TelemetryError, TelemetryGuard, init_telemetry};

#[cfg(feature = "test-support")]
pub mod test_support;
