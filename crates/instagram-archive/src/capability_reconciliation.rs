//! Closed capability reconciliation for one official Instagram account.

use std::collections::{BTreeMap, BTreeSet};

use sqlx::Row as _;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::Database;

/// The account type reported by the provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountType {
    /// Professional business account.
    Business,
    /// Professional creator account.
    Creator,
    /// Personal account, unsupported by this official lane.
    Personal,
    /// Missing or unrecognized provider value.
    Unknown,
}

impl AccountType {
    fn is_professional(self) -> bool {
        matches!(self, Self::Business | Self::Creator)
    }

    const fn wire_value(self) -> &'static str {
        match self {
            Self::Business => "business",
            Self::Creator => "creator",
            Self::Personal => "personal",
            Self::Unknown => "unknown",
        }
    }
}

/// Provider-observed status for one permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionStatus {
    /// Provider reports the permission granted.
    Granted,
    /// User declined the permission.
    Declined,
    /// A former grant expired.
    Expired,
    /// Provider's complete observation omitted the permission.
    Absent,
    /// Provider returned a status this version does not recognize.
    Unknown,
}

impl PermissionStatus {
    const fn wire_value(self) -> &'static str {
        match self {
            Self::Granted => "granted",
            Self::Declined => "declined",
            Self::Expired => "expired",
            Self::Absent => "absent",
            Self::Unknown => "unknown",
        }
    }
}

/// A complete provider observation for one account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountObservation {
    /// Stable external account identity.
    pub provider_account_id: String,
    /// Provider-observed account type.
    pub account_type: AccountType,
    /// Complete permission name-to-status map.
    pub permissions: BTreeMap<String, PermissionStatus>,
    /// Separate consent for provider external writes.
    pub external_write_consent: bool,
    /// Observation time.
    pub observed_at: OffsetDateTime,
    /// Protected raw evidence row.
    pub raw_record_id: Uuid,
}

/// Closed official-account capability inventory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AccountCapability {
    /// Read stable provider account identity and type.
    AccountIdentityRead,
    /// Read the connected account's own media in a future sync item.
    OwnMediaRead,
    /// Publish content, intentionally disabled in this item.
    ContentPublish,
    /// Manage comments, intentionally disabled in this item.
    CommentManagement,
    /// Manage messages, intentionally disabled in this item.
    MessageManagement,
    /// Read native Saved membership, unsupported by the provider surface.
    NativeSavedRead,
}

impl AccountCapability {
    /// Every capability, exactly once per reconciliation generation.
    pub const ALL: [Self; 6] = [
        Self::AccountIdentityRead,
        Self::OwnMediaRead,
        Self::ContentPublish,
        Self::CommentManagement,
        Self::MessageManagement,
        Self::NativeSavedRead,
    ];

    /// Stable database/wire representation.
    #[must_use]
    pub const fn wire_value(self) -> &'static str {
        match self {
            Self::AccountIdentityRead => "account_identity_read",
            Self::OwnMediaRead => "own_media_read",
            Self::ContentPublish => "content_publish",
            Self::CommentManagement => "comment_management",
            Self::MessageManagement => "message_management",
            Self::NativeSavedRead => "native_saved_read",
        }
    }
}

/// Reconciled availability state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityState {
    /// Current evidence grants the capability.
    Available,
    /// Current evidence does not grant the capability.
    Unavailable,
    /// The supported provider surface cannot grant the capability.
    NotSupported,
}

/// Closed explanation for a reconciled state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityReason {
    /// Professional account and required permission are currently granted.
    Granted,
    /// Provider account type is not supported by this lane.
    AccountTypeUnsupported,
    /// User declined the required permission.
    PermissionDeclined,
    /// Required permission expired.
    PermissionExpired,
    /// Complete observation did not contain the required permission.
    PermissionAbsent,
    /// Provider returned an unrecognized permission status.
    PermissionUnknown,
    /// Exact provider write permission is not granted.
    MissingPermission,
    /// Separate external-write consent is absent.
    WriteConsentRequired,
    /// Provider exposes no supported surface for this capability.
    ProviderNotSupported,
    /// Account was locally revoked.
    Revoked,
    /// Provider authentication requires user reauthorization.
    ReauthorizationRequired,
}

