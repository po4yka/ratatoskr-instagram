use super::{DataClassDisposition, DeletionAction, OwnedDataClass};

const fn disposition(class: OwnedDataClass, action: DeletionAction) -> DataClassDisposition {
    DataClassDisposition { class, action }
}

/// Exact first-version storage inventory, including protected `BlobStore` classes.
pub const OWNED_DATA_CLASSES: &[OwnedDataClass] = &[
    OwnedDataClass::Accounts,
    OwnedDataClass::AccountCapabilities,
    OwnedDataClass::AccountCredentialAudit,
    OwnedDataClass::AccountPermissionObservations,
    OwnedDataClass::Credentials,
    OwnedDataClass::OAuthFlows,
    OwnedDataClass::ProviderApiUsage,
    OwnedDataClass::Profiles,
    OwnedDataClass::Media,
    OwnedDataClass::MediaRelations,
    OwnedDataClass::MediaRevisions,
    OwnedDataClass::OwnMediaSyncState,
    OwnedDataClass::OwnMediaSyncRuns,
    OwnedDataClass::OwnMediaSyncItems,
    OwnedDataClass::OwnMediaAuthority,
    OwnedDataClass::Captures,
    OwnedDataClass::CaptureAnalysisLinks,
    OwnedDataClass::CaptureNotes,
    OwnedDataClass::ExportSnapshots,
    OwnedDataClass::ImportRuns,
    OwnedDataClass::ImportRunTransitions,
    OwnedDataClass::ExportRecords,
    OwnedDataClass::ExportCompletenessReports,
    OwnedDataClass::RawRecords,
    OwnedDataClass::AvailabilityObservations,
    OwnedDataClass::OutboxEvents,
    OwnedDataClass::InboxEvents,
    OwnedDataClass::DeletionOperations,
    OwnedDataClass::DeletionEffects,
    OwnedDataClass::LocalSourceRemovals,
    OwnedDataClass::BlobDeletionTasks,
    OwnedDataClass::ReresolutionRuns,
    OwnedDataClass::ReresolutionItems,
    OwnedDataClass::ExportReprocessingRuns,
    OwnedDataClass::ExportReprocessingItems,
    OwnedDataClass::DataExportArchiveBlob,
    OwnedDataClass::ProviderMediaBlob,
    OwnedDataClass::RawResponseBlob,
    OwnedDataClass::UserUploadBlob,
];

/// Capture-target classification for every owned row and blob class.
pub const CAPTURE_DELETION_CLASSIFICATIONS: &[DataClassDisposition] = &[
    disposition(OwnedDataClass::Accounts, DeletionAction::NotApplicable),
    disposition(
        OwnedDataClass::AccountCapabilities,
        DeletionAction::NotApplicable,
    ),
    disposition(
        OwnedDataClass::AccountCredentialAudit,
        DeletionAction::NotApplicable,
    ),
    disposition(
        OwnedDataClass::AccountPermissionObservations,
        DeletionAction::NotApplicable,
    ),
    disposition(OwnedDataClass::Credentials, DeletionAction::NotApplicable),
    disposition(OwnedDataClass::OAuthFlows, DeletionAction::NotApplicable),
    disposition(
        OwnedDataClass::ProviderApiUsage,
        DeletionAction::NotApplicable,
    ),
    disposition(OwnedDataClass::Profiles, DeletionAction::NotApplicable),
    disposition(OwnedDataClass::Media, DeletionAction::Detach),
    disposition(OwnedDataClass::MediaRelations, DeletionAction::Detach),
    disposition(OwnedDataClass::MediaRevisions, DeletionAction::Detach),
    disposition(
        OwnedDataClass::OwnMediaSyncState,
        DeletionAction::NotApplicable,
    ),
    disposition(
        OwnedDataClass::OwnMediaSyncRuns,
        DeletionAction::NotApplicable,
    ),
    disposition(
        OwnedDataClass::OwnMediaSyncItems,
        DeletionAction::NotApplicable,
    ),
    disposition(
        OwnedDataClass::OwnMediaAuthority,
        DeletionAction::NotApplicable,
    ),
    disposition(OwnedDataClass::Captures, DeletionAction::Delete),
    disposition(OwnedDataClass::CaptureAnalysisLinks, DeletionAction::Delete),
    disposition(OwnedDataClass::CaptureNotes, DeletionAction::Delete),
    disposition(
        OwnedDataClass::ExportSnapshots,
        DeletionAction::NotApplicable,
    ),
    disposition(OwnedDataClass::ImportRuns, DeletionAction::NotApplicable),
    disposition(
        OwnedDataClass::ImportRunTransitions,
        DeletionAction::NotApplicable,
    ),
    disposition(OwnedDataClass::ExportRecords, DeletionAction::NotApplicable),
    disposition(
        OwnedDataClass::ExportCompletenessReports,
        DeletionAction::NotApplicable,
    ),
    disposition(OwnedDataClass::RawRecords, DeletionAction::Detach),
    disposition(
        OwnedDataClass::AvailabilityObservations,
        DeletionAction::Delete,
    ),
    disposition(OwnedDataClass::OutboxEvents, DeletionAction::RetainAudit),
    disposition(OwnedDataClass::InboxEvents, DeletionAction::RetainAudit),
    disposition(
        OwnedDataClass::DeletionOperations,
        DeletionAction::RetainAudit,
    ),
    disposition(OwnedDataClass::DeletionEffects, DeletionAction::RetainAudit),
    disposition(
        OwnedDataClass::LocalSourceRemovals,
        DeletionAction::RetainAudit,
    ),
    disposition(
        OwnedDataClass::BlobDeletionTasks,
        DeletionAction::RetainAudit,
    ),
    disposition(
        OwnedDataClass::ReresolutionRuns,
        DeletionAction::RetainAudit,
    ),
    disposition(OwnedDataClass::ReresolutionItems, DeletionAction::Delete),
    disposition(
        OwnedDataClass::ExportReprocessingRuns,
        DeletionAction::NotApplicable,
    ),
    disposition(
        OwnedDataClass::ExportReprocessingItems,
        DeletionAction::NotApplicable,
    ),
    disposition(
        OwnedDataClass::DataExportArchiveBlob,
        DeletionAction::NotApplicable,
    ),
    disposition(OwnedDataClass::ProviderMediaBlob, DeletionAction::Detach),
    disposition(OwnedDataClass::RawResponseBlob, DeletionAction::Detach),
    disposition(OwnedDataClass::UserUploadBlob, DeletionAction::Delete),
];

