//! Explicit, fail-closed policy boundary for provider-media byte archival.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use sha2::{Digest as _, Sha256};
use tokio::io::AsyncWriteExt as _;

use crate::{Database, PersistenceError};

/// MIME values eligible for provider-media archival.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovedMediaMime {
    /// JPEG image bytes.
    ImageJpeg,
    /// PNG image bytes.
    ImagePng,
    /// MP4 video bytes.
    VideoMp4,
}

impl ApprovedMediaMime {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ImageJpeg => "image/jpeg",
            Self::ImagePng => "image/png",
            Self::VideoMp4 => "video/mp4",
        }
    }
}

/// One acquired response together with its immutable lease evidence.
#[derive(Debug)]
pub struct AcquiredMedia<'a> {
    /// Final URL after redirects.
    pub final_url: &'a str,
    /// MIME authorized before I/O.
    pub expected_mime: ApprovedMediaMime,
    /// MIME returned by the final response.
    pub observed_content_type: &'a str,
    /// Response byte length declared by the transport.
    pub declared_bytes: u64,
    /// Digest authorized by the caller's immutable observation.
    pub expected_digest: [u8; 32],
    /// Bounded transport-acquired body.
    pub body: &'a [u8],
}

/// A post-fetch verification refusal that keeps the record metadata-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaVerificationReason {
    /// The final URL is not HTTPS.
    FinalUrlNotHttps,
    /// The response MIME differs from the authorized type.
    MimeMismatch,
    /// Actual bytes disagree with the response length.
    ContentLengthMismatch,
    /// Actual bytes disagree with the expected digest.
    ContentDigestMismatch,
    /// Actual bytes exceed the immutable fetch lease.
    ResponseBudgetExceeded,
}

/// Verified media storage evidence safe to attach to a media row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivedMedia {
    /// Service-owned content-addressed reference.
    pub blob_ref: String,
    /// SHA-256 digest bytes.
    pub content_hash: Vec<u8>,
    /// Verified byte length.
    pub byte_size: i64,
    /// Approved MIME.
    pub media_type: &'static str,
}

/// Result of verifying and attempting to archive one acquired response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaArchiveOutcome {
    /// Verification refused the bytes without a durable partial object.
    MetadataOnly(MediaVerificationReason),
    /// Fully verified immutable bytes were stored.
    Archived(ArchivedMedia),
}

/// Reference-safe cleanup decision for one expiring media row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlobCleanupPlan {
    /// Another database-wide live reference still requires the object.
    RetainShared {
        /// References excluding the expiring media row.
        live_references: i64,
    },
    /// No live reference remains, so durable deletion may be scheduled.
    ScheduleDelete {
        /// Digest-bound service-owned reference.
        blob_ref: String,
        /// Expected digest used by exact deletion.
        content_hash: Vec<u8>,
    },
}

/// Safe bounded failure vocabulary for post-commit `BlobStore` deletion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobDeletionFailure {
    /// The service-owned storage path could not be read or changed.
    StorageUnavailable,
    /// The object bytes did not match the task's expected digest or length.
    DigestMismatch,
    /// A database-wide live reference still requires the object.
    StillReferenced,
}

impl BlobDeletionFailure {
    const fn as_str(self) -> &'static str {
        match self {
            Self::StorageUnavailable => "storage_unavailable",
            Self::DigestMismatch => "digest_mismatch",
            Self::StillReferenced => "still_referenced",
        }
    }
}

