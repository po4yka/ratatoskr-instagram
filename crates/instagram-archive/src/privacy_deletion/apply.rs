//! Atomic SQL erasure, shared-holding decisions, and durable blob scheduling.

use super::{
    DeletionRequest, DeletionTarget, PersistenceError, PrivacyDeletionError, Uuid,
    append_removal_fact, append_source_removal_fact, instant_from_time, own_media_source_identity,
    source_identity,
};

#[expect(
    clippy::too_many_lines,
    reason = "the transaction stays linear so erasure, audit, guard, blob work, and outbox atomicity are auditable"
)]
pub(super) async fn apply_target_rows(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    request: DeletionRequest,
) -> Result<u64, PrivacyDeletionError> {
    let DeletionTarget::Capture(capture_id) = request.target else {
        return apply_connection_rows(transaction, request).await;
    };
    let (owner, canonical_url, media_id): (Uuid, String, Option<Uuid>) = sqlx::query_as(
        "select user_ref, canonical_url, media_id from instagram_archive.captures \
         where capture_id = $1",
    )
    .bind(capture_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    let owner_holds_source: bool = match media_id {
        Some(media_id) => sqlx::query_scalar(
            "select exists(select 1 from instagram_archive.captures \
             where user_ref = $1 and media_id = $2 and capture_id <> $3)",
        )
        .bind(owner)
        .bind(media_id)
        .bind(capture_id)
        .fetch_one(&mut **transaction)
        .await
        .map_err(PersistenceError::Query)?,
        None => sqlx::query_scalar(
            "select exists(select 1 from instagram_archive.captures \
             where user_ref = $1 and canonical_url = $2 and capture_id <> $3)",
        )
        .bind(owner)
        .bind(&canonical_url)
        .bind(capture_id)
        .fetch_one(&mut **transaction)
        .await
        .map_err(PersistenceError::Query)?,
    };
    if !owner_holds_source {
        let removed_at = time::OffsetDateTime::now_utc();
        append_removal_fact(
            transaction,
            capture_id,
            ratatoskr_social_contracts::RemovalReason::UserRequested,
            instant_from_time(removed_at, capture_id)?,
        )
        .await?;
        sqlx::query(
            "delete from instagram_archive.outbox_events \
             where aggregate_type = 'capture' and aggregate_id = $1 \
               and event_type <> 'social.source.removed.v1'",
        )
        .bind(capture_id)
        .execute(&mut **transaction)
        .await
        .map_err(PersistenceError::Query)?;
        sqlx::query(
            "insert into instagram_archive.local_source_removals \
             (user_ref, social_source_id, capture_id, operation_id, reason, removed_at) \
             values ($1, $2, $3, $4, 'user_requested', $5) \
             on conflict (user_ref, social_source_id) do nothing",
        )
        .bind(owner)
        .bind(source_identity(owner, &canonical_url))
        .bind(capture_id)
        .bind(request.operation_id)
        .bind(removed_at)
        .execute(&mut **transaction)
        .await
        .map_err(PersistenceError::Query)?;
    }
    for statement in [
        "delete from instagram_archive.capture_analysis_links where capture_id = $1",
        "delete from instagram_archive.capture_notes where capture_id = $1",
        "delete from instagram_archive.reresolution_items where capture_id = $1",
        "delete from instagram_archive.availability_observations where capture_id = $1",
    ] {
        sqlx::query(statement)
            .bind(capture_id)
            .execute(&mut **transaction)
            .await
            .map_err(PersistenceError::Query)?;
    }
    sqlx::query("delete from instagram_archive.captures where capture_id = $1")
        .bind(capture_id)
        .execute(&mut **transaction)
        .await
        .map_err(PersistenceError::Query)?;
    let Some(media_id) = media_id else {
        return Ok(0);
    };
    let independent_holding: bool = sqlx::query_scalar(
        "select exists(select 1 from instagram_archive.captures where media_id = $1) \
         or exists(select 1 from instagram_archive.media \
                   where media_id = $1 and (account_id is not null or acquisition_method = 'data_export'))",
    )
    .bind(media_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    if independent_holding {
        return Ok(0);
    }

    let media_blob: Option<(String, Vec<u8>, i64)> = sqlx::query_as(
        "select blob_ref, content_hash, byte_size from instagram_archive.media \
         where media_id = $1 and blob_ref is not null",
    )
    .bind(media_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    let raw_blobs: Vec<(Uuid, String, Vec<u8>, i64)> = sqlx::query_as(
        "select distinct r.raw_record_id, r.blob_ref, r.content_hash, r.byte_size \
         from instagram_archive.media_revisions revision \
         join instagram_archive.raw_records r on r.raw_record_id = revision.raw_record_id \
         where revision.media_id = $1 order by r.raw_record_id",
    )
    .bind(media_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    sqlx::query(
        "delete from instagram_archive.media_relations \
         where parent_media_id = $1 or child_media_id = $1",
    )
    .bind(media_id)
    .execute(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    sqlx::query("delete from instagram_archive.availability_observations where media_id = $1")
        .bind(media_id)
        .execute(&mut **transaction)
        .await
        .map_err(PersistenceError::Query)?;
    sqlx::query(
        "update instagram_archive.media set current_revision_id = null where media_id = $1",
    )
    .bind(media_id)
    .execute(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    sqlx::query("delete from instagram_archive.media_revisions where media_id = $1")
        .bind(media_id)
        .execute(&mut **transaction)
        .await
        .map_err(PersistenceError::Query)?;
    sqlx::query("delete from instagram_archive.media where media_id = $1")
        .bind(media_id)
        .execute(&mut **transaction)
        .await
        .map_err(PersistenceError::Query)?;

    let mut pending = 0_u64;
    if let Some((blob_ref, content_hash, byte_size)) = media_blob {
        schedule_blob_task(
            transaction,
            request.operation_id,
            blob_ref,
            content_hash,
            byte_size,
            "provider_media",
        )
        .await?;
        pending += 1;
    }
    for (raw_id, blob_ref, content_hash, byte_size) in raw_blobs {
        let retained: bool = sqlx::query_scalar(
            "select exists(select 1 from instagram_archive.media_revisions where raw_record_id = $1)",
        )
        .bind(raw_id)
        .fetch_one(&mut **transaction)
        .await
        .map_err(PersistenceError::Query)?;
        if retained {
            continue;
        }
        sqlx::query("delete from instagram_archive.raw_records where raw_record_id = $1")
            .bind(raw_id)
            .execute(&mut **transaction)
            .await
            .map_err(PersistenceError::Query)?;
        schedule_blob_task(
            transaction,
            request.operation_id,
            blob_ref,
            content_hash,
            byte_size,
            "raw_response",
        )
        .await?;
        pending += 1;
    }
    Ok(pending)
}

#[expect(
    clippy::too_many_lines,
    reason = "the transaction keeps the complete inventory and shared-holding decisions together"
)]
async fn apply_connection_rows(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    request: DeletionRequest,
) -> Result<u64, PrivacyDeletionError> {
    let DeletionTarget::Connection(account_id) = request.target else {
        return Ok(0);
    };
    let raw_ids: Vec<Uuid> = sqlx::query_scalar(
        "select distinct raw_record_id from (\
           select raw_record_id from instagram_archive.account_permission_observations \
            where account_id = $1 \
           union all \
           select raw_record_id from instagram_archive.profiles \
            where account_id = $1 and raw_record_id is not null \
           union all \
           select item.raw_record_id from instagram_archive.own_media_sync_items item \
            join instagram_archive.own_media_sync_runs run on run.run_id = item.run_id \
            where run.account_id = $1\
         ) raw_ids order by raw_record_id",
    )
    .bind(account_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;

    let media_rows: Vec<(Uuid, Option<String>, String)> = sqlx::query_as(
        "select media_id, provider_media_id, permalink from instagram_archive.media \
         where account_id = $1 order by media_id",
    )
    .bind(account_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    let mut pending = 0_u64;
    for (media_id, provider_media_id, permalink) in media_rows {
        let capture_holding: bool = sqlx::query_scalar(
            "select exists(select 1 from instagram_archive.captures where media_id = $1)",
        )
        .bind(media_id)
        .fetch_one(&mut **transaction)
        .await
        .map_err(PersistenceError::Query)?;
        if capture_holding {
            sqlx::query(
                "update instagram_archive.media set account_id = null, updated_at = now() \
                 where media_id = $1",
            )
            .bind(media_id)
            .execute(&mut **transaction)
            .await
            .map_err(PersistenceError::Query)?;
            continue;
        }
        let source_id = provider_media_id.as_deref().map_or_else(
            || source_identity(request.user_ref, &permalink),
            |provider_id| own_media_source_identity(request.user_ref, provider_id),
        );
        let removed_at = time::OffsetDateTime::now_utc();
        append_source_removal_fact(
            transaction,
            request.user_ref,
            source_id,
            "media",
            media_id,
            ratatoskr_social_contracts::RemovalReason::UserRequested,
            instant_from_time(removed_at, media_id)?,
        )
        .await?;
        sqlx::query(
            "delete from instagram_archive.outbox_events \
             where aggregate_type = 'media' and aggregate_id = $1 \
               and event_type <> 'social.source.removed.v1'",
        )
        .bind(media_id)
        .execute(&mut **transaction)
        .await
        .map_err(PersistenceError::Query)?;
        sqlx::query(
            "insert into instagram_archive.local_source_removals \
             (user_ref, social_source_id, media_id, operation_id, reason, removed_at) \
             values ($1, $2, $3, $4, 'user_requested', $5) \
             on conflict (user_ref, social_source_id) do nothing",
        )
        .bind(request.user_ref)
        .bind(source_id)
        .bind(media_id)
        .bind(request.operation_id)
        .bind(removed_at)
        .execute(&mut **transaction)
        .await
        .map_err(PersistenceError::Query)?;
        pending +=
            erase_exclusive_connection_media(transaction, request.operation_id, media_id).await?;
    }

    for statement in [
        "delete from instagram_archive.own_media_authority where account_id = $1",
        "delete from instagram_archive.own_media_sync_state where account_id = $1",
    ] {
        sqlx::query(statement)
            .bind(account_id)
            .execute(&mut **transaction)
            .await
            .map_err(PersistenceError::Query)?;
    }
    sqlx::query(
        "delete from instagram_archive.own_media_sync_items where run_id in \
         (select run_id from instagram_archive.own_media_sync_runs where account_id = $1)",
    )
    .bind(account_id)
    .execute(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    for statement in [
        "delete from instagram_archive.own_media_sync_runs where account_id = $1",
        "delete from instagram_archive.account_capabilities where account_id = $1",
        "delete from instagram_archive.account_permission_observations where account_id = $1",
        "delete from instagram_archive.profiles where account_id = $1",
        "delete from instagram_archive.provider_api_usage where account_id = $1",
        "delete from instagram_archive.oauth_flows where account_id = $1",
        "delete from instagram_archive.credentials where account_id = $1",
        "delete from instagram_archive.account_credential_audit where account_id = $1",
    ] {
        sqlx::query(statement)
            .bind(account_id)
            .execute(&mut **transaction)
            .await
            .map_err(PersistenceError::Query)?;
    }

    for raw_id in raw_ids {
        let raw: Option<(String, Vec<u8>, i64)> = sqlx::query_as(
            "select blob_ref, content_hash, byte_size from instagram_archive.raw_records r \
             where raw_record_id = $1 \
               and not exists (select 1 from instagram_archive.media_revisions revision \
                               where revision.raw_record_id = r.raw_record_id)",
        )
        .bind(raw_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(PersistenceError::Query)?;
        if let Some((blob_ref, content_hash, byte_size)) = raw {
            sqlx::query("delete from instagram_archive.raw_records where raw_record_id = $1")
                .bind(raw_id)
                .execute(&mut **transaction)
                .await
                .map_err(PersistenceError::Query)?;
            schedule_blob_task(
                transaction,
                request.operation_id,
                blob_ref,
                content_hash,
                byte_size,
                "raw_response",
            )
            .await?;
            pending += 1;
        }
    }
    sqlx::query("delete from instagram_archive.accounts where account_id = $1")
        .bind(account_id)
        .execute(&mut **transaction)
        .await
        .map_err(PersistenceError::Query)?;
    Ok(pending)
}

async fn erase_exclusive_connection_media(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operation_id: Uuid,
    media_id: Uuid,
) -> Result<u64, PrivacyDeletionError> {
    let media_blob: Option<(String, Vec<u8>, i64)> = sqlx::query_as(
        "select blob_ref, content_hash, byte_size from instagram_archive.media \
         where media_id = $1 and blob_ref is not null",
    )
    .bind(media_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    let raw_blobs: Vec<(Uuid, String, Vec<u8>, i64)> = sqlx::query_as(
        "select distinct r.raw_record_id, r.blob_ref, r.content_hash, r.byte_size \
         from instagram_archive.media_revisions revision \
         join instagram_archive.raw_records r on r.raw_record_id = revision.raw_record_id \
         where revision.media_id = $1 order by r.raw_record_id",
    )
    .bind(media_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    for statement in [
        "delete from instagram_archive.media_relations where parent_media_id = $1 or child_media_id = $1",
        "delete from instagram_archive.availability_observations where media_id = $1",
        "delete from instagram_archive.own_media_authority where account_id = (select account_id from instagram_archive.media where media_id = $1)",
    ] {
        sqlx::query(statement)
            .bind(media_id)
            .execute(&mut **transaction)
            .await
            .map_err(PersistenceError::Query)?;
    }
    sqlx::query(
        "update instagram_archive.media set current_revision_id = null where media_id = $1",
    )
    .bind(media_id)
    .execute(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    sqlx::query("delete from instagram_archive.media_revisions where media_id = $1")
        .bind(media_id)
        .execute(&mut **transaction)
        .await
        .map_err(PersistenceError::Query)?;
    sqlx::query("delete from instagram_archive.media where media_id = $1")
        .bind(media_id)
        .execute(&mut **transaction)
        .await
        .map_err(PersistenceError::Query)?;
    let mut pending = 0_u64;
    if let Some((blob_ref, content_hash, byte_size)) = media_blob {
        schedule_blob_task(
            transaction,
            operation_id,
            blob_ref,
            content_hash,
            byte_size,
            "provider_media",
        )
        .await?;
        pending += 1;
    }
    for (raw_id, blob_ref, content_hash, byte_size) in raw_blobs {
        let referenced: bool = sqlx::query_scalar(
            "select exists(select 1 from instagram_archive.media_revisions where raw_record_id = $1)",
        )
        .bind(raw_id)
        .fetch_one(&mut **transaction)
        .await
        .map_err(PersistenceError::Query)?;
        if !referenced {
            sqlx::query("delete from instagram_archive.raw_records where raw_record_id = $1")
                .bind(raw_id)
                .execute(&mut **transaction)
                .await
                .map_err(PersistenceError::Query)?;
            schedule_blob_task(
                transaction,
                operation_id,
                blob_ref,
                content_hash,
                byte_size,
                "raw_response",
            )
            .await?;
            pending += 1;
        }
    }
    Ok(pending)
}

async fn schedule_blob_task(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operation_id: Uuid,
    blob_ref: String,
    content_hash: Vec<u8>,
    byte_size: i64,
    media_class: &'static str,
) -> Result<(), PrivacyDeletionError> {
    sqlx::query(
        "insert into instagram_archive.blob_deletion_tasks \
         (task_id, operation_id, blob_ref, content_hash, byte_size, media_class, state) \
         values ($1, $2, $3, $4, $5, $6, 'pending') \
         on conflict (blob_ref, content_hash) do nothing",
    )
    .bind(Uuid::now_v7())
    .bind(operation_id)
    .bind(blob_ref)
    .bind(content_hash)
    .bind(byte_size)
    .bind(media_class)
    .execute(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    Ok(())
}