/// Official-account-connection classification for every owned row and blob class.
pub const CONNECTION_DELETION_CLASSIFICATIONS: &[DataClassDisposition] = &[
    disposition(OwnedDataClass::Accounts, DeletionAction::Delete),
    disposition(OwnedDataClass::AccountCapabilities, DeletionAction::Delete),
    disposition(
        OwnedDataClass::AccountCredentialAudit,
        DeletionAction::Delete,
    ),
    disposition(
        OwnedDataClass::AccountPermissionObservations,
        DeletionAction::Delete,
    ),
    disposition(OwnedDataClass::Credentials, DeletionAction::Delete),
    disposition(OwnedDataClass::OAuthFlows, DeletionAction::Delete),
    disposition(OwnedDataClass::ProviderApiUsage, DeletionAction::Delete),
    disposition(OwnedDataClass::Profiles, DeletionAction::Delete),
    disposition(OwnedDataClass::Media, DeletionAction::Detach),
    disposition(OwnedDataClass::MediaRelations, DeletionAction::Detach),
    disposition(OwnedDataClass::MediaRevisions, DeletionAction::Detach),
    disposition(OwnedDataClass::OwnMediaSyncState, DeletionAction::Delete),
    disposition(OwnedDataClass::OwnMediaSyncRuns, DeletionAction::Delete),
    disposition(OwnedDataClass::OwnMediaSyncItems, DeletionAction::Delete),
    disposition(OwnedDataClass::OwnMediaAuthority, DeletionAction::Delete),
    disposition(OwnedDataClass::Captures, DeletionAction::NotApplicable),
    disposition(
        OwnedDataClass::CaptureAnalysisLinks,
        DeletionAction::NotApplicable,
    ),
    disposition(OwnedDataClass::CaptureNotes, DeletionAction::NotApplicable),
    disposition(
        OwnedDataClass::ExportSnapshots,
        DeletionAction::NotApplicable,
    ),
    disposition(OwnedDataClass::ImportRuns, DeletionAction::NotApplicable),
    disposition(
        OwnedDataClass::ImportRunTransitions,
        DeletionAction::NotApplicable,
    ),
    disposition(OwnedDataClass::ExportRecords, DeletionAction::NotApplicable),
    disposition(
        OwnedDataClass::ExportCompletenessReports,
        DeletionAction::NotApplicable,
    ),
    disposition(OwnedDataClass::RawRecords, DeletionAction::Detach),
    disposition(
        OwnedDataClass::AvailabilityObservations,
        DeletionAction::Detach,
    ),
    disposition(OwnedDataClass::OutboxEvents, DeletionAction::Delete),
    disposition(OwnedDataClass::InboxEvents, DeletionAction::RetainAudit),
    disposition(
        OwnedDataClass::DeletionOperations,
        DeletionAction::RetainAudit,
    ),
    disposition(OwnedDataClass::DeletionEffects, DeletionAction::RetainAudit),
    disposition(
        OwnedDataClass::LocalSourceRemovals,
        DeletionAction::RetainAudit,
    ),
    disposition(
        OwnedDataClass::BlobDeletionTasks,
        DeletionAction::RetainAudit,
    ),
    disposition(
        OwnedDataClass::ReresolutionRuns,
        DeletionAction::NotApplicable,
    ),
    disposition(
        OwnedDataClass::ReresolutionItems,
        DeletionAction::NotApplicable,
    ),
    disposition(
        OwnedDataClass::ExportReprocessingRuns,
        DeletionAction::NotApplicable,
    ),
    disposition(
        OwnedDataClass::ExportReprocessingItems,
        DeletionAction::NotApplicable,
    ),
    disposition(
        OwnedDataClass::DataExportArchiveBlob,
        DeletionAction::NotApplicable,
    ),
    disposition(OwnedDataClass::ProviderMediaBlob, DeletionAction::Detach),
    disposition(OwnedDataClass::RawResponseBlob, DeletionAction::Detach),
    disposition(
        OwnedDataClass::UserUploadBlob,
        DeletionAction::NotApplicable,
    ),
];