impl BlobDeletionBackend for MediaBlobStore {
    fn delete_if_matches<'a>(
        &'a self,
        blob_ref: &'a str,
        content_hash: &'a [u8],
        byte_size: i64,
    ) -> Pin<Box<dyn Future<Output = Result<(), BlobDeletionFailure>> + Send + 'a>> {
        Box::pin(async move {
            let digest_hex = hex_encode(content_hash);
            let expected_ref = format!("instagram-archive/media/sha256/{digest_hex}");
            if blob_ref != expected_ref || byte_size < 0 {
                return Err(BlobDeletionFailure::DigestMismatch);
            }
            let path = self.root.join("sha256").join(&digest_hex);
            if !tokio::fs::try_exists(&path)
                .await
                .map_err(|_| BlobDeletionFailure::StorageUnavailable)?
            {
                return Ok(());
            }
            let bytes = tokio::fs::read(&path)
                .await
                .map_err(|_| BlobDeletionFailure::StorageUnavailable)?;
            if i64::try_from(bytes.len()).ok() != Some(byte_size)
                || Sha256::digest(&bytes).as_slice() != content_hash
            {
                return Err(BlobDeletionFailure::DigestMismatch);
            }
            tokio::fs::remove_file(&path)
                .await
                .map_err(|_| BlobDeletionFailure::StorageUnavailable)?;
            if tokio::fs::try_exists(&path)
                .await
                .map_err(|_| BlobDeletionFailure::StorageUnavailable)?
            {
                return Err(BlobDeletionFailure::StorageUnavailable);
            }
            Ok(())
        })
    }
}

/// Async digest-and-length-bound `BlobStore` deletion seam.
pub trait BlobDeletionBackend {
    /// Deletes the object only when its current bytes match the expected evidence.
    fn delete_if_matches<'a>(
        &'a self,
        blob_ref: &'a str,
        content_hash: &'a [u8],
        byte_size: i64,
    ) -> Pin<Box<dyn Future<Output = Result<(), BlobDeletionFailure>> + Send + 'a>>;
}

/// Durable state returned by one idempotent blob-task attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobDeletionTaskOutcome {
    /// The task remains durable for retry.
    Pending(BlobDeletionFailure),
    /// The object is verified absent and the task is terminal.
    Complete,
}

/// Processes one durable `BlobStore` deletion task.
///
/// # Errors
///
/// Returns a persistence failure when task state cannot be loaded or advanced.
pub async fn process_blob_deletion_task<B>(
    database: &Database,
    backend: &B,
    task_id: uuid::Uuid,
) -> Result<BlobDeletionTaskOutcome, PersistenceError>
where
    B: BlobDeletionBackend + Sync,
{
    let (blob_ref, content_hash, byte_size, state, operation_id): (
        String,
        Vec<u8>,
        i64,
        String,
        Option<uuid::Uuid>,
    ) = sqlx::query_as(
        "select blob_ref, content_hash, byte_size, state, operation_id \
             from instagram_archive.blob_deletion_tasks where task_id = $1",
    )
    .bind(task_id)
    .fetch_one(database.pool())
    .await
    .map_err(PersistenceError::Query)?;
    if state == "complete" {
        return Ok(BlobDeletionTaskOutcome::Complete);
    }
    let (live_references,): (i64,) = sqlx::query_as(
        "select coalesce(sum(reference_count), 0)::bigint from (\
           select count(*)::bigint as reference_count from instagram_archive.media \
            where blob_ref = $1 and content_hash = $2 and byte_size = $3 \
           union all \
           select count(*)::bigint from instagram_archive.raw_records \
            where blob_ref = $1 and content_hash = $2 and byte_size = $3 \
           union all \
           select count(*)::bigint from instagram_archive.export_snapshots \
            where archive_blob_ref = $1 and archive_hash = $2 and archive_byte_size = $3\
         ) as live_references",
    )
    .bind(&blob_ref)
    .bind(&content_hash)
    .bind(byte_size)
    .fetch_one(database.pool())
    .await
    .map_err(PersistenceError::Query)?;
    if live_references > 0 {
        record_blob_failure(
            database,
            task_id,
            operation_id,
            BlobDeletionFailure::StillReferenced,
        )
        .await?;
        return Ok(BlobDeletionTaskOutcome::Pending(
            BlobDeletionFailure::StillReferenced,
        ));
    }
    if let Err(failure) = backend
        .delete_if_matches(&blob_ref, &content_hash, byte_size)
        .await
    {
        record_blob_failure(database, task_id, operation_id, failure).await?;
        return Ok(BlobDeletionTaskOutcome::Pending(failure));
    }
    sqlx::query(
        "update instagram_archive.blob_deletion_tasks \
         set state = 'complete', attempt_count = attempt_count + 1, last_failure_class = null, \
             completed_at = now(), updated_at = now() where task_id = $1",
    )
    .bind(task_id)
    .execute(database.pool())
    .await
    .map_err(PersistenceError::Query)?;
    if let Some(operation_id) = operation_id {
        sqlx::query(
            "update instagram_archive.deletion_operations d \
             set state = 'complete', updated_at = now(), finished_at = now() \
             where d.operation_id = $1 \
               and not exists (select 1 from instagram_archive.blob_deletion_tasks t \
                               where t.operation_id = d.operation_id and t.state = 'pending')",
        )
        .bind(operation_id)
        .execute(database.pool())
        .await
        .map_err(PersistenceError::Query)?;
    }
    Ok(BlobDeletionTaskOutcome::Complete)
}

