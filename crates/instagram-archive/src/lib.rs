#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Domain library for the Ratatoskr Instagram bounded context.
//!
//! The foundation owns process configuration, telemetry bootstrap, and
//! application of the first-version `instagram_archive` schema. Explicit
//! capture intake and public resolution through the approved surface are
//! implemented; account connection and Data Export imports arrive with later
//! implementation plan items.

/// Official-account refresh, downgrade, and revoke persistence.
pub mod account;
/// Knowledge completion intake and local capture-result linkage.
pub mod analysis_linkage;
/// The capability matrix: acquisition modes, support status, authority
/// ceilings, and the upstream-versus-preservation boundary.
pub mod capability;
/// Total reconciliation of provider observations into per-account capabilities.
pub mod capability_reconciliation;
/// Explicit capture intake: submission identity, provenance, and the
/// unavailable fallback.
pub mod capture;
pub mod config;
/// Authenticated encryption for provider credential material.
pub mod credentials;
/// The owned `PostgreSQL` pool and the embedded `instagram_archive` schema.
pub mod database;
/// Owner-bound OAuth begin and callback completion.
pub mod oauth;
/// Canonicalization of client-delivered Instagram URLs into stable permalinks.
pub mod permalink;
/// Official Meta Instagram Login adapter contract.
pub mod provider;
/// Durable pre-I/O accounting for official provider calls.
pub mod provider_budget;
/// Social-source publishing: snapshot construction, transactional outbox
/// appends, and the at-least-once publisher loop over a transport seam.
pub mod publishing;
/// Public resolution: the approved surface seam, immutable parser-versioned
/// revisions of raw payloads, deterministic normalization, and truthful
/// failure observations.
pub mod resolution;
pub mod telemetry;
/// Local tombstones and `social.source.removed.v1` publication.
pub mod tombstone;

pub use analysis_linkage::{AnalysisCompletionError, AnalysisCompletionOutcome};
pub use capability::{
    AcquisitionMode, AvailabilityObservationKind, ModeCapability, NATIVE_SAVED_LIST_SYNC,
    NativeSavedSupport, PreservationState, SavedAuthority, SupportStatus, UpstreamStatus,
    retention_after_observation,
};
pub use config::{
    AdminConfig, Config, ConfigError, Limits, OAuthConfig, PublisherConfig, StorageConfig,
    TelemetryConfig,
};
pub use database::{Database, PersistenceError};
pub use permalink::{CanonicalPermalink, PermalinkError, PermalinkKind};
pub use publishing::{
    EventTransport, FactKind, PRODUCER_NAME, PublishError, SOCIAL_PLATFORM, TransportError,
    source_identity,
};
pub use resolution::{
    NormalizeError, NormalizedMedia, OEMBED_PARSER_VERSION, PublicSurface, ResolutionError,
    ResolutionOutcome, StoredResolution, SurfaceOutcome, kind_media_type, normalize,
};
pub use telemetry::{TelemetryError, TelemetryGuard, init_telemetry};
pub use tombstone::{TombstoneError, TombstoneOutcome};

#[cfg(feature = "test-support")]
pub mod test_support;
