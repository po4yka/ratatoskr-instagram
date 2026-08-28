//! Closed inventory for owner-authorized privacy deletion.

mod apply;
mod inventory;

use uuid::Uuid;

use crate::publishing::{
    append_removal_fact, append_source_removal_fact, instant_from_time, own_media_source_identity,
    source_identity,
};
use crate::{Database, PersistenceError};

pub use inventory::{
    CAPTURE_DELETION_CLASSIFICATIONS, CONNECTION_DELETION_CLASSIFICATIONS, OWNED_DATA_CLASSES,
};

/// One Instagram-owned database table or protected `BlobStore` class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OwnedDataClass {
    /// `instagram_archive.accounts`.
    Accounts,
    /// `instagram_archive.account_capabilities`.
    AccountCapabilities,
    /// `instagram_archive.account_credential_audit`.
    AccountCredentialAudit,
    /// `instagram_archive.account_permission_observations`.
    AccountPermissionObservations,
    /// `instagram_archive.credentials`.
    Credentials,
    /// `instagram_archive.oauth_flows`.
    OAuthFlows,
    /// `instagram_archive.provider_api_usage`.
    ProviderApiUsage,
    /// `instagram_archive.profiles`.
    Profiles,
    /// `instagram_archive.media`.
    Media,
    /// `instagram_archive.media_relations`.
    MediaRelations,
    /// `instagram_archive.media_revisions`.
    MediaRevisions,
    /// `instagram_archive.own_media_sync_state`.
    OwnMediaSyncState,
    /// `instagram_archive.own_media_sync_runs`.
    OwnMediaSyncRuns,
    /// `instagram_archive.own_media_sync_items`.
    OwnMediaSyncItems,
    /// `instagram_archive.own_media_authority`.
    OwnMediaAuthority,
    /// `instagram_archive.captures`.
    Captures,
    /// `instagram_archive.capture_analysis_links`.
    CaptureAnalysisLinks,
    /// `instagram_archive.capture_notes`.
    CaptureNotes,
    /// `instagram_archive.export_snapshots`.
    ExportSnapshots,
    /// `instagram_archive.import_runs`.
    ImportRuns,
    /// `instagram_archive.import_run_transitions`.
    ImportRunTransitions,
    /// `instagram_archive.export_records`.
    ExportRecords,
    /// `instagram_archive.export_completeness_reports`.
    ExportCompletenessReports,
    /// `instagram_archive.raw_records`.
    RawRecords,
    /// `instagram_archive.availability_observations`.
    AvailabilityObservations,
    /// `instagram_archive.outbox_events`.
    OutboxEvents,
    /// `instagram_archive.inbox_events`.
    InboxEvents,
    /// `instagram_archive.deletion_operations`.
    DeletionOperations,
    /// `instagram_archive.deletion_effects`.
    DeletionEffects,
    /// `instagram_archive.local_source_removals`.
    LocalSourceRemovals,
    /// `instagram_archive.blob_deletion_tasks`.
    BlobDeletionTasks,
    /// `instagram_archive.reresolution_runs`.
    ReresolutionRuns,
    /// `instagram_archive.reresolution_items`.
    ReresolutionItems,
    /// `instagram_archive.export_reprocessing_runs`.
    ExportReprocessingRuns,
    /// `instagram_archive.export_reprocessing_items`.
    ExportReprocessingItems,
    /// An immutable Data Export archive object.
    DataExportArchiveBlob,
    /// Provider media bytes admitted by retention policy.
    ProviderMediaBlob,
    /// A retained raw provider response object.
    RawResponseBlob,
    /// A separately authorized user-upload object.
    UserUploadBlob,
}