async fn record_blob_failure(
    database: &Database,
    task_id: uuid::Uuid,
    operation_id: Option<uuid::Uuid>,
    failure: BlobDeletionFailure,
) -> Result<(), PersistenceError> {
    sqlx::query(
        "update instagram_archive.blob_deletion_tasks \
         set attempt_count = attempt_count + 1, last_failure_class = $2, updated_at = now() \
         where task_id = $1 and state = 'pending'",
    )
    .bind(task_id)
    .bind(failure.as_str())
    .execute(database.pool())
    .await
    .map_err(PersistenceError::Query)?;
    if let Some(operation_id) = operation_id {
        sqlx::query(
            "update instagram_archive.deletion_operations \
             set state = 'pending_blob_deletion', updated_at = now() where operation_id = $1",
        )
        .bind(operation_id)
        .execute(database.pool())
        .await
        .map_err(PersistenceError::Query)?;
    }
    Ok(())
}

/// Plans expiry of one archived media reference without mutating durable state.
///
/// # Errors
///
/// Returns a typed persistence failure when the row cannot be loaded.
pub async fn plan_media_reference_expiry(
    database: &Database,
    media_id: uuid::Uuid,
) -> Result<BlobCleanupPlan, PersistenceError> {
    let (blob_ref, content_hash): (String, Vec<u8>) = sqlx::query_as(
        "select blob_ref, content_hash from instagram_archive.media where media_id = $1",
    )
    .bind(media_id)
    .fetch_one(database.pool())
    .await
    .map_err(PersistenceError::Query)?;
    let (live_references,): (i64,) = sqlx::query_as(
        "select coalesce(sum(reference_count), 0)::bigint from (\
           select count(*)::bigint as reference_count from instagram_archive.media \
            where blob_ref = $1 and content_hash = $2 and media_id <> $3 \
           union all \
           select count(*)::bigint from instagram_archive.raw_records \
            where blob_ref = $1 and content_hash = $2 \
           union all \
           select count(*)::bigint from instagram_archive.export_snapshots \
            where archive_blob_ref = $1 and archive_hash = $2\
         ) as live_references",
    )
    .bind(&blob_ref)
    .bind(&content_hash)
    .bind(media_id)
    .fetch_one(database.pool())
    .await
    .map_err(PersistenceError::Query)?;
    if live_references > 0 {
        return Ok(BlobCleanupPlan::RetainShared { live_references });
    }
    Ok(BlobCleanupPlan::ScheduleDelete {
        blob_ref,
        content_hash,
    })
}

/// A private, service-owned content-addressed media store.
#[derive(Debug, Clone)]
pub struct MediaBlobStore {
    root: PathBuf,
}