/// One row of a complete reconciliation result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconciledCapability {
    /// Capability described by this row.
    pub capability: AccountCapability,
    /// Current availability.
    pub state: CapabilityState,
    /// Closed explanation.
    pub reason: CapabilityReason,
}

/// One persisted capability row including its complete generation identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoredAccountCapability {
    /// Reconciliation generation shared by every row from one observation.
    pub generation_id: Uuid,
    /// Reconciled row.
    pub reconciled: ReconciledCapability,
}

/// Capability projection persistence failure with no provider or database detail in Display.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CapabilityPersistenceError {
    /// An owned query failed.
    #[error("account capability persistence failed")]
    Database(#[source] sqlx::Error),
    /// A stored closed vocabulary value was not recognized.
    #[error("stored account capability state is malformed")]
    MalformedRecord,
}

impl Database {
    /// Replaces an account's permission evidence and total capability matrix.
    ///
    /// # Errors
    ///
    /// Returns [`CapabilityPersistenceError`] when the transaction cannot commit.
    pub async fn reconcile_account_capabilities(
        &self,
        account_id: Uuid,
        observation: &AccountObservation,
    ) -> Result<Uuid, CapabilityPersistenceError> {
        let mut transaction = self
            .pool()
            .begin()
            .await
            .map_err(CapabilityPersistenceError::Database)?;
        let generation_id = self
            .reconcile_account_capabilities_in_transaction(
                &mut transaction,
                account_id,
                observation,
            )
            .await?;
        transaction
            .commit()
            .await
            .map_err(CapabilityPersistenceError::Database)?;
        Ok(generation_id)
    }

