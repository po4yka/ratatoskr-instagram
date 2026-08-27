//! Capability-model contract: what the matrix answers for each acquisition
//! mode, that authority ceilings hold, that local constants match the published
//! social-contract grammar value for value, and that upstream availability
//! never touches local preservation.
//!
//! The contract vocabularies pinned here are copied from
//! `ratatoskr-contracts` `crates/social-contracts/src/vocabulary.rs` at
//! revision `361fe94` (2026-08-25), recorded in `docs/CAPABILITY_MATRIX.md`.

use ratatoskr_instagram_archive::capability::{
    AcquisitionMode, AvailabilityObservationKind, NATIVE_SAVED_LIST_SYNC, PreservationState,
    SavedAuthority, SupportStatus, UpstreamStatus, retention_after_observation,
};

/// The `AcquisitionMethod` vocabulary of `ratatoskr-social-contracts@361fe94`.
const CONTRACT_ACQUISITION_METHODS: [&str; 6] = [
    "official_api",
    "share_extension",
    "browser_extension",
    "public_resolution",
    "data_export",
    "legacy_import",
];

/// The `SavedAuthority` vocabulary of `ratatoskr-social-contracts@361fe94`.
const CONTRACT_SAVED_AUTHORITIES: [&str; 4] = [
    "explicit_user_capture",
    "export_observation",
    "authoritative_platform_state",
    "legacy_observation",
];

/// The documented matrix row for one mode: wire methods plus authority ceiling.
#[derive(Debug)]
struct Expected {
    mode: AcquisitionMode,
    wire_methods: &'static [&'static str],
    ceiling: SavedAuthority,
}

fn documented_matrix() -> [Expected; 5] {
    use AcquisitionMode::{
        DataExport, ExplicitCapture, LegacyImport, OwnAccountSync, PublicResolution,
    };
    use SavedAuthority::{
        AuthoritativePlatformState as Authoritative, ExplicitUserCapture as Explicit,
        ExportObservation as Export, LegacyObservation as Legacy,
    };
    [
        Expected {
            mode: ExplicitCapture,
            wire_methods: &["share_extension", "browser_extension"],
            ceiling: Explicit,
        },
        Expected {
            mode: PublicResolution,
            wire_methods: &["public_resolution"],
            ceiling: Explicit,
        },
        Expected {
            mode: OwnAccountSync,
            wire_methods: &["official_api"],
            ceiling: Authoritative,
        },
        Expected {
            mode: DataExport,
            wire_methods: &["data_export"],
            ceiling: Export,
        },
        Expected {
            mode: LegacyImport,
            wire_methods: &["legacy_import"],
            ceiling: Legacy,
        },
    ]
}

#[test]
fn each_mode_resolves_to_its_documented_capability() {
    let mut seen: Vec<AcquisitionMode> = Vec::new();
    for expected in documented_matrix() {
        let capability = expected.mode.capability();
        assert_eq!(capability.authority_ceiling, expected.ceiling);

        let mut produced = capability.wire_methods().to_vec();
        produced.sort_unstable();
        let mut documented = expected.wire_methods.to_vec();
        documented.sort_unstable();
        assert_eq!(
            produced, documented,
            "wire vocabulary mismatch for {expected:?}"
        );

        seen.push(expected.mode);
    }
    let mut inventory = AcquisitionMode::ALL.to_vec();
    inventory.sort_unstable();
    seen.sort_unstable();
    assert_eq!(
        inventory, seen,
        "the mode inventory is exactly the five documented modes"
    );
}

#[test]
fn no_mode_reports_supported_while_its_lane_is_unimplemented() {
    for mode in AcquisitionMode::ALL {
        if mode == AcquisitionMode::ExplicitCapture {
            continue; // implemented by plan item 3; see the dedicated test below.
        }
        if mode == AcquisitionMode::PublicResolution {
            continue; // implemented by plan item 4; see the dedicated test below.
        }
        if mode == AcquisitionMode::OwnAccountSync {
            continue; // implemented by plan item 7; see account_capabilities.rs.
        }
        if mode == AcquisitionMode::DataExport {
            continue; // implemented by plan item 8; see capability.rs and data_export.rs.
        }
        let status = mode.capability().status;
        assert_ne!(
            status,
            SupportStatus::Supported,
            "{mode:?} must not claim support before its plan item lands"
        );
        assert_eq!(status, SupportStatus::Planned);
    }
}

#[test]
fn implemented_explicit_capture_lane_reports_support_with_unchanged_terms() {
    let capability = AcquisitionMode::ExplicitCapture.capability();
    assert_eq!(
        capability.status,
        SupportStatus::Supported,
        "the explicit-capture lane is exercisable once its plan item lands"
    );
    assert_eq!(
        capability.wire_methods(),
        &["share_extension", "browser_extension"],
        "support changes no documented wire vocabulary"
    );
    assert_eq!(
        capability.authority_ceiling,
        SavedAuthority::ExplicitUserCapture,
        "support never raises the authority ceiling"
    );
}

