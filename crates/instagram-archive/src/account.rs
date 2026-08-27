//! Official account lifecycle persistence.

use sha2::{Digest as _, Sha256};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::Database;
use crate::capability_reconciliation::{
    AccountCapability, AccountObservation, AccountType, CapabilityPersistenceError,
    PermissionStatus, StoredAccountCapability,
};
use crate::credentials::crypto::{CredentialKeyring, CryptoError, TokenBinding, TokenKind};
use crate::provider::{
    InstagramProvider, ProviderError, ProviderFailureClass, ReqwestInstagramProvider,
};
use crate::provider_budget::{
    BudgetError, MetaUsage, ProviderBudget, RequestClass, UsageOutcome, UsageReservation,
};

/// Safe result of the optional provider-side revoke attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderRevokeOutcome {
    /// Provider documented and accepted the revoke.
    Succeeded,
    /// Provider documented the call but it failed.
    Failed,
    /// Selected profile documents no provider revoke operation.
    Unsupported,
}

/// Account lifecycle persistence failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AccountLifecycleError {
    /// Account is absent or not owned by the supplied Ratatoskr user.
    #[error("official Instagram account is unavailable")]
    Unavailable,
    /// An owned lifecycle transaction failed.
    #[error("official Instagram account lifecycle persistence failed")]
    Database(#[source] sqlx::Error),
    /// Selected provider profile exposes no refresh strategy.
    #[error("official Instagram refresh is unsupported")]
    UnsupportedRefresh,
    /// Stored credential could not be authenticated.
    #[error("official Instagram credential could not be opened")]
    Crypto(#[source] CryptoError),
    /// Provider refresh or discovery failed.
    #[error("official Instagram provider lifecycle operation failed")]
    Provider(#[source] ProviderError),
    /// Provider attempt accounting failed.
    #[error("official Instagram provider lifecycle budget failed")]
    Budget(#[source] BudgetError),
    /// Capability replacement failed.
    #[error("official Instagram capability replacement failed")]
    Capability(#[source] CapabilityPersistenceError),
}

impl Database {
    /// Checks exact internal-user ownership without exposing whether another owner has the account.
    ///
    /// # Errors
    ///
    /// Returns [`AccountLifecycleError`] when the owned query fails.
    pub async fn official_account_owned_by(
        &self,
        account_id: Uuid,
        user_ref: Uuid,
    ) -> Result<bool, AccountLifecycleError> {
        let owned: bool = sqlx::query_scalar(
            "select exists(select 1 from instagram_archive.accounts
             where account_id = $1 and user_ref = $2)",
        )
        .bind(account_id)
        .bind(user_ref)
        .fetch_one(self.pool())
        .await
        .map_err(AccountLifecycleError::Database)?;
        Ok(owned)
    }

    /// Refreshes encrypted credential material and re-discovers actual provider capabilities.
    ///
    /// # Errors
    ///
    /// Returns [`AccountLifecycleError`] for ownership, unsupported strategy, provider, budget,
    /// cryptography, or persistence failures.
    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "one lifecycle transaction boundary carries the complete owner, provider, budget, crypto, and clock policy"
    )]
    pub async fn refresh_official_account(
        &self,
        keyring: &CredentialKeyring,
        provider: &dyn InstagramProvider,
        account_id: Uuid,
        user_ref: Uuid,
        call_budget: u32,
        discovery_retries: u32,
        refresh_supported: bool,
        now: OffsetDateTime,
    ) -> Result<Vec<StoredAccountCapability>, AccountLifecycleError> {
        if !refresh_supported {
            return Err(AccountLifecycleError::UnsupportedRefresh);
        }
        let stored: Option<(Uuid, String, Vec<u8>)> = sqlx::query_as(
            "select a.user_ref, a.connection_status, c.access_token_envelope
             from instagram_archive.accounts a
             join instagram_archive.credentials c using (account_id)
             where a.account_id = $1",
        )
        .bind(account_id)
        .fetch_optional(self.pool())
        .await
        .map_err(AccountLifecycleError::Database)?;
        let Some((owner, connection_status, envelope)) = stored else {
            return Err(AccountLifecycleError::Unavailable);
        };
        if owner != user_ref || connection_status == "revoked" {
            return Err(AccountLifecycleError::Unavailable);
        }
        let access_token = keyring
            .open(
                TokenBinding {
                    subject_id: account_id,
                    kind: TokenKind::Access,
                },
                &envelope,
            )
            .map_err(AccountLifecycleError::Crypto)?;
        let operation_id = Uuid::now_v7();
        let mut budget =
            ProviderBudget::new(self.clone(), operation_id, Some(account_id), call_budget);

        let reservation = budget
            .reserve(RequestClass::TokenRefresh, now)
            .await
            .map_err(AccountLifecycleError::Budget)?;
        let refreshed = provider.refresh_token(&access_token).await;
        finish_attempt(&budget, reservation, refreshed.as_ref().err(), now).await?;
        let refreshed = match refreshed {
            Ok(refreshed) => refreshed,
            Err(error) if error.class == ProviderFailureClass::Authentication => {
                self.mark_reauthorization_required(account_id, user_ref, now)
                    .await?;
                return Err(AccountLifecycleError::Provider(error));
            }
            Err(error) => return Err(AccountLifecycleError::Provider(error)),
        };

        let mut account_retries = discovery_retries;
        let account = loop {
            let reservation = budget
                .reserve(RequestClass::AccountDiscovery, now)
                .await
                .map_err(AccountLifecycleError::Budget)?;
            let result = provider.discover_account(&refreshed.access_token).await;
            finish_attempt(&budget, reservation, result.as_ref().err(), now).await?;
            match result {
                Ok(account) => break account,
                Err(error) if error.class == ProviderFailureClass::Authentication => {
                    self.mark_reauthorization_required(account_id, user_ref, now)
                        .await?;
                    return Err(AccountLifecycleError::Provider(error));
                }
                Err(error)
                    if account_retries > 0
                        && ReqwestInstagramProvider::should_retry(
                            RequestClass::AccountDiscovery,
                            error.class,
                        ) =>
                {
                    account_retries -= 1;
                }
                Err(error) => return Err(AccountLifecycleError::Provider(error)),
            }
        };

        let mut permission_retries = discovery_retries;
        let permissions = loop {
            let reservation = budget
                .reserve(RequestClass::PermissionDiscovery, now)
                .await
                .map_err(AccountLifecycleError::Budget)?;
            let result = provider.discover_permissions(&refreshed.access_token).await;
            finish_attempt(&budget, reservation, result.as_ref().err(), now).await?;
            match result {
                Ok(permissions) => break permissions,
                Err(error) if error.class == ProviderFailureClass::Authentication => {
                    self.mark_reauthorization_required(account_id, user_ref, now)
                        .await?;
                    return Err(AccountLifecycleError::Provider(error));
                }
                Err(error)
                    if permission_retries > 0
                        && ReqwestInstagramProvider::should_retry(
                            RequestClass::PermissionDiscovery,
                            error.class,
                        ) =>
                {
                    permission_retries -= 1;
                }
                Err(error) => return Err(AccountLifecycleError::Provider(error)),
            }
        };
        let provider_id: String = sqlx::query_scalar(
            "select provider_account_id from instagram_archive.accounts where account_id = $1",
        )
        .bind(account_id)
        .fetch_one(self.pool())
        .await
        .map_err(AccountLifecycleError::Database)?;
        if provider_id != account.provider_account_id {
            return Err(AccountLifecycleError::Unavailable);
        }

        let new_envelope = keyring
            .seal(
                TokenBinding {
                    subject_id: account_id,
                    kind: TokenKind::Access,
                },
                &refreshed.access_token,
            )
            .map_err(AccountLifecycleError::Crypto)?;
        let key_version = i32::try_from(keyring.current_version())
            .map_err(|_| AccountLifecycleError::Unavailable)?;
        let expires_at = refreshed
            .expires_in_seconds
            .and_then(|seconds| i64::try_from(seconds).ok())
            .map(|seconds| now + time::Duration::seconds(seconds));
        let granted_permissions = permissions
            .statuses
            .iter()
            .filter(|(_, status)| **status == PermissionStatus::Granted)
            .map(|(permission, _)| permission.clone())
            .collect::<Vec<_>>();
        let mut transaction = self
            .pool()
            .begin()
            .await
            .map_err(AccountLifecycleError::Database)?;
        let current_status: Option<String> = sqlx::query_scalar(
            "select connection_status from instagram_archive.accounts
             where account_id = $1 and user_ref = $2 for update",
        )
        .bind(account_id)
        .bind(user_ref)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(AccountLifecycleError::Database)?;
        if current_status.as_deref() != Some("connected") {
            return Err(AccountLifecycleError::Unavailable);
        }
        let raw_record_id =
            insert_permission_evidence(&mut transaction, &permissions.statuses, now).await?;
        sqlx::query(
            "update instagram_archive.credentials
             set access_token_envelope = $2, refresh_token_envelope = null, key_version = $3,
                 granted_permissions = $4, expires_at = $5, rotated_at = $6
             where account_id = $1",
        )
        .bind(account_id)
        .bind(new_envelope)
        .bind(key_version)
        .bind(&granted_permissions)
        .bind(expires_at)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(AccountLifecycleError::Database)?;
        sqlx::query(
            "update instagram_archive.accounts
             set username = $2, account_type = $3, connection_status = 'connected',
                 scopes = $4, updated_at = $5
             where account_id = $1 and user_ref = $6",
        )
        .bind(account_id)
        .bind(&account.username)
        .bind(account_type_wire(account.account_type))
        .bind(&granted_permissions)
        .bind(now)
        .bind(user_ref)
        .execute(&mut *transaction)
        .await
        .map_err(AccountLifecycleError::Database)?;
        sqlx::query(
            "insert into instagram_archive.account_credential_audit
             (audit_id, account_id, change_kind, outcome, detail, occurred_at)
             values ($1, $2, 'refreshed', 'succeeded', '{}', $3)",
        )
        .bind(Uuid::now_v7())
        .bind(account_id)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(AccountLifecycleError::Database)?;
        self.reconcile_account_capabilities_in_transaction(
            &mut transaction,
            account_id,
            &AccountObservation {
                provider_account_id: account.provider_account_id,
                account_type: account.account_type,
                permissions: permissions.statuses,
                external_write_consent: false,
                observed_at: now,
                raw_record_id,
            },
        )
        .await
        .map_err(AccountLifecycleError::Capability)?;
        transaction
            .commit()
            .await
            .map_err(AccountLifecycleError::Database)?;
        self.load_account_capabilities(account_id)
            .await
            .map_err(AccountLifecycleError::Capability)
    }

    async fn mark_reauthorization_required(
        &self,
        account_id: Uuid,
        user_ref: Uuid,
        now: OffsetDateTime,
    ) -> Result<(), AccountLifecycleError> {
        let mut transaction = self
            .pool()
            .begin()
            .await
            .map_err(AccountLifecycleError::Database)?;
        sqlx::query("delete from instagram_archive.credentials where account_id = $1")
            .bind(account_id)
            .execute(&mut *transaction)
            .await
            .map_err(AccountLifecycleError::Database)?;
        sqlx::query("delete from instagram_archive.account_capabilities where account_id = $1")
            .bind(account_id)
            .execute(&mut *transaction)
            .await
            .map_err(AccountLifecycleError::Database)?;
        let generation_id = Uuid::now_v7();
        for capability in AccountCapability::ALL {
            sqlx::query(
                "insert into instagram_archive.account_capabilities
                 (account_id, generation_id, capability, capability_state, reason, observed_at)
                 values ($1, $2, $3, 'unavailable', 'reauthorization_required', $4)",
            )
            .bind(account_id)
            .bind(generation_id)
            .bind(capability.wire_value())
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(AccountLifecycleError::Database)?;
        }
        let updated = sqlx::query(
            "update instagram_archive.accounts
             set connection_status = 'reauthorization_required', scopes = '{}', updated_at = $3
             where account_id = $1 and user_ref = $2",
        )
        .bind(account_id)
        .bind(user_ref)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(AccountLifecycleError::Database)?;
        if updated.rows_affected() != 1 {
            return Err(AccountLifecycleError::Unavailable);
        }
        sqlx::query(
            "insert into instagram_archive.account_credential_audit
             (audit_id, account_id, change_kind, outcome, detail, occurred_at)
             values ($1, $2, 'reauthorization_required', 'authentication_failed', '{}', $3)",
        )
        .bind(Uuid::now_v7())
        .bind(account_id)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(AccountLifecycleError::Database)?;
        transaction
            .commit()
            .await
            .map_err(AccountLifecycleError::Database)
    }

    /// Scrubs every locally recoverable secret and marks the account revoked.
    ///
    /// # Errors
    ///
    /// Returns [`AccountLifecycleError`] when ownership fails or the transaction cannot commit.
    pub async fn scrub_revoked_account(
        &self,
        account_id: Uuid,
        user_ref: Uuid,
        provider_outcome: ProviderRevokeOutcome,
        now: OffsetDateTime,
    ) -> Result<(), AccountLifecycleError> {
        let mut transaction = self
            .pool()
            .begin()
            .await
            .map_err(AccountLifecycleError::Database)?;
        let account: Option<(Uuid, String)> = sqlx::query_as(
            "select user_ref, connection_status from instagram_archive.accounts
             where account_id = $1 for update",
        )
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(AccountLifecycleError::Database)?;
        let Some((stored_owner, connection_status)) = account else {
            return Err(AccountLifecycleError::Unavailable);
        };
        if stored_owner != user_ref {
            return Err(AccountLifecycleError::Unavailable);
        }
        if connection_status == "revoked" {
            return Ok(());
        }
        sqlx::query("delete from instagram_archive.credentials where account_id = $1")
            .bind(account_id)
            .execute(&mut *transaction)
            .await
            .map_err(AccountLifecycleError::Database)?;
        sqlx::query("delete from instagram_archive.oauth_flows where user_ref = $1")
            .bind(user_ref)
            .execute(&mut *transaction)
            .await
            .map_err(AccountLifecycleError::Database)?;
        sqlx::query("delete from instagram_archive.account_capabilities where account_id = $1")
            .bind(account_id)
            .execute(&mut *transaction)
            .await
            .map_err(AccountLifecycleError::Database)?;
        let generation_id = Uuid::now_v7();
        for capability in AccountCapability::ALL {
            sqlx::query(
                "insert into instagram_archive.account_capabilities
                 (account_id, generation_id, capability, capability_state, reason, observed_at)
                 values ($1, $2, $3, 'unavailable', 'revoked', $4)",
            )
            .bind(account_id)
            .bind(generation_id)
            .bind(capability.wire_value())
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(AccountLifecycleError::Database)?;
        }
        sqlx::query(
            "update instagram_archive.accounts
             set connection_status = 'revoked', scopes = '{}', updated_at = $3
             where account_id = $1 and user_ref = $2",
        )
        .bind(account_id)
        .bind(user_ref)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(AccountLifecycleError::Database)?;
        let outcome = match provider_outcome {
            ProviderRevokeOutcome::Succeeded => "succeeded",
            ProviderRevokeOutcome::Failed => "provider_failed",
            ProviderRevokeOutcome::Unsupported => "provider_unsupported",
        };
        let detail = serde_json::json!({ "provider_revoke": outcome });
        sqlx::query(
            "insert into instagram_archive.account_credential_audit
             (audit_id, account_id, change_kind, outcome, detail, occurred_at)
             values ($1, $2, 'revoked', $3, $4, $5)",
        )
        .bind(Uuid::now_v7())
        .bind(account_id)
        .bind(outcome)
        .bind(detail)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(AccountLifecycleError::Database)?;
        transaction
            .commit()
            .await
            .map_err(AccountLifecycleError::Database)?;
        Ok(())
    }

    /// Scrubs accounts stranded in `revoking` during startup.
    ///
    /// # Errors
    ///
    /// Returns [`AccountLifecycleError`] when the sweep cannot complete.
    pub async fn scrub_stranded_revocations(
        &self,
        now: OffsetDateTime,
    ) -> Result<u64, AccountLifecycleError> {
        let accounts: Vec<(Uuid, Uuid)> = sqlx::query_as(
            "select account_id, user_ref from instagram_archive.accounts
             where connection_status = 'revoking' order by account_id",
        )
        .fetch_all(self.pool())
        .await
        .map_err(AccountLifecycleError::Database)?;
        let count =
            u64::try_from(accounts.len()).map_err(|_| AccountLifecycleError::Unavailable)?;
        for (account_id, user_ref) in accounts {
            self.scrub_revoked_account(account_id, user_ref, ProviderRevokeOutcome::Failed, now)
                .await?;
        }
        Ok(count)
    }
}

async fn finish_attempt(
    budget: &ProviderBudget,
    reservation: UsageReservation,
    error: Option<&ProviderError>,
    now: OffsetDateTime,
) -> Result<(), AccountLifecycleError> {
    let outcome = error.map_or(UsageOutcome::Succeeded, |error| match error.class {
        ProviderFailureClass::Authentication => UsageOutcome::Authentication,
        ProviderFailureClass::Validation => UsageOutcome::Validation,
        ProviderFailureClass::RateLimited => UsageOutcome::RateLimited,
        ProviderFailureClass::Server => UsageOutcome::Server,
        ProviderFailureClass::Network => UsageOutcome::Network,
        ProviderFailureClass::ResponseRefused => UsageOutcome::ResponseRefused,
        ProviderFailureClass::Unsupported => UsageOutcome::ProviderUnsupported,
    });
    budget
        .complete(
            reservation,
            outcome,
            error.and_then(|error| error.http_status),
            MetaUsage::default(),
            now,
        )
        .await
        .map_err(AccountLifecycleError::Budget)
}

async fn insert_permission_evidence(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    permissions: &std::collections::BTreeMap<String, PermissionStatus>,
    now: OffsetDateTime,
) -> Result<Uuid, AccountLifecycleError> {
    let body = serde_json::to_vec(&serde_json::json!({
        "data": permissions.iter().map(|(permission, status)| {
            serde_json::json!({
                "permission": permission,
                "status": permission_status_wire(*status),
            })
        }).collect::<Vec<_>>()
    }))
    .map_err(|_| AccountLifecycleError::Unavailable)?;
    let digest = Sha256::digest(&body);
    let blob_ref = format!("{digest:x}");
    let byte_size = i64::try_from(body.len()).map_err(|_| AccountLifecycleError::Unavailable)?;
    let raw_record_id = Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.raw_records
         (raw_record_id, record_kind, blob_ref, content_hash, byte_size, body, observed_at)
         values ($1, 'api_response', $2, $3, $4, $5, $6)",
    )
    .bind(raw_record_id)
    .bind(blob_ref)
    .bind(digest.to_vec())
    .bind(byte_size)
    .bind(body)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(AccountLifecycleError::Database)?;
    Ok(raw_record_id)
}

const fn account_type_wire(account_type: AccountType) -> &'static str {
    match account_type {
        AccountType::Business => "business",
        AccountType::Creator => "creator",
        AccountType::Personal => "personal",
        AccountType::Unknown => "unknown",
    }
}

const fn permission_status_wire(status: PermissionStatus) -> &'static str {
    match status {
        PermissionStatus::Granted => "granted",
        PermissionStatus::Declined => "declined",
        PermissionStatus::Expired => "expired",
        PermissionStatus::Absent => "absent",
        PermissionStatus::Unknown => "unknown",
    }
}
