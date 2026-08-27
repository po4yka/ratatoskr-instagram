#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Domain library for the Ratatoskr Instagram bounded context.
//!
//! The foundation owns process configuration, telemetry bootstrap, and
//! application of the first-version `instagram_archive` schema. Explicit
//! capture intake and public resolution through the approved surface are
//! implemented; account connection and Data Export imports arrive with later
//! implementation plan items.

/// Knowledge completion intake and local capture-result linkage.
pub mod analysis_linkage;
/// The capability matrix: acquisition modes, support status, authority
/// ceilings, and the upstream-versus-preservation boundary.
pub mod capability;
/// Explicit capture intake: submission identity, provenance, and the
/// unavailable fallback.
pub mod capture;
/// Provider-specific browser-command validation and capture handoff.
pub mod command_capture;
pub mod config;
/// The owned `PostgreSQL` pool and the embedded `instagram_archive` schema.
pub mod database;
/// Canonicalization of client-delivered Instagram URLs into stable permalinks.
pub mod permalink;
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
pub use command_capture::{
    BrowserCaptureCommand, BrowserCaptureIngested, CommandCaptureError,
    decode_browser_capture_command,
};
pub use config::{
    AdminConfig, BusConfig, Config, ConfigError, Limits, PublisherConfig, StorageConfig,
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