impl OwnedDataClass {
    /// Returns the stable inventory key used by deletion reports and tests.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Accounts => "table:accounts",
            Self::AccountCapabilities => "table:account_capabilities",
            Self::AccountCredentialAudit => "table:account_credential_audit",
            Self::AccountPermissionObservations => "table:account_permission_observations",
            Self::Credentials => "table:credentials",
            Self::OAuthFlows => "table:oauth_flows",
            Self::ProviderApiUsage => "table:provider_api_usage",
            Self::Profiles => "table:profiles",
            Self::Media => "table:media",
            Self::MediaRelations => "table:media_relations",
            Self::MediaRevisions => "table:media_revisions",
            Self::OwnMediaSyncState => "table:own_media_sync_state",
            Self::OwnMediaSyncRuns => "table:own_media_sync_runs",
            Self::OwnMediaSyncItems => "table:own_media_sync_items",
            Self::OwnMediaAuthority => "table:own_media_authority",
            Self::Captures => "table:captures",
            Self::CaptureAnalysisLinks => "table:capture_analysis_links",
            Self::CaptureNotes => "table:capture_notes",
            Self::ExportSnapshots => "table:export_snapshots",
            Self::ImportRuns => "table:import_runs",
            Self::ImportRunTransitions => "table:import_run_transitions",
            Self::ExportRecords => "table:export_records",
            Self::ExportCompletenessReports => "table:export_completeness_reports",
            Self::RawRecords => "table:raw_records",
            Self::AvailabilityObservations => "table:availability_observations",
            Self::OutboxEvents => "table:outbox_events",
            Self::InboxEvents => "table:inbox_events",
            Self::DeletionOperations => "table:deletion_operations",
            Self::DeletionEffects => "table:deletion_effects",
            Self::LocalSourceRemovals => "table:local_source_removals",
            Self::BlobDeletionTasks => "table:blob_deletion_tasks",
            Self::ReresolutionRuns => "table:reresolution_runs",
            Self::ReresolutionItems => "table:reresolution_items",
            Self::ExportReprocessingRuns => "table:export_reprocessing_runs",
            Self::ExportReprocessingItems => "table:export_reprocessing_items",
            Self::DataExportArchiveBlob => "blob:data_export_archive",
            Self::ProviderMediaBlob => "blob:provider_media",
            Self::RawResponseBlob => "blob:raw_response",
            Self::UserUploadBlob => "blob:user_upload",
        }
    }

    fn audit_key(self) -> &'static str {
        match self {
            Self::DataExportArchiveBlob => "data_export_archive_blob",
            Self::ProviderMediaBlob => "provider_media_blob",
            Self::RawResponseBlob => "raw_response_blob",
            Self::UserUploadBlob => "user_upload_blob",
            _ => match self.key().strip_prefix("table:") {
                Some(key) => key,
                None => "invalid_class",
            },
        }
    }

    fn from_audit_key(value: &str) -> Option<Self> {
        OWNED_DATA_CLASSES
            .iter()
            .copied()
            .find(|class| class.audit_key() == value)
    }
}

/// The effect one target-specific deletion plan can have on one owned class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionAction {
    /// Physically erase target-owned content or credentials.
    Delete,
    /// Remove only the target-specific reference.
    Detach,
    /// Keep bounded, content-free audit or delivery evidence.
    RetainAudit,
    /// Keep storage required by another authorized holding.
    RetainShared,
    /// The class cannot contain data for this target kind.
    NotApplicable,
}

impl DeletionAction {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Delete => "delete",
            Self::Detach => "detach",
            Self::RetainAudit => "retain_audit",
            Self::RetainShared => "retain_shared",
            Self::NotApplicable => "not_applicable",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "delete" => Some(Self::Delete),
            "detach" => Some(Self::Detach),
            "retain_audit" => Some(Self::RetainAudit),
            "retain_shared" => Some(Self::RetainShared),
            "not_applicable" => Some(Self::NotApplicable),
            _ => None,
        }
    }
}

/// One unambiguous target-specific classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataClassDisposition {
    /// The owned row/blob class.
    pub class: OwnedDataClass,
    /// Its target-specific deletion effect.
    pub action: DeletionAction,
}

/// One owner-scoped local deletion target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionTarget {
    /// One explicit capture and its capture-specific intent.
    Capture(Uuid),
    /// One official Instagram account connection.
    Connection(Uuid),
}

/// Stable replay identity and owner authorization for a deletion request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeletionRequest {
    /// Caller-supplied stable idempotency identity.
    pub operation_id: Uuid,
    /// Internal owner identity authenticated by the caller.
    pub user_ref: Uuid,
    /// Capture or official connection to remove locally.
    pub target: DeletionTarget,
}