#[test]
fn implemented_public_resolution_lane_reports_support_with_unchanged_terms() {
    let capability = AcquisitionMode::PublicResolution.capability();
    assert_eq!(
        capability.status,
        SupportStatus::Supported,
        "the public-resolution lane is exercisable once its plan item lands"
    );
    assert_eq!(
        capability.wire_methods(),
        &["public_resolution"],
        "support changes no documented wire vocabulary"
    );
    assert_eq!(
        capability.authority_ceiling,
        SavedAuthority::ExplicitUserCapture,
        "resolving upstream content never raises the authority ceiling"
    );
}

#[test]
fn native_saved_list_synchronization_reports_not_supported_with_reason() {
    assert_eq!(
        NATIVE_SAVED_LIST_SYNC.status,
        SupportStatus::NotSupported,
        "no supported provider surface exposes the personal Saved list"
    );
    assert!(
        NATIVE_SAVED_LIST_SYNC.reason.contains("no supported"),
        "the reason must state the provider limitation: {:?}",
        NATIVE_SAVED_LIST_SYNC.reason
    );
}

#[test]
fn only_own_account_sync_reaches_authoritative_platform_state() {
    for expected in documented_matrix() {
        let ceiling = expected.mode.capability().authority_ceiling;
        if expected.mode == AcquisitionMode::OwnAccountSync {
            assert_eq!(ceiling, SavedAuthority::AuthoritativePlatformState);
        } else {
            assert_ne!(
                ceiling,
                SavedAuthority::AuthoritativePlatformState,
                "{expected:?} must never reach authoritative platform state"
            );
        }
    }
}

#[test]
fn local_method_and_authority_sets_equal_the_recorded_contract_sets() {
    let mut owned: Vec<(&str, usize)> = Vec::new();
    for mode in AcquisitionMode::ALL {
        for method in mode.capability().wire_methods() {
            match owned.iter_mut().find(|(name, _)| *name == *method) {
                Some((_, count)) => *count += 1,
                None => owned.push((method, 1)),
            }
        }
    }

    let mut local = owned.iter().map(|(name, _)| *name).collect::<Vec<_>>();
    local.sort_unstable();
    let mut contract = CONTRACT_ACQUISITION_METHODS.to_vec();
    contract.sort_unstable();
    assert_eq!(
        local, contract,
        "every contract acquisition method must exist locally exactly once"
    );
    for (name, count) in owned {
        assert_eq!(
            count, 1,
            "{name} must be produced by exactly one acquisition mode"
        );
    }

    let mut authorities = AcquisitionMode::ALL
        .iter()
        .map(|mode| authority_wire_value(mode.capability().authority_ceiling))
        .collect::<Vec<_>>();
    authorities.sort_unstable();
    authorities.dedup();
    let mut contract_authorities = CONTRACT_SAVED_AUTHORITIES.to_vec();
    contract_authorities.sort_unstable();
    assert_eq!(
        authorities, contract_authorities,
        "the reachable authority set must equal the contract vocabulary"
    );
}

/// The `snake_case` wire value of an authority, as shared with the schema CHECKs
/// and the contract serde representation.
fn authority_wire_value(authority: SavedAuthority) -> &'static str {
    match authority {
        SavedAuthority::AuthoritativePlatformState => "authoritative_platform_state",
        SavedAuthority::ExplicitUserCapture => "explicit_user_capture",
        SavedAuthority::ExportObservation => "export_observation",
        SavedAuthority::LegacyObservation => "legacy_observation",
    }
}

#[test]
fn collapsing_observations_keeps_private_away_from_deleted() {
    use AvailabilityObservationKind as Kind;
    use UpstreamStatus as Status;

    let mapping = [
        (Kind::Available, Status::Available),
        (Kind::Unavailable, Status::Unavailable),
        (Kind::Deleted, Status::Deleted),
        (Kind::Private, Status::Unavailable),
        (Kind::TemporarilyUnavailable, Status::Unavailable),
        (Kind::Unsupported, Status::Unknown),
        (Kind::ResolutionFailed, Status::Unknown),
    ];
    for (kind, expected) in mapping {
        let collapsed = kind.collapse_to_media_status();
        assert_eq!(
            collapsed, expected,
            "collapse of {kind:?} must follow the documented mapping"
        );
        if kind != AvailabilityObservationKind::Deleted {
            assert_ne!(
                collapsed,
                UpstreamStatus::Deleted,
                "{kind:?} must never collapse to deleted: deletion was not proven"
            );
        }
    }
}

#[test]
fn applying_any_observation_changes_no_preservation_state() {
    for current in PreservationState::ALL {
        for observed in AvailabilityObservationKind::ALL {
            assert_eq!(
                retention_after_observation(current, observed),
                current,
                "observing {observed:?} must not change {current:?}"
            );
        }
    }
}
