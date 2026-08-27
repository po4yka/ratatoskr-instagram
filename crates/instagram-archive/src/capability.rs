//! Provenance semantics: acquisition modes, their authority ceilings, support
//! status, and the boundary between upstream availability and local preservation.
//!
//! SKELETON (task 1.1): the lookup functions below return placeholder values on
//! purpose, so the scenario tests in `tests/capability.rs` fail on their
//! assertions rather than on a compile error. Task 2.1 replaces every
//! placeholder with the documented constants.

/// One way a source can enter this bounded context.
///
/// The inventory is closed: five modes, one per ingestion lane the service may
/// ever operate, with `LegacyImport` carrying monolith migration. Adding a mode
/// means adding a lane and an alignment entry, never silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AcquisitionMode {
    /// The user pushed an Instagram URL into Ratatoskr through a share target
    /// or browser extension.
    ExplicitCapture,
    /// Content was resolved through the supported public metadata surface.
    PublicResolution,
    /// Own-account media was read through the official authenticated API.
    OwnAccountSync,
    /// Records were parsed out of a provider Data Export the user supplied.
    DataExport,
    /// Records were carried over from the retired monolith.
    LegacyImport,
}

impl AcquisitionMode {
    /// Every acquisition mode, exactly the declared inventory.
    pub const ALL: [AcquisitionMode; 5] = [
        AcquisitionMode::ExplicitCapture,
        AcquisitionMode::PublicResolution,
        AcquisitionMode::OwnAccountSync,
        AcquisitionMode::DataExport,
        AcquisitionMode::LegacyImport,
    ];

    /// The capability matrix answer for this mode: the mode's support status
    /// and its authority ceiling. A lane reports `Planned` until the plan item
    /// implementing it flips its status with a reviewed test change;
    /// `ExplicitCapture`, `PublicResolution`, and `OwnAccountSync` are
    /// supported by their implemented ingestion lanes.
    #[must_use]
    pub fn capability(self) -> ModeCapability {
        let authority_ceiling = match self {
            Self::ExplicitCapture | Self::PublicResolution => SavedAuthority::ExplicitUserCapture,
            Self::OwnAccountSync => SavedAuthority::AuthoritativePlatformState,
            Self::DataExport => SavedAuthority::ExportObservation,
            Self::LegacyImport => SavedAuthority::LegacyObservation,
        };
        let status = match self {
            Self::ExplicitCapture
            | Self::PublicResolution
            | Self::OwnAccountSync
            | Self::DataExport => SupportStatus::Supported,
            Self::LegacyImport => SupportStatus::Planned,
        };
        ModeCapability {
            mode: self,
            status,
            authority_ceiling,
        }
    }
}

/// Whether a capability can be exercised today.
///
/// `Planned` names a lane the repository intends to build; flipping a status to
/// `Supported` is a deliberate, tested change made by the plan item that lands
/// the implementation. `NotSupported` states a provider limitation honestly
/// instead of leaving it silent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SupportStatus {
    /// Implemented and exercisable in this service today.
    Supported,
    /// Planned by an open implementation item; not exercisable yet.
    Planned,
    /// No supported provider surface exists; the service will not pretend.
    NotSupported,
}

/// What a saved-state claim is worth.
///
/// Mirrors the `SavedAuthority` vocabulary of `ratatoskr-social-contracts`
/// (`crates/social-contracts/src/vocabulary.rs`, revision `361fe94`); the
/// alignment test pins the two sets together. An explicit capture proves the
/// user saved an item to Ratatoskr, never membership in the provider's native
/// Saved list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SavedAuthority {
    /// The platform itself exposes this saved state through a supported API.
    AuthoritativePlatformState,
    /// A user action inside Ratatoskr captured the source; provider state unknown.
    ExplicitUserCapture,
    /// A provider export shows the item was saved at some point, without live authority.
    ExportObservation,
    /// Migrated from the retired monolith; worth what that record was worth.
    LegacyObservation,
}

/// The capability matrix row for one acquisition mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModeCapability {
    /// The mode this row describes.
    pub mode: AcquisitionMode,
    /// Whether the lane is exercisable today.
    pub status: SupportStatus,
    /// The strongest saved-authority claim this mode may ever make.
    pub authority_ceiling: SavedAuthority,
}

impl ModeCapability {
    /// The closed wire vocabulary this mode produces, as stored provenance
    /// values shared with the social-contract grammar. Each contract
    /// acquisition method belongs to exactly one mode.
    #[must_use]
    pub fn wire_methods(&self) -> &'static [&'static str] {
        match self.mode {
            AcquisitionMode::ExplicitCapture => &["share_extension", "browser_extension"],
            AcquisitionMode::PublicResolution => &["public_resolution"],
            AcquisitionMode::OwnAccountSync => &["official_api"],
            AcquisitionMode::DataExport => &["data_export"],
            AcquisitionMode::LegacyImport => &["legacy_import"],
        }
    }
}

/// The matrix answer for native Saved-list synchronization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeSavedSupport {
    /// Always [`SupportStatus::NotSupported`] while no supported surface exists.
    pub status: SupportStatus,
    /// Why the answer is what it is, written for operators and reviews.
    pub reason: &'static str,
}