/// Terminal state of an owner deletion operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionResult {
    /// Stable request identity.
    pub operation_id: Uuid,
    /// Deterministic content-free per-class effects applied by this operation.
    pub effects: Vec<DeletionEffectCount>,
}

/// Deterministic, content-free deletion preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionPlan {
    /// Stable operation identity used by apply.
    pub operation_id: Uuid,
    /// Classified per-class effects in inventory order.
    pub effects: Vec<DeletionEffectCount>,
}

/// Bounded count for one classified owned data class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeletionEffectCount {
    /// The classified storage class.
    pub class: OwnedDataClass,
    /// Planned target-specific action.
    pub action: DeletionAction,
    /// Rows or blob references affected, never content.
    pub affected_count: i64,
}

/// Owner deletion refusal or persistence failure.
#[derive(Debug, thiserror::Error)]
pub enum PrivacyDeletionError {
    /// The target is absent or does not belong to the authenticated owner.
    #[error("the deletion target was not found for this owner")]
    TargetNotFound,
    /// An operation id was already bound to a different owner or target.
    #[error("the deletion operation identity is already bound to another request")]
    OperationConflict,
    /// A canonical downstream removal request could not be appended.
    #[error("the local source removal request could not be published")]
    Publication(#[from] crate::publishing::PublishError),
    /// Owned persistence failed without exposing content or credentials.
    #[error(transparent)]
    Persistence(#[from] PersistenceError),
}

/// Owner-bound deletion application service.
#[derive(Debug)]
pub struct DeletionStore<'a> {
    database: &'a Database,
}

impl<'a> DeletionStore<'a> {
    /// Creates a deletion store over the Instagram-owned database.
    #[must_use]
    pub const fn new(database: &'a Database) -> Self {
        Self { database }
    }

    /// Computes a deterministic deletion plan without durable mutation.
    ///
    /// # Errors
    ///
    /// Returns a typed owner refusal or persistence failure.
    pub async fn preview(
        &self,
        request: DeletionRequest,
    ) -> Result<DeletionPlan, PrivacyDeletionError> {
        let mut transaction = self
            .database
            .pool()
            .begin()
            .await
            .map_err(PersistenceError::Query)?;
        require_owned_locked(&mut transaction, request).await?;
        let plan = build_plan(request);
        transaction
            .rollback()
            .await
            .map_err(PersistenceError::Query)?;
        Ok(DeletionPlan {
            operation_id: request.operation_id,
            effects: plan,
        })
    }