    /// Replaces the complete capability generation inside a caller-owned transaction.
    pub(crate) async fn reconcile_account_capabilities_in_transaction(
        &self,
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        account_id: Uuid,
        observation: &AccountObservation,
    ) -> Result<Uuid, CapabilityPersistenceError> {
        const REQUIRED_PERMISSIONS: [&str; 4] = [
            "instagram_business_basic",
            "instagram_business_content_publish",
            "instagram_business_manage_comments",
            "instagram_business_manage_messages",
        ];
        let generation_id = Uuid::now_v7();
        sqlx::query(
            "delete from instagram_archive.account_permission_observations where account_id = $1",
        )
        .bind(account_id)
        .execute(&mut **transaction)
        .await
        .map_err(CapabilityPersistenceError::Database)?;
        sqlx::query("delete from instagram_archive.account_capabilities where account_id = $1")
            .bind(account_id)
            .execute(&mut **transaction)
            .await
            .map_err(CapabilityPersistenceError::Database)?;

        let permission_names = observation
            .permissions
            .keys()
            .map(String::as_str)
            .chain(REQUIRED_PERMISSIONS)
            .collect::<BTreeSet<_>>();
        for permission_name in permission_names {
            let status = observation
                .permissions
                .get(permission_name)
                .copied()
                .unwrap_or(PermissionStatus::Absent);
            sqlx::query(
                "insert into instagram_archive.account_permission_observations
                 (observation_id, account_id, generation_id, permission_name, permission_status,
                  raw_record_id, observed_at)
                 values ($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(Uuid::now_v7())
            .bind(account_id)
            .bind(generation_id)
            .bind(permission_name)
            .bind(status.wire_value())
            .bind(observation.raw_record_id)
            .bind(observation.observed_at)
            .execute(&mut **transaction)
            .await
            .map_err(CapabilityPersistenceError::Database)?;
        }

        for reconciled in reconcile(observation) {
            sqlx::query(
                "insert into instagram_archive.account_capabilities
                 (account_id, generation_id, capability, capability_state, reason, observed_at)
                 values ($1, $2, $3, $4, $5, $6)",
            )
            .bind(account_id)
            .bind(generation_id)
            .bind(reconciled.capability.wire_value())
            .bind(state_wire_value(reconciled.state))
            .bind(reason_wire_value(reconciled.reason))
            .bind(observation.observed_at)
            .execute(&mut **transaction)
            .await
            .map_err(CapabilityPersistenceError::Database)?;
        }
        let granted_permissions = observation
            .permissions
            .iter()
            .filter(|(_, status)| **status == PermissionStatus::Granted)
            .map(|(permission, _)| permission.clone())
            .collect::<Vec<_>>();
        sqlx::query(
            "update instagram_archive.accounts
             set account_type = $2, scopes = $3, updated_at = now()
             where account_id = $1",
        )
        .bind(account_id)
        .bind(observation.account_type.wire_value())
        .bind(granted_permissions)
        .execute(&mut **transaction)
        .await
        .map_err(CapabilityPersistenceError::Database)?;
        Ok(generation_id)
    }

    /// Loads the total current matrix for exactly one account.
    ///
    /// # Errors
    ///
    /// Returns [`CapabilityPersistenceError`] when rows cannot be read or decoded.
    pub async fn load_account_capabilities(
        &self,
        account_id: Uuid,
    ) -> Result<Vec<StoredAccountCapability>, CapabilityPersistenceError> {
        let rows = sqlx::query(
            "select generation_id, capability, capability_state, reason
             from instagram_archive.account_capabilities
             where account_id = $1 order by capability",
        )
        .bind(account_id)
        .fetch_all(self.pool())
        .await
        .map_err(CapabilityPersistenceError::Database)?;
        rows.into_iter()
            .map(|row| {
                let capability_text: String = row
                    .try_get("capability")
                    .map_err(CapabilityPersistenceError::Database)?;
                let state_text: String = row
                    .try_get("capability_state")
                    .map_err(CapabilityPersistenceError::Database)?;
                let reason_text: String = row
                    .try_get("reason")
                    .map_err(CapabilityPersistenceError::Database)?;
                Ok(StoredAccountCapability {
                    generation_id: row
                        .try_get("generation_id")
                        .map_err(CapabilityPersistenceError::Database)?,
                    reconciled: ReconciledCapability {
                        capability: parse_capability(&capability_text)
                            .ok_or(CapabilityPersistenceError::MalformedRecord)?,
                        state: parse_state(&state_text)
                            .ok_or(CapabilityPersistenceError::MalformedRecord)?,
                        reason: parse_reason(&reason_text)
                            .ok_or(CapabilityPersistenceError::MalformedRecord)?,
                    },
                })
            })
            .collect()
    }
}

/// Reconciles one complete observation into all closed capability rows.
#[must_use]
pub fn reconcile(observation: &AccountObservation) -> Vec<ReconciledCapability> {
    AccountCapability::ALL
        .into_iter()
        .map(|capability| reconcile_one(observation, capability))
        .collect()
}

fn reconcile_one(
    observation: &AccountObservation,
    capability: AccountCapability,
) -> ReconciledCapability {
    if capability == AccountCapability::NativeSavedRead {
        return row(
            capability,
            CapabilityState::NotSupported,
            CapabilityReason::ProviderNotSupported,
        );
    }
    if !observation.account_type.is_professional() {
        return row(
            capability,
            CapabilityState::Unavailable,
            CapabilityReason::AccountTypeUnsupported,
        );
    }
    let basic = observation
        .permissions
        .get("instagram_business_basic")
        .copied()
        .unwrap_or(PermissionStatus::Absent);
    if basic != PermissionStatus::Granted {
        return row(
            capability,
            CapabilityState::Unavailable,
            permission_reason(basic, false),
        );
    }
    let write_permission = match capability {
        AccountCapability::AccountIdentityRead | AccountCapability::OwnMediaRead => {
            return row(
                capability,
                CapabilityState::Available,
                CapabilityReason::Granted,
            );
        }
        AccountCapability::ContentPublish => "instagram_business_content_publish",
        AccountCapability::CommentManagement => "instagram_business_manage_comments",
        AccountCapability::MessageManagement => "instagram_business_manage_messages",
        AccountCapability::NativeSavedRead => {
            return row(
                capability,
                CapabilityState::NotSupported,
                CapabilityReason::ProviderNotSupported,
            );
        }
    };
    let status = observation
        .permissions
        .get(write_permission)
        .copied()
        .unwrap_or(PermissionStatus::Absent);
    if status != PermissionStatus::Granted {
        return row(
            capability,
            CapabilityState::Unavailable,
            permission_reason(status, true),
        );
    }
    if !observation.external_write_consent {
        return row(
            capability,
            CapabilityState::Unavailable,
            CapabilityReason::WriteConsentRequired,
        );
    }
    row(
        capability,
        CapabilityState::Available,
        CapabilityReason::Granted,
    )
}

const fn permission_reason(status: PermissionStatus, write_permission: bool) -> CapabilityReason {
    match status {
        PermissionStatus::Granted => CapabilityReason::Granted,
        PermissionStatus::Declined => CapabilityReason::PermissionDeclined,
        PermissionStatus::Expired => CapabilityReason::PermissionExpired,
        PermissionStatus::Absent if write_permission => CapabilityReason::MissingPermission,
        PermissionStatus::Absent => CapabilityReason::PermissionAbsent,
        PermissionStatus::Unknown => CapabilityReason::PermissionUnknown,
    }
}

const fn row(
    capability: AccountCapability,
    state: CapabilityState,
    reason: CapabilityReason,
) -> ReconciledCapability {
    ReconciledCapability {
        capability,
        state,
        reason,
    }
}

const fn state_wire_value(state: CapabilityState) -> &'static str {
    match state {
        CapabilityState::Available => "available",
        CapabilityState::Unavailable => "unavailable",
        CapabilityState::NotSupported => "not_supported",
    }
}

const fn reason_wire_value(reason: CapabilityReason) -> &'static str {
    match reason {
        CapabilityReason::Granted => "granted",
        CapabilityReason::AccountTypeUnsupported => "account_type_unsupported",
        CapabilityReason::PermissionDeclined => "permission_declined",
        CapabilityReason::PermissionExpired => "permission_expired",
        CapabilityReason::PermissionAbsent => "permission_absent",
        CapabilityReason::PermissionUnknown => "permission_unknown",
        CapabilityReason::MissingPermission => "missing_permission",
        CapabilityReason::WriteConsentRequired => "write_consent_required",
        CapabilityReason::ProviderNotSupported => "provider_not_supported",
        CapabilityReason::Revoked => "revoked",
        CapabilityReason::ReauthorizationRequired => "reauthorization_required",
    }
}