impl MediaBlobStore {
    /// Creates a media store rooted at a private service-owned directory.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

/// Why verified provider-media storage failed.
#[derive(Debug, thiserror::Error)]
pub enum MediaStoreError {
    /// The response was too large for durable integer fields.
    #[error("media response size exceeds supported storage")]
    SizeOverflow,
    /// The private object store could not be read or written.
    #[error("media object storage failed")]
    Storage(#[source] std::io::Error),
    /// Existing bytes at a digest path disagree with that digest.
    #[error("media object disagrees with its content digest")]
    DigestConflict,
}

/// Verifies and stores one response within its immutable fetch lease.
///
/// # Errors
///
/// Returns a bounded storage failure when the private object store fails.
pub async fn archive_acquired_media(
    store: &MediaBlobStore,
    lease: MediaFetchLease,
    response: AcquiredMedia<'_>,
) -> Result<MediaArchiveOutcome, MediaStoreError> {
    match reqwest::Url::parse(response.final_url) {
        Ok(url) if url.scheme() == "https" => {}
        _ => {
            return Ok(MediaArchiveOutcome::MetadataOnly(
                MediaVerificationReason::FinalUrlNotHttps,
            ));
        }
    }
    let observed_mime = response
        .observed_content_type
        .split(';')
        .next()
        .map(str::trim);
    if observed_mime != Some(response.expected_mime.as_str()) {
        return Ok(MediaArchiveOutcome::MetadataOnly(
            MediaVerificationReason::MimeMismatch,
        ));
    }
    let actual_bytes =
        u64::try_from(response.body.len()).map_err(|_| MediaStoreError::SizeOverflow)?;
    if actual_bytes > lease.max_bytes {
        return Ok(MediaArchiveOutcome::MetadataOnly(
            MediaVerificationReason::ResponseBudgetExceeded,
        ));
    }
    if actual_bytes != response.declared_bytes {
        return Ok(MediaArchiveOutcome::MetadataOnly(
            MediaVerificationReason::ContentLengthMismatch,
        ));
    }
    let digest = Sha256::digest(response.body);
    if digest.as_slice() != response.expected_digest {
        return Ok(MediaArchiveOutcome::MetadataOnly(
            MediaVerificationReason::ContentDigestMismatch,
        ));
    }
    let digest_hex = hex_encode(&digest);
    let object_root = store.root.join("sha256");
    let temporary_root = store.root.join("tmp");
    tokio::fs::create_dir_all(&object_root)
        .await
        .map_err(MediaStoreError::Storage)?;
    tokio::fs::create_dir_all(&temporary_root)
        .await
        .map_err(MediaStoreError::Storage)?;
    let temporary = temporary_root.join(uuid::Uuid::now_v7().to_string());
    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .await
        .map_err(MediaStoreError::Storage)?;
    if let Err(error) = file.write_all(response.body).await {
        drop(file);
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(MediaStoreError::Storage(error));
    }
    if let Err(error) = file.sync_all().await {
        drop(file);
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(MediaStoreError::Storage(error));
    }
    drop(file);
    let destination = object_root.join(&digest_hex);
    match tokio::fs::hard_link(&temporary, &destination).await {
        Ok(()) => tokio::fs::remove_file(&temporary)
            .await
            .map_err(MediaStoreError::Storage)?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            tokio::fs::remove_file(&temporary)
                .await
                .map_err(MediaStoreError::Storage)?;
            let existing = tokio::fs::read(&destination)
                .await
                .map_err(MediaStoreError::Storage)?;
            if Sha256::digest(existing).as_slice() != digest.as_slice() {
                return Err(MediaStoreError::DigestConflict);
            }
        }
        Err(error) => {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(MediaStoreError::Storage(error));
        }
    }
    let byte_size =
        i64::try_from(response.body.len()).map_err(|_| MediaStoreError::SizeOverflow)?;
    Ok(MediaArchiveOutcome::Archived(ArchivedMedia {
        blob_ref: format!("instagram-archive/media/sha256/{digest_hex}"),
        content_hash: digest.to_vec(),
        byte_size,
        media_type: response.expected_mime.as_str(),
    }))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut output, byte| {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
        output
    })
}

/// Why an observation remains metadata-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataOnlyReason {
    /// No authorized policy requested byte archival.
    PolicyNotAuthorized,
    /// The acquisition provenance cannot authorize provider-media archival.
    AcquisitionNotEligible,
    /// Rights or permission to retain bytes were not established.
    RightsUnknown,
    /// A separately recorded explicit user action is required.
    ExplicitActionRequired,
    /// The observed media URL is absent, malformed, or not HTTPS.
    HttpsRequired,
    /// Media kind eligibility was not established.
    KindUnknown,
    /// Response MIME eligibility was not established.
    MimeUnknown,
    /// The provider URL lifetime cannot cover the fetch lease.
    UrlLifetimeUnknown,
    /// The provider object size is unknown.
    ObjectSizeUnknown,
    /// The object is empty or exceeds its byte ceiling.
    ObjectBudgetExceeded,
    /// Remaining owner storage is unknown.
    OwnerBudgetUnknown,
    /// Remaining owner storage cannot fit the object.
    OwnerBudgetExceeded,
}