    /// Applies one stable owner deletion request.
    ///
    /// # Errors
    ///
    /// Returns a typed owner refusal or persistence failure.
    pub async fn apply(
        &self,
        request: DeletionRequest,
    ) -> Result<DeletionResult, PrivacyDeletionError> {
        if let Some(result) = load_existing(self.database, request).await? {
            return Ok(result);
        }
        let mut transaction = self
            .database
            .pool()
            .begin()
            .await
            .map_err(PersistenceError::Query)?;
        require_owned_locked(&mut transaction, request).await?;
        let effects = build_plan(request);
        let (target_kind, target_id) = target_parts(request.target);
        sqlx::query(
            "insert into instagram_archive.deletion_operations \
             (operation_id, user_ref, target_kind, target_id, reason, state, requested_at) \
             values ($1, $2, $3, $4, 'user_requested', 'planned', now())",
        )
        .bind(request.operation_id)
        .bind(request.user_ref)
        .bind(target_kind)
        .bind(target_id)
        .execute(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;
        for effect in &effects {
            sqlx::query(
                "insert into instagram_archive.deletion_effects \
                 (operation_id, data_class, action, affected_count) values ($1, $2, $3, $4)",
            )
            .bind(request.operation_id)
            .bind(effect.class.audit_key())
            .bind(effect.action.as_str())
            .bind(effect.affected_count)
            .execute(&mut *transaction)
            .await
            .map_err(PersistenceError::Query)?;
        }
        let pending_blob_tasks = apply::apply_target_rows(&mut transaction, request).await?;
        sqlx::query(
            "update instagram_archive.deletion_operations \
             set state = $2, updated_at = now(), \
                 finished_at = case when $2 = 'complete' then now() else null end \
             where operation_id = $1",
        )
        .bind(request.operation_id)
        .bind(if pending_blob_tasks > 0 {
            "pending_blob_deletion"
        } else {
            "complete"
        })
        .execute(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;
        transaction
            .commit()
            .await
            .map_err(PersistenceError::Query)?;
        Ok(DeletionResult {
            operation_id: request.operation_id,
            effects,
        })
    }
}

async fn load_existing(
    database: &Database,
    request: DeletionRequest,
) -> Result<Option<DeletionResult>, PrivacyDeletionError> {
    let existing: Option<(Uuid, String, Uuid, String)> = sqlx::query_as(
        "select user_ref, target_kind, target_id, state \
         from instagram_archive.deletion_operations where operation_id = $1",
    )
    .bind(request.operation_id)
    .fetch_optional(database.pool())
    .await
    .map_err(PersistenceError::Query)?;
    let Some((user_ref, target_kind, target_id, state)) = existing else {
        return Ok(None);
    };
    let (expected_kind, expected_id) = target_parts(request.target);
    if user_ref != request.user_ref || target_kind != expected_kind || target_id != expected_id {
        return Err(PrivacyDeletionError::OperationConflict);
    }
    if state != "complete" && state != "pending_blob_deletion" {
        return Err(PrivacyDeletionError::OperationConflict);
    }
    let stored: Vec<(String, String, i64)> = sqlx::query_as(
        "select data_class, action, affected_count from instagram_archive.deletion_effects \
         where operation_id = $1",
    )
    .bind(request.operation_id)
    .fetch_all(database.pool())
    .await
    .map_err(PersistenceError::Query)?;
    let mut effects = Vec::with_capacity(stored.len());
    for (class, action, affected_count) in stored {
        let class = OwnedDataClass::from_audit_key(&class)
            .ok_or(PrivacyDeletionError::OperationConflict)?;
        let action =
            DeletionAction::from_str(&action).ok_or(PrivacyDeletionError::OperationConflict)?;
        effects.push(DeletionEffectCount {
            class,
            action,
            affected_count,
        });
    }
    effects.sort_by_key(|effect| effect.class);
    Ok(Some(DeletionResult {
        operation_id: request.operation_id,
        effects,
    }))
}

fn build_plan(request: DeletionRequest) -> Vec<DeletionEffectCount> {
    let classifications = match request.target {
        DeletionTarget::Capture(_) => CAPTURE_DELETION_CLASSIFICATIONS,
        DeletionTarget::Connection(_) => CONNECTION_DELETION_CLASSIFICATIONS,
    };
    let mut effects = classifications
        .iter()
        .map(|entry| DeletionEffectCount {
            class: entry.class,
            action: entry.action,
            affected_count: 0,
        })
        .collect::<Vec<_>>();
    let target_class = match request.target {
        DeletionTarget::Capture(_) => OwnedDataClass::Captures,
        DeletionTarget::Connection(_) => OwnedDataClass::Accounts,
    };
    if let Some(effect) = effects
        .iter_mut()
        .find(|effect| effect.class == target_class)
    {
        effect.affected_count = 1;
    }
    effects
}

async fn require_owned_locked(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    request: DeletionRequest,
) -> Result<(), PrivacyDeletionError> {
    let owner: Option<Uuid> = match request.target {
        DeletionTarget::Capture(capture_id) => sqlx::query_scalar(
            "select user_ref from instagram_archive.captures where capture_id = $1 for update",
        )
        .bind(capture_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(PersistenceError::Query)?,
        DeletionTarget::Connection(account_id) => sqlx::query_scalar(
            "select user_ref from instagram_archive.accounts where account_id = $1 for update",
        )
        .bind(account_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(PersistenceError::Query)?,
    };
    if owner != Some(request.user_ref) {
        return Err(PrivacyDeletionError::TargetNotFound);
    }
    Ok(())
}

const fn target_parts(target: DeletionTarget) -> (&'static str, Uuid) {
    match target {
        DeletionTarget::Capture(id) => ("capture", id),
        DeletionTarget::Connection(id) => ("connection", id),
    }
}