fn parse_capability(value: &str) -> Option<AccountCapability> {
    match value {
        "account_identity_read" => Some(AccountCapability::AccountIdentityRead),
        "own_media_read" => Some(AccountCapability::OwnMediaRead),
        "content_publish" => Some(AccountCapability::ContentPublish),
        "comment_management" => Some(AccountCapability::CommentManagement),
        "message_management" => Some(AccountCapability::MessageManagement),
        "native_saved_read" => Some(AccountCapability::NativeSavedRead),
        _ => None,
    }
}

fn parse_state(value: &str) -> Option<CapabilityState> {
    match value {
        "available" => Some(CapabilityState::Available),
        "unavailable" => Some(CapabilityState::Unavailable),
        "not_supported" => Some(CapabilityState::NotSupported),
        _ => None,
    }
}

fn parse_reason(value: &str) -> Option<CapabilityReason> {
    match value {
        "granted" => Some(CapabilityReason::Granted),
        "account_type_unsupported" => Some(CapabilityReason::AccountTypeUnsupported),
        "permission_declined" => Some(CapabilityReason::PermissionDeclined),
        "permission_expired" => Some(CapabilityReason::PermissionExpired),
        "permission_absent" => Some(CapabilityReason::PermissionAbsent),
        "permission_unknown" => Some(CapabilityReason::PermissionUnknown),
        "missing_permission" => Some(CapabilityReason::MissingPermission),
        "write_consent_required" => Some(CapabilityReason::WriteConsentRequired),
        "provider_not_supported" => Some(CapabilityReason::ProviderNotSupported),
        "revoked" => Some(CapabilityReason::Revoked),
        "reauthorization_required" => Some(CapabilityReason::ReauthorizationRequired),
        _ => None,
    }
}
