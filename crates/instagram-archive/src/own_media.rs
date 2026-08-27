//! Scheduled, capability-gated synchronization of a connected account's own media.
use crate::Database;
use crate::credentials::crypto::{CredentialKeyring, CryptoError, TokenBinding, TokenKind};
use crate::provider::{
    InstagramProvider, ProviderError, ProviderFailureClass, ProviderOwnMediaItem,
};
use crate::provider_budget::{BudgetError, MetaUsage, ProviderBudget, RequestClass, UsageOutcome};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use time::OffsetDateTime;
use uuid::Uuid;
type ActiveRunRow = (Uuid, Option<String>, Option<String>, Option<String>, Uuid);
/// Finite policy for one scheduler pass and one account traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct OwnMediaSyncConfig {
    /// Whether the local scheduler may contact the provider.
    pub enabled: bool,
    /// Delay after one terminal account outcome.
    pub cadence_seconds: u64,
    /// Maximum accounts claimed by one scheduler pass.
    pub accounts_per_tick: u32,
    /// Maximum accepted pages in one run attempt.
    pub pages_per_run: u32,
    /// Maximum durable provider reservations in one run attempt.
    pub call_budget: u32,
}
/// Closed result of one account job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnMediaSyncOutcome {
    /// Current capability truthfully prevented provider contact.
    CapabilityNoop {
        /// Durable run recording the decision.
        run_id: Uuid,
        /// Closed capability reason copied from the current generation.
        reason: String,
    },
    /// A complete traversal atomically became current.
    Completed {
        /// Durable authoritative run.
        run_id: Uuid,
        /// Newly committed newest provider identity, when the account has media.
        watermark_provider_media_id: Option<String>,
    },
    /// A partial traversal retained its committed cursor without authority.
    Retryable {
        /// Durable resumable run.
        run_id: Uuid,
        /// Cursor for the next page, or the beginning when no page was accepted.
        next_cursor: Option<String>,
    },
}
/// Bounded result counters from one direct scheduler pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OwnMediaSyncSummary {
    /// Due accounts selected by the finite query.
    pub attempted: u32,
    /// Traversals that became authoritative.
    pub completed: u32,
    /// Jobs truthfully skipped by current capability state.
    pub capability_noops: u32,
    /// Runs that retained resumable progress.
    pub retryable: u32,
    /// Account jobs that terminated with a redacted failure.
    pub failed: u32,
}
/// Redacted own-media job failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum OwnMediaSyncError {
    /// Account or current capability row does not exist.
    #[error("own-media account job is unavailable")]
    Unavailable,
    /// Owned persistence failed.
    #[error("own-media synchronization persistence failed")]
    Database(#[source] sqlx::Error),
    /// Encrypted credential could not be authenticated.
    #[error("own-media credential could not be opened")]
    Crypto(#[source] CryptoError),
    /// Durable provider-attempt accounting failed.
    #[error("own-media provider budget failed")]
    Budget(#[source] BudgetError),
    /// Official provider request failed with a redacted class.
    #[error("own-media provider request failed")]
    Provider(#[source] ProviderError),
    /// A normalized `SocialSource` fact could not be built or appended.
    #[error("own-media publication failed")]
    Publish(#[source] crate::publishing::PublishError),
    /// Stored or provider metadata violated the closed grammar.
    #[error("own-media metadata is malformed")]
    MalformedMetadata,
}
/// One bounded composition root for account jobs.
pub struct OwnMediaSyncExecutor<'a> {
    database: &'a Database,
    keyring: &'a CredentialKeyring,
    provider: &'a dyn InstagramProvider,
    config: OwnMediaSyncConfig,
}
impl std::fmt::Debug for OwnMediaSyncExecutor<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OwnMediaSyncExecutor")
            .field("database", self.database)
            .field("keyring", self.keyring)
            .field("provider", &"[OFFICIAL_PROVIDER]")
            .field("config", &self.config)
            .finish()
    }
}
impl<'a> OwnMediaSyncExecutor<'a> {
    /// Creates an executor. Scheduling remains disabled unless configuration explicitly enables it.
    #[must_use]
    pub const fn new(
        database: &'a Database,
        keyring: &'a CredentialKeyring,
        provider: &'a dyn InstagramProvider,
        config: OwnMediaSyncConfig,
    ) -> Self {
        Self {
            database,
            keyring,
            provider,
            config,
        }
    }
    /// Runs one account job after evaluating its complete current capability generation.
    /// # Errors
    /// Returns [`OwnMediaSyncError`] when current state is absent or persistence fails.
    pub async fn run_account_once(
        &self,
        account_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<OwnMediaSyncOutcome, OwnMediaSyncError> {
        let row: Option<(Uuid, Uuid, String, String, String)> = sqlx::query_as(
            "select a.user_ref, c.generation_id, a.connection_status,
                    c.capability_state, c.reason
             from instagram_archive.accounts a
             join instagram_archive.account_capabilities c on c.account_id = a.account_id
             where a.account_id = $1 and c.capability = 'own_media_read'",
        )
        .bind(account_id)
        .fetch_optional(self.database.pool())
        .await
        .map_err(OwnMediaSyncError::Database)?;
        let Some((user_ref, generation_id, connection_status, state, reason)) = row else {
            return Err(OwnMediaSyncError::Unavailable);
        };
        if connection_status != "connected" || state != "available" {
            let run_id = Uuid::now_v7();
            let reason = if connection_status == "reauthorization_required" {
                "reauthorization_required".to_owned()
            } else if connection_status == "revoked" {
                "revoked".to_owned()
            } else {
                reason
            };
            let cadence = i64::try_from(self.config.cadence_seconds)
                .map_err(|_| OwnMediaSyncError::MalformedMetadata)?;
            let next_due_at = now + time::Duration::seconds(cadence);
            let mut transaction = self
                .database
                .pool()
                .begin()
                .await
                .map_err(OwnMediaSyncError::Database)?;
            sqlx::query(
                "with closed_stale as (
                   update instagram_archive.own_media_sync_runs set status = 'failed', outcome_reason = 'capability_changed', updated_at = $6, finished_at = $6 where account_id = $2 and status in ('running', 'retryable') returning run_id
                 ) insert into instagram_archive.own_media_sync_runs
                 (run_id, account_id, user_ref, capability_generation_id, status, outcome_reason,
                  started_at, updated_at, finished_at)
                 values ($1, $2, $3, $4, 'capability_noop', $5, $6, $6, $6)",
            )
            .bind(run_id)
            .bind(account_id)
            .bind(user_ref)
            .bind(generation_id)
            .bind(&reason)
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(OwnMediaSyncError::Database)?;
            sqlx::query(
                "insert into instagram_archive.own_media_sync_state
                 (account_id, watermark_provider_media_id, next_due_at,
                  last_run_id, last_outcome, updated_at)
                 values ($1, null, $2, $3, 'capability_noop', $4)
                 on conflict (account_id) do update set
                   next_due_at = excluded.next_due_at, last_run_id = excluded.last_run_id,
                   last_outcome = excluded.last_outcome, updated_at = excluded.updated_at",
            )
            .bind(account_id)
            .bind(next_due_at)
            .bind(run_id)
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(OwnMediaSyncError::Database)?;
            transaction
                .commit()
                .await
                .map_err(OwnMediaSyncError::Database)?;
            return Ok(OwnMediaSyncOutcome::CapabilityNoop { run_id, reason });
        }
        self.run_eligible(account_id, user_ref, generation_id, now)
            .await
    }
    /// Claims and runs one bounded set of due accounts without sleeping.
    /// # Errors
    /// Returns [`OwnMediaSyncError::Database`] when the due query cannot be read.
    pub async fn run_due_once(
        &self,
        now: OffsetDateTime,
    ) -> Result<OwnMediaSyncSummary, OwnMediaSyncError> {
        if !self.config.enabled {
            return Ok(OwnMediaSyncSummary::default());
        }
        let limit = i64::from(self.config.accounts_per_tick);
        let account_ids: Vec<Uuid> = sqlx::query_scalar(
            "select a.account_id
             from instagram_archive.accounts a
             left join instagram_archive.own_media_sync_state s using (account_id)
             left join instagram_archive.own_media_sync_runs r
               on r.account_id = a.account_id and r.status in ('running', 'retryable')
             where r.run_id is not null or s.next_due_at is null or s.next_due_at <= $1
             order by coalesce(s.next_due_at, a.connected_at), a.account_id
             limit $2",
        )
        .bind(now)
        .bind(limit)
        .fetch_all(self.database.pool())
        .await
        .map_err(OwnMediaSyncError::Database)?;
        let mut summary = OwnMediaSyncSummary::default();
        for account_id in account_ids {
            summary.attempted = summary.attempted.saturating_add(1);
            match self.run_account_once(account_id, now).await {
                Ok(OwnMediaSyncOutcome::Completed { .. }) => {
                    summary.completed = summary.completed.saturating_add(1);
                }
                Ok(OwnMediaSyncOutcome::CapabilityNoop { .. }) => {
                    summary.capability_noops = summary.capability_noops.saturating_add(1);
                }
                Ok(OwnMediaSyncOutcome::Retryable { .. }) => {
                    summary.retryable = summary.retryable.saturating_add(1);
                }
                Err(_) => summary.failed = summary.failed.saturating_add(1),
            }
        }
        Ok(summary)
    }
    #[expect(clippy::too_many_lines, reason = "bounded traversal state machine")]
    async fn run_eligible(
        &self,
        account_id: Uuid,
        user_ref: Uuid,
        generation_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<OwnMediaSyncOutcome, OwnMediaSyncError> {
        let account: Option<(String, Vec<u8>)> = sqlx::query_as(
            "select a.provider_account_id, c.access_token_envelope
             from instagram_archive.accounts a
             join instagram_archive.credentials c using (account_id)
             where a.account_id = $1 and a.user_ref = $2 and a.connection_status = 'connected'",
        )
        .bind(account_id)
        .bind(user_ref)
        .fetch_optional(self.database.pool())
        .await
        .map_err(OwnMediaSyncError::Database)?;
        let Some((provider_account_id, envelope)) = account else {
            return Err(OwnMediaSyncError::Unavailable);
        };
        let access_token = self
            .keyring
            .open(
                TokenBinding {
                    subject_id: account_id,
                    kind: TokenKind::Access,
                },
                &envelope,
            )
            .map_err(OwnMediaSyncError::Crypto)?;
        let active: Option<ActiveRunRow> = sqlx::query_as(
            "select run_id, start_watermark_provider_media_id,
                        candidate_watermark_provider_media_id, next_cursor,
                        capability_generation_id
                 from instagram_archive.own_media_sync_runs
                 where account_id = $1 and status in ('running', 'retryable')",
        )
        .bind(account_id)
        .fetch_optional(self.database.pool())
        .await
        .map_err(OwnMediaSyncError::Database)?;
        let (run_id, old_watermark, mut candidate_watermark, mut next_cursor, resumed) =
            if let Some((run_id, old_watermark, candidate, cursor, run_generation)) = active {
                if run_generation != generation_id {
                    return Err(OwnMediaSyncError::Unavailable);
                }
                sqlx::query(
                    "update instagram_archive.own_media_sync_runs
                     set status = 'running', outcome_reason = null, updated_at = $2
                     where run_id = $1 and status = 'retryable'",
                )
                .bind(run_id)
                .bind(now)
                .execute(self.database.pool())
                .await
                .map_err(OwnMediaSyncError::Database)?;
                (run_id, old_watermark, candidate, cursor, true)
            } else {
                let old_watermark: Option<String> = sqlx::query_scalar(
                    "select watermark_provider_media_id
                     from instagram_archive.own_media_sync_state where account_id = $1",
                )
                .bind(account_id)
                .fetch_optional(self.database.pool())
                .await
                .map_err(OwnMediaSyncError::Database)?
                .flatten();
                let run_id = Uuid::now_v7();
                let mut transaction = self
                    .database
                    .pool()
                    .begin()
                    .await
                    .map_err(OwnMediaSyncError::Database)?;
                sqlx::query(
                    "insert into instagram_archive.own_media_sync_runs
                     (run_id, account_id, user_ref, capability_generation_id,
                      start_watermark_provider_media_id, status, started_at, updated_at)
                     values ($1, $2, $3, $4, $5, 'running', $6, $6)",
                )
                .bind(run_id)
                .bind(account_id)
                .bind(user_ref)
                .bind(generation_id)
                .bind(&old_watermark)
                .bind(now)
                .execute(&mut *transaction)
                .await
                .map_err(OwnMediaSyncError::Database)?;
                sqlx::query(
                    "insert into instagram_archive.own_media_sync_items
                     (run_id, provider_media_id, owner_provider_account_id, media_type, permalink,
                      caption, published_at, media_url, thumbnail_url, raw_record_id, observed_at)
                     select $1, i.provider_media_id, i.owner_provider_account_id, i.media_type,
                            i.permalink, i.caption, i.published_at, i.media_url, i.thumbnail_url,
                            i.raw_record_id, i.observed_at
                     from instagram_archive.own_media_authority a
                     join instagram_archive.own_media_sync_items i on i.run_id = a.run_id
                     where a.account_id = $2",
                )
                .bind(run_id)
                .bind(account_id)
                .execute(&mut *transaction)
                .await
                .map_err(OwnMediaSyncError::Database)?;
                transaction
                    .commit()
                    .await
                    .map_err(OwnMediaSyncError::Database)?;
                (run_id, old_watermark, None, None, false)
            };

        let mut budget = if resumed {
            ProviderBudget::resume(
                self.database.clone(),
                run_id,
                Some(account_id),
                self.config.call_budget,
            )
            .await
            .map_err(OwnMediaSyncError::Budget)?
        } else {
            ProviderBudget::new(
                self.database.clone(),
                run_id,
                Some(account_id),
                self.config.call_budget,
            )
        };
        let mut completed = false;
        for _ in 0..self.config.pages_per_run {
            let reservation = match budget.reserve(RequestClass::OwnMediaPage, now).await {
                Ok(reservation) => reservation,
                Err(BudgetError::Exhausted) => {
                    sqlx::query(
                        "update instagram_archive.own_media_sync_runs
                         set status = 'retryable', outcome_reason = 'budget_exhausted', updated_at = $2
                         where run_id = $1 and status = 'running'",
                    )
                    .bind(run_id)
                    .bind(now)
                    .execute(self.database.pool())
                    .await
                    .map_err(OwnMediaSyncError::Database)?;
                    return Ok(OwnMediaSyncOutcome::Retryable {
                        run_id,
                        next_cursor,
                    });
                }
                Err(error) => return Err(OwnMediaSyncError::Budget(error)),
            };
            let result = self
                .provider
                .list_own_media_page(&provider_account_id, &access_token, next_cursor.as_deref())
                .await;
            finish_attempt(&budget, reservation, result.as_ref().err(), now).await?;
            let page = match result {
                Ok(page) => page,
                Err(error) => {
                    mark_provider_failure(self.database, run_id, error, now).await?;
                    return if is_retryable(error.class) {
                        Ok(OwnMediaSyncOutcome::Retryable {
                            run_id,
                            next_cursor,
                        })
                    } else {
                        Err(OwnMediaSyncError::Provider(error))
                    };
                }
            };
            if candidate_watermark.is_none() {
                candidate_watermark = page
                    .items
                    .first()
                    .map(|item| item.provider_media_id.clone());
            }
            let reached_watermark = old_watermark.as_ref().is_some_and(|watermark| {
                page.items
                    .iter()
                    .any(|item| item.provider_media_id == *watermark)
            });
            next_cursor.clone_from(&page.next_cursor);
            persist_page(
                self.database,
                run_id,
                &provider_account_id,
                &page.items,
                &page.raw_body,
                page.next_cursor.as_deref(),
                candidate_watermark.as_deref(),
                now,
            )
            .await?;
            if reached_watermark || (old_watermark.is_none() && next_cursor.is_none()) {
                completed = true;
                break;
            }
            if next_cursor.is_none() {
                break;
            }
        }
        if !completed {
            sqlx::query(
                "update instagram_archive.own_media_sync_runs
                 set status = 'retryable', outcome_reason = 'page_limit', updated_at = $2
                 where run_id = $1 and status = 'running'",
            )
            .bind(run_id)
            .bind(now)
            .execute(self.database.pool())
            .await
            .map_err(OwnMediaSyncError::Database)?;
            return Ok(OwnMediaSyncOutcome::Retryable {
                run_id,
                next_cursor,
            });
        }
        finalize_run(
            self.database,
            run_id,
            account_id,
            user_ref,
            generation_id,
            &provider_account_id,
            candidate_watermark.as_deref(),
            self.config.cadence_seconds,
            now,
        )
        .await?;
        Ok(OwnMediaSyncOutcome::Completed {
            run_id,
            watermark_provider_media_id: candidate_watermark,
        })
    }
}

#[expect(clippy::too_many_arguments, reason = "atomic page evidence commit")]
async fn persist_page(
    database: &Database,
    run_id: Uuid,
    provider_account_id: &str,
    items: &[ProviderOwnMediaItem],
    raw_body: &[u8],
    next_cursor: Option<&str>,
    candidate_watermark: Option<&str>,
    now: OffsetDateTime,
) -> Result<(), OwnMediaSyncError> {
    let digest = Sha256::digest(raw_body);
    let blob_ref = digest
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            use std::fmt::Write as _;
            let _ = write!(hex, "{byte:02x}");
            hex
        });
    let byte_size =
        i64::try_from(raw_body.len()).map_err(|_| OwnMediaSyncError::MalformedMetadata)?;
    let raw_record_id = Uuid::now_v7();
    let mut transaction = database
        .pool()
        .begin()
        .await
        .map_err(OwnMediaSyncError::Database)?;
    sqlx::query(
        "insert into instagram_archive.raw_records
         (raw_record_id, record_kind, blob_ref, content_hash, byte_size, body, observed_at)
         values ($1, 'api_response', $2, $3, $4, $5, $6)",
    )
    .bind(raw_record_id)
    .bind(blob_ref)
    .bind(digest.to_vec())
    .bind(byte_size)
    .bind(raw_body)
    .bind(now)
    .execute(&mut *transaction)
    .await
    .map_err(OwnMediaSyncError::Database)?;
    for item in items {
        if item.owner_provider_account_id != provider_account_id {
            return Err(OwnMediaSyncError::MalformedMetadata);
        }
        let media_type = normalized_media_type(item);
        let published_at = item.published_at;
        sqlx::query(
            "insert into instagram_archive.own_media_sync_items
             (run_id, provider_media_id, owner_provider_account_id, media_type, permalink,
              caption, published_at, media_url, thumbnail_url, raw_record_id, observed_at)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
             on conflict (run_id, provider_media_id) do update set
               owner_provider_account_id = excluded.owner_provider_account_id,
               media_type = excluded.media_type, permalink = excluded.permalink,
               caption = excluded.caption, published_at = excluded.published_at,
               media_url = excluded.media_url, thumbnail_url = excluded.thumbnail_url,
               raw_record_id = excluded.raw_record_id, observed_at = excluded.observed_at",
        )
        .bind(run_id)
        .bind(&item.provider_media_id)
        .bind(&item.owner_provider_account_id)
        .bind(media_type)
        .bind(&item.permalink)
        .bind(&item.caption)
        .bind(published_at)
        .bind(&item.media_url)
        .bind(&item.thumbnail_url)
        .bind(raw_record_id)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(OwnMediaSyncError::Database)?;
    }
    let item_count =
        i64::try_from(items.len()).map_err(|_| OwnMediaSyncError::MalformedMetadata)?;
    sqlx::query(
        "update instagram_archive.own_media_sync_runs
         set next_cursor = $2,
             candidate_watermark_provider_media_id = coalesce(candidate_watermark_provider_media_id, $3),
             page_count = page_count + 1, item_count = item_count + $4, updated_at = $5
         where run_id = $1 and status = 'running'",
    )
    .bind(run_id)
    .bind(next_cursor)
    .bind(candidate_watermark)
    .bind(item_count)
    .bind(now)
    .execute(&mut *transaction)
    .await
    .map_err(OwnMediaSyncError::Database)?;
    transaction
        .commit()
        .await
        .map_err(OwnMediaSyncError::Database)
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "atomic authority transaction"
)]
async fn finalize_run(
    database: &Database,
    run_id: Uuid,
    account_id: Uuid,
    user_ref: Uuid,
    generation_id: Uuid,
    provider_account_id: &str,
    candidate_watermark: Option<&str>,
    cadence_seconds: u64,
    now: OffsetDateTime,
) -> Result<(), OwnMediaSyncError> {
    let cadence =
        i64::try_from(cadence_seconds).map_err(|_| OwnMediaSyncError::MalformedMetadata)?;
    let next_due_at = now + time::Duration::seconds(cadence);
    let mut transaction = database
        .pool()
        .begin()
        .await
        .map_err(OwnMediaSyncError::Database)?;
    let current: Option<(Uuid, String, String, Uuid, String)> = sqlx::query_as(
        "select a.user_ref, a.provider_account_id, a.connection_status,
                c.generation_id, c.capability_state
         from instagram_archive.accounts a
         join instagram_archive.account_capabilities c on c.account_id = a.account_id
         where a.account_id = $1 and c.capability = 'own_media_read' for update of a, c",
    )
    .bind(account_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(OwnMediaSyncError::Database)?;
    if current.as_ref().is_none_or(|current| {
        current.0 != user_ref
            || current.1 != provider_account_id
            || current.2 != "connected"
            || current.3 != generation_id
            || current.4 != "available"
    }) {
        sqlx::query(
            "update instagram_archive.own_media_sync_runs
             set status = 'failed', outcome_reason = 'capability_changed',
                 updated_at = $2, finished_at = $2 where run_id = $1",
        )
        .bind(run_id)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(OwnMediaSyncError::Database)?;
        transaction
            .commit()
            .await
            .map_err(OwnMediaSyncError::Database)?;
        return Err(OwnMediaSyncError::Unavailable);
    }
    let staged: Vec<(String, String, String, Option<String>, OffsetDateTime, Uuid)> =
        sqlx::query_as(
            "select provider_media_id, permalink, media_type, caption, published_at, raw_record_id
         from instagram_archive.own_media_sync_items where run_id = $1 order by provider_media_id",
        )
        .bind(run_id)
        .fetch_all(&mut *transaction)
        .await
        .map_err(OwnMediaSyncError::Database)?;
    for (provider_media_id, permalink, media_type, caption, published_at, raw_record_id) in staged {
        let media_id: Uuid = if let Some(media_id) = sqlx::query_scalar(
            "select media_id from instagram_archive.media where provider_media_id = $1",
        )
        .bind(&provider_media_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(OwnMediaSyncError::Database)?
        {
            media_id
        } else {
            let media_id = Uuid::now_v7();
            sqlx::query(
                "insert into instagram_archive.media
                 (media_id, account_id, provider_media_id, permalink, media_type, caption,
                  published_at, acquisition_method, saved_authority, upstream_status)
                 values ($1, $2, $3, $4, $5, $6, $7, 'official_api',
                         'authoritative_platform_state', 'available')",
            )
            .bind(media_id)
            .bind(account_id)
            .bind(&provider_media_id)
            .bind(&permalink)
            .bind(&media_type)
            .bind(&caption)
            .bind(published_at)
            .execute(&mut *transaction)
            .await
            .map_err(OwnMediaSyncError::Database)?;
            media_id
        };
        sqlx::query(
            "update instagram_archive.media set account_id = $2, permalink = $3,
             media_type = $4, caption = $5, published_at = $6,
             acquisition_method = 'official_api',
             saved_authority = 'authoritative_platform_state', upstream_status = 'available',
             updated_at = $7 where media_id = $1",
        )
        .bind(media_id)
        .bind(account_id)
        .bind(&permalink)
        .bind(&media_type)
        .bind(&caption)
        .bind(published_at)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(OwnMediaSyncError::Database)?;
        let revision_id = Uuid::now_v7();
        sqlx::query(
            "insert into instagram_archive.media_revisions
             (revision_id, media_id, raw_record_id, parser_version, resolved_at)
             values ($1, $2, $3, 'official-own-media-v1', $4)",
        )
        .bind(revision_id)
        .bind(media_id)
        .bind(raw_record_id)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(OwnMediaSyncError::Database)?;
        sqlx::query(
            "update instagram_archive.media set current_revision_id = $2 where media_id = $1",
        )
        .bind(media_id)
        .bind(revision_id)
        .execute(&mut *transaction)
        .await
        .map_err(OwnMediaSyncError::Database)?;
    }
    crate::publishing::append_own_media_facts(
        &mut transaction,
        run_id,
        user_ref,
        candidate_watermark,
        now,
    )
    .await
    .map_err(OwnMediaSyncError::Publish)?;
    sqlx::query(
        "insert into instagram_archive.own_media_authority (account_id, run_id, activated_at)
         values ($1, $2, $3) on conflict (account_id) do update
         set run_id = excluded.run_id, activated_at = excluded.activated_at",
    )
    .bind(account_id)
    .bind(run_id)
    .bind(now)
    .execute(&mut *transaction)
    .await
    .map_err(OwnMediaSyncError::Database)?;
    sqlx::query(
        "insert into instagram_archive.own_media_sync_state
         (account_id, watermark_provider_media_id, next_due_at, last_run_id, last_outcome, updated_at)
         values ($1, $2, $3, $4, 'completed', $5)
         on conflict (account_id) do update set
           watermark_provider_media_id = excluded.watermark_provider_media_id,
           next_due_at = excluded.next_due_at, last_run_id = excluded.last_run_id,
           last_outcome = excluded.last_outcome, updated_at = excluded.updated_at",
    )
    .bind(account_id)
    .bind(candidate_watermark)
    .bind(next_due_at)
    .bind(run_id)
    .bind(now)
    .execute(&mut *transaction)
    .await
    .map_err(OwnMediaSyncError::Database)?;
    sqlx::query(
        "update instagram_archive.own_media_sync_runs set status = 'completed',
         outcome_reason = 'completed', next_cursor = null, updated_at = $2, finished_at = $2
         where run_id = $1 and status = 'running'",
    )
    .bind(run_id)
    .bind(now)
    .execute(&mut *transaction)
    .await
    .map_err(OwnMediaSyncError::Database)?;
    transaction
        .commit()
        .await
        .map_err(OwnMediaSyncError::Database)
}

fn normalized_media_type(item: &ProviderOwnMediaItem) -> &'static str {
    match (item.media_type.as_str(), item.media_product_type.as_str()) {
        ("VIDEO", "REELS") => "reel",
        ("IMAGE", _) => "image",
        ("VIDEO", _) => "video",
        ("CAROUSEL_ALBUM", _) => "carousel",
        _ => "unknown",
    }
}

async fn finish_attempt(
    budget: &ProviderBudget,
    reservation: crate::provider_budget::UsageReservation,
    error: Option<&ProviderError>,
    now: OffsetDateTime,
) -> Result<(), OwnMediaSyncError> {
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
        .map_err(OwnMediaSyncError::Budget)
}

const fn is_retryable(class: ProviderFailureClass) -> bool {
    matches!(
        class,
        ProviderFailureClass::Network
            | ProviderFailureClass::RateLimited
            | ProviderFailureClass::Server
    )
}

async fn mark_provider_failure(
    database: &Database,
    run_id: Uuid,
    error: ProviderError,
    now: OffsetDateTime,
) -> Result<(), OwnMediaSyncError> {
    let (status, reason, finished_at) = if is_retryable(error.class) {
        ("retryable", "provider_retryable", None)
    } else {
        ("failed", "response_refused", Some(now))
    };
    sqlx::query(
        "update instagram_archive.own_media_sync_runs
         set status = $2, outcome_reason = $3, updated_at = $4, finished_at = $5
         where run_id = $1 and status = 'running'",
    )
    .bind(run_id)
    .bind(status)
    .bind(reason)
    .bind(now)
    .bind(finished_at)
    .execute(database.pool())
    .await
    .map_err(OwnMediaSyncError::Database)?;
    Ok(())
}