/// Immutable finite permission to begin one media fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaFetchLease {
    /// Maximum response bytes accepted for this object.
    pub max_bytes: u64,
}

/// Persisted policy result for one provider-media observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaRetentionDecision {
    /// Persist metadata only and do not start network I/O.
    MetadataOnly(MetadataOnlyReason),
    /// Byte archival is authorized within this immutable lease.
    Archive(MediaFetchLease),
}

/// Inputs required before provider-media bytes may be requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaPolicyInput {
    /// Whether an authorized retention policy explicitly requests bytes.
    pub archive_requested: bool,
    /// Whether the acquisition lane permits provider-media byte archival.
    pub acquisition_eligible: bool,
    /// Whether rights or permission to retain bytes are established.
    pub rights_confirmed: Option<bool>,
    /// Whether the observed provider media URL is a validated HTTPS URL.
    pub url_is_https: Option<bool>,
    /// Whether the provider media kind is eligible.
    pub kind_eligible: Option<bool>,
    /// Whether the expected MIME is eligible.
    pub mime_eligible: Option<bool>,
    /// Whether the URL remains valid for the whole fetch lease.
    pub url_lifetime_sufficient: Option<bool>,
    /// Provider-declared byte length, when known.
    pub declared_bytes: Option<u64>,
    /// Per-object response ceiling.
    pub max_object_bytes: u64,
    /// Owner storage remaining before this object is admitted.
    pub owner_remaining_bytes: Option<u64>,
    /// Whether a separately recorded explicit user action exists.
    pub explicit_action: bool,
}

/// Evaluates policy and invokes `fetch` only when byte archival is authorized.
pub fn observe_media<F>(input: MediaPolicyInput, mut fetch: F) -> MediaRetentionDecision
where
    F: FnMut(MediaFetchLease),
{
    if !input.archive_requested {
        return MediaRetentionDecision::MetadataOnly(MetadataOnlyReason::PolicyNotAuthorized);
    }
    let refused = if !input.acquisition_eligible {
        Some(MetadataOnlyReason::AcquisitionNotEligible)
    } else if !input.explicit_action {
        Some(MetadataOnlyReason::ExplicitActionRequired)
    } else if input.rights_confirmed != Some(true) {
        Some(MetadataOnlyReason::RightsUnknown)
    } else if input.url_is_https != Some(true) {
        Some(MetadataOnlyReason::HttpsRequired)
    } else if input.kind_eligible != Some(true) {
        Some(MetadataOnlyReason::KindUnknown)
    } else if input.mime_eligible != Some(true) {
        Some(MetadataOnlyReason::MimeUnknown)
    } else if input.url_lifetime_sufficient != Some(true) {
        Some(MetadataOnlyReason::UrlLifetimeUnknown)
    } else if input.declared_bytes.is_none() {
        Some(MetadataOnlyReason::ObjectSizeUnknown)
    } else if input.declared_bytes == Some(0)
        || input
            .declared_bytes
            .is_some_and(|bytes| bytes > input.max_object_bytes)
    {
        Some(MetadataOnlyReason::ObjectBudgetExceeded)
    } else if input.owner_remaining_bytes.is_none() {
        Some(MetadataOnlyReason::OwnerBudgetUnknown)
    } else if input
        .owner_remaining_bytes
        .zip(input.declared_bytes)
        .is_some_and(|(remaining, bytes)| remaining < bytes)
    {
        Some(MetadataOnlyReason::OwnerBudgetExceeded)
    } else {
        None
    };
    if let Some(reason) = refused {
        return MediaRetentionDecision::MetadataOnly(reason);
    }
    let Some(max_bytes) = input.declared_bytes else {
        return MediaRetentionDecision::MetadataOnly(MetadataOnlyReason::ObjectSizeUnknown);
    };
    let lease = MediaFetchLease { max_bytes };
    fetch(lease);
    MediaRetentionDecision::Archive(lease)
}