/// Native Saved-list synchronization of a personal account.
///
/// Instagram exposes no supported API surface that reads a personal account's
/// native Saved list, so this service states the limitation instead of
/// approximating it from captures or exports.
pub const NATIVE_SAVED_LIST_SYNC: NativeSavedSupport = NativeSavedSupport {
    status: SupportStatus::NotSupported,
    reason: "no supported provider surface exposes the personal Saved list",
};

/// What Instagram last reported about a source's existence.
///
/// Kept strictly apart from [`PreservationState`]: this column records the
/// platform's side, never what Ratatoskr holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpstreamStatus {
    /// The source resolved normally when last observed.
    Available,
    /// The source failed to resolve; deletion is not proven.
    Unavailable,
    /// The provider stated or implied the source no longer exists.
    Deleted,
    /// The source exists but denies anonymous access.
    Private,
    /// No usable observation exists yet.
    Unknown,
}

/// One availability observation, at the resolution the resolver reported it.
///
/// Mirrors the `availability_observations.availability` CHECK vocabulary: finer
/// than [`UpstreamStatus`] because the distinction matters for retry policy and
/// honesty, collapsible into it for the media row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AvailabilityObservationKind {
    /// Resolved normally.
    Available,
    /// Failed to resolve without a proven cause.
    Unavailable,
    /// Provider stated or implied the source is gone.
    Deleted,
    /// Exists but denies anonymous access.
    Private,
    /// Failed transiently; retrying later may succeed.
    TemporarilyUnavailable,
    /// URL shape this resolver does not handle.
    Unsupported,
    /// Resolution attempted and failed before classification was possible.
    ResolutionFailed,
}

impl AvailabilityObservationKind {
    /// Every observation kind, mirroring the schema CHECK exactly.
    pub const ALL: [AvailabilityObservationKind; 7] = [
        AvailabilityObservationKind::Available,
        AvailabilityObservationKind::Unavailable,
        AvailabilityObservationKind::Deleted,
        AvailabilityObservationKind::Private,
        AvailabilityObservationKind::TemporarilyUnavailable,
        AvailabilityObservationKind::Unsupported,
        AvailabilityObservationKind::ResolutionFailed,
    ];

    /// The `snake_case` wire value stored in
    /// `availability_observations.availability`, equal to the schema CHECK
    /// vocabulary value for value.
    #[must_use]
    pub const fn wire_value(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Unavailable => "unavailable",
            Self::Deleted => "deleted",
            Self::Private => "private",
            Self::TemporarilyUnavailable => "temporarily_unavailable",
            Self::Unsupported => "unsupported",
            Self::ResolutionFailed => "resolution_failed",
        }
    }

    /// Collapse an observation into the media-row upstream status.
    ///
    /// Private and transient failures collapse to `unavailable` — being denied
    /// access or rate-limited is evidence about access, never about existence —
    /// while unsupported URL shapes and failed resolutions, which learned
    /// nothing, collapse to `unknown`.
    #[must_use]
    pub fn collapse_to_media_status(self) -> UpstreamStatus {
        match self {
            Self::Available => UpstreamStatus::Available,
            Self::Unavailable | Self::TemporarilyUnavailable | Self::Private => {
                UpstreamStatus::Unavailable
            }
            Self::Deleted => UpstreamStatus::Deleted,
            Self::Unsupported | Self::ResolutionFailed => UpstreamStatus::Unknown,
        }
    }
}

/// What Ratatoskr holds locally for a source.
///
/// Independent of [`UpstreamStatus`] by construction: observing deletion
/// upstream never demotes content already preserved, and nothing here implies
/// anything about what the platform currently serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PreservationState {
    /// Media bytes and metadata preserved locally.
    ContentPreserved,
    /// Metadata and raw evidence preserved; media bytes not archived.
    MetadataOnly,
    /// Only a user-uploaded artifact is held, with its own provenance.
    UserArtifactOnly,
    /// Nothing beyond the capture record itself.
    NothingPreserved,
}

impl PreservationState {
    /// Every preservation state.
    pub const ALL: [PreservationState; 4] = [
        PreservationState::ContentPreserved,
        PreservationState::MetadataOnly,
        PreservationState::UserArtifactOnly,
        PreservationState::NothingPreserved,
    ];
}

/// Apply an availability observation to a preservation state.
///
/// The rule is identity on purpose: observations describe what the platform
/// reported, and no observation — including a proven deletion — is evidence
/// about what Ratatoskr preserved. Demotion happens only through explicit user
/// action, so absence in a later export or a failed resolution can never delete
/// an archived capture (`AGENTS.md`, "absence-in-export never causing
/// deletion").
#[must_use]
pub fn retention_after_observation(
    current: PreservationState,
    observed: AvailabilityObservationKind,
) -> PreservationState {
    let _ = observed;
    current
}

#[cfg(test)]
mod data_export_tests {
    use super::*;

    #[test]
    fn data_export_is_supported_after_item_eight() {
        let capability = AcquisitionMode::DataExport.capability();
        assert_eq!(capability.status, SupportStatus::Supported);
        assert_eq!(
            capability.authority_ceiling,
            SavedAuthority::ExportObservation
        );
        assert_eq!(capability.wire_methods(), ["data_export"]);
        assert_eq!(
            AcquisitionMode::LegacyImport.capability().status,
            SupportStatus::Planned
        );
        assert_eq!(NATIVE_SAVED_LIST_SYNC.status, SupportStatus::NotSupported);
    }
}
