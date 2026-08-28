//! Media byte archival and reference-safe retention tests.

use sha2::Digest as _;

use ratatoskr_instagram_archive::media_retention::{
    AcquiredMedia, ApprovedMediaMime, BlobCleanupPlan, BlobDeletionBackend, BlobDeletionFailure,
    BlobDeletionTaskOutcome, MediaArchiveOutcome, MediaBlobStore, MediaFetchLease,
    MediaPolicyInput, MediaRetentionDecision, MediaVerificationReason, MetadataOnlyReason,
    archive_acquired_media, observe_media, plan_media_reference_expiry, process_blob_deletion_task,
};
use ratatoskr_instagram_archive::test_support::TestDatabase;

#[test]
fn metadata_observation_never_fetches_without_an_authorized_policy() {
    let mut fetches = 0_u32;
    let decision = observe_media(
        MediaPolicyInput {
            archive_requested: false,
            acquisition_eligible: true,
            rights_confirmed: Some(true),
            url_is_https: Some(true),
            kind_eligible: Some(true),
            mime_eligible: Some(true),
            url_lifetime_sufficient: Some(true),
            declared_bytes: Some(1024),
            max_object_bytes: 8 * 1024 * 1024,
            owner_remaining_bytes: Some(8 * 1024 * 1024),
            explicit_action: true,
        },
        |_| fetches += 1,
    );

    assert_eq!(
        decision,
        MediaRetentionDecision::MetadataOnly(MetadataOnlyReason::PolicyNotAuthorized)
    );
    assert_eq!(fetches, 0, "metadata-only observation must not fetch");
}

struct FailOnceThenDelete {
    path: std::path::PathBuf,
    failed: std::sync::atomic::AtomicBool,
}

impl BlobDeletionBackend for FailOnceThenDelete {
    fn delete_if_matches<'a>(
        &'a self,
        _blob_ref: &'a str,
        _content_hash: &'a [u8],
        _byte_size: i64,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), BlobDeletionFailure>> + Send + 'a>,
    > {
        Box::pin(async move {
            if !self.failed.swap(true, std::sync::atomic::Ordering::SeqCst) {
                return Err(BlobDeletionFailure::StorageUnavailable);
            }
            tokio::fs::remove_file(&self.path)
                .await
                .map_err(|_| BlobDeletionFailure::StorageUnavailable)?;
            if tokio::fs::try_exists(&self.path)
                .await
                .map_err(|_| BlobDeletionFailure::StorageUnavailable)?
            {
                return Err(BlobDeletionFailure::StorageUnavailable);
            }
            Ok(())
        })
    }
}

#[tokio::test]
async fn failed_blob_delete_stays_pending_until_verified_absent() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let root = std::env::temp_dir().join(format!("instagram-delete-test-{}", uuid::Uuid::now_v7()));
    let body = b"unreferenced synthetic media";
    let digest = sha2::Sha256::digest(body).to_vec();
    let digest_hex = digest.iter().fold(String::new(), |mut output, byte| {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
        output
    });
    let path = root.join("sha256").join(&digest_hex);
    tokio::fs::create_dir_all(path.parent().expect("digest path has parent"))
        .await
        .expect("test store root");
    tokio::fs::write(&path, body)
        .await
        .expect("test object stores");
    let task_id = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.blob_deletion_tasks \
         (task_id, blob_ref, content_hash, byte_size, media_class, state) \
         values ($1, $2, $3, $4, 'provider_media', 'pending')",
    )
    .bind(task_id)
    .bind(format!("instagram-archive/media/sha256/{digest_hex}"))
    .bind(&digest)
    .bind(i64::try_from(body.len()).expect("synthetic body length fits"))
    .execute(test.database.pool())
    .await
    .expect("deletion task stores");
    let backend = FailOnceThenDelete {
        path: path.clone(),
        failed: std::sync::atomic::AtomicBool::new(false),
    };

    let first = process_blob_deletion_task(&test.database, &backend, task_id)
        .await
        .expect("first attempt records failure");
    let second = process_blob_deletion_task(&test.database, &backend, task_id)
        .await
        .expect("retry completes");
    let stored: (String, i32, Option<String>) = sqlx::query_as(
        "select state, attempt_count, last_failure_class \
         from instagram_archive.blob_deletion_tasks where task_id = $1",
    )
    .bind(task_id)
    .fetch_one(test.database.pool())
    .await
    .expect("task state reads");

    assert_eq!(
        first,
        BlobDeletionTaskOutcome::Pending(BlobDeletionFailure::StorageUnavailable)
    );
    assert_eq!(second, BlobDeletionTaskOutcome::Complete);
    assert_eq!(stored, ("complete".to_owned(), 2, None));
    assert!(!path.exists(), "complete requires verified absence");

    if root.exists() {
        std::fs::remove_dir_all(root).expect("test storage cleanup");
    }
    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn expiring_one_reference_preserves_a_blob_referenced_elsewhere() {
    let test = TestDatabase::create().await.expect("a fresh test database");
    let first_media = uuid::Uuid::now_v7();
    let digest = vec![0x42_u8; 32];
    for (media_id, suffix) in [(first_media, "one"), (uuid::Uuid::now_v7(), "two")] {
        sqlx::query(
            "insert into instagram_archive.media \
             (media_id, permalink, media_type, acquisition_method, saved_authority, \
              upstream_status, blob_ref, content_hash, byte_size, media_state, retention_class) \
             values ($1, $2, 'image', 'public_resolution', 'explicit_user_capture', \
              'available', 'instagram-archive/media/sha256/shared', $3, 7, \
              'bytes_archived', 'explicit_archive')",
        )
        .bind(media_id)
        .bind(format!("https://www.instagram.com/p/SHARED{suffix}/"))
        .bind(&digest)
        .execute(test.database.pool())
        .await
        .expect("synthetic media reference stores");
    }

    let plan = plan_media_reference_expiry(&test.database, first_media)
        .await
        .expect("cleanup plan answers");
    assert_eq!(plan, BlobCleanupPlan::RetainShared { live_references: 1 });

    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn verified_bytes_attach_only_after_url_mime_size_and_digest_checks() {
    let body = b"synthetic provider media";
    let valid_digest: [u8; 32] = sha2::Sha256::digest(body).into();
    let cases = [
        (
            MediaVerificationReason::FinalUrlNotHttps,
            "http://cdn.example.invalid/media.jpg",
            "image/jpeg",
            body.len() as u64,
            valid_digest,
        ),
        (
            MediaVerificationReason::MimeMismatch,
            "https://cdn.example.invalid/media.jpg",
            "application/octet-stream",
            body.len() as u64,
            valid_digest,
        ),
        (
            MediaVerificationReason::ContentLengthMismatch,
            "https://cdn.example.invalid/media.jpg",
            "image/jpeg",
            body.len() as u64 + 1,
            valid_digest,
        ),
        (
            MediaVerificationReason::ContentDigestMismatch,
            "https://cdn.example.invalid/media.jpg",
            "image/jpeg",
            body.len() as u64,
            [0x5a; 32],
        ),
    ];

    for (expected, final_url, observed_content_type, declared_bytes, expected_digest) in cases {
        let root = std::env::temp_dir().join(format!(
            "instagram-media-verification-{}",
            uuid::Uuid::now_v7()
        ));
        let store = MediaBlobStore::new(&root);
        let outcome = archive_acquired_media(
            &store,
            MediaFetchLease { max_bytes: 1024 },
            AcquiredMedia {
                final_url,
                expected_mime: ApprovedMediaMime::ImageJpeg,
                observed_content_type,
                declared_bytes,
                expected_digest,
                body,
            },
        )
        .await
        .expect("verification refusal is a normal outcome");
        let object_count =
            std::fs::read_dir(root.join("sha256")).map_or(0, std::iter::Iterator::count);
        assert_eq!(outcome, MediaArchiveOutcome::MetadataOnly(expected));
        assert_eq!(object_count, 0, "refusal must leave no partial object");
        if root.exists() {
            std::fs::remove_dir_all(root).expect("test storage cleanup");
        }
    }
}

fn eligible_policy() -> MediaPolicyInput {
    MediaPolicyInput {
        archive_requested: true,
        acquisition_eligible: true,
        rights_confirmed: Some(true),
        url_is_https: Some(true),
        kind_eligible: Some(true),
        mime_eligible: Some(true),
        url_lifetime_sufficient: Some(true),
        declared_bytes: Some(1024),
        max_object_bytes: 4096,
        owner_remaining_bytes: Some(4096),
        explicit_action: true,
    }
}

#[test]
fn archival_refuses_before_io_when_any_guard_is_unknown_or_exhausted() {
    let cases = [
        (
            MetadataOnlyReason::AcquisitionNotEligible,
            MediaPolicyInput {
                acquisition_eligible: false,
                ..eligible_policy()
            },
        ),
        (
            MetadataOnlyReason::RightsUnknown,
            MediaPolicyInput {
                rights_confirmed: None,
                ..eligible_policy()
            },
        ),
        (
            MetadataOnlyReason::ExplicitActionRequired,
            MediaPolicyInput {
                explicit_action: false,
                ..eligible_policy()
            },
        ),
        (
            MetadataOnlyReason::HttpsRequired,
            MediaPolicyInput {
                url_is_https: None,
                ..eligible_policy()
            },
        ),
        (
            MetadataOnlyReason::KindUnknown,
            MediaPolicyInput {
                kind_eligible: None,
                ..eligible_policy()
            },
        ),
        (
            MetadataOnlyReason::MimeUnknown,
            MediaPolicyInput {
                mime_eligible: None,
                ..eligible_policy()
            },
        ),
        (
            MetadataOnlyReason::UrlLifetimeUnknown,
            MediaPolicyInput {
                url_lifetime_sufficient: None,
                ..eligible_policy()
            },
        ),
        (
            MetadataOnlyReason::ObjectSizeUnknown,
            MediaPolicyInput {
                declared_bytes: None,
                ..eligible_policy()
            },
        ),
        (
            MetadataOnlyReason::ObjectBudgetExceeded,
            MediaPolicyInput {
                declared_bytes: Some(4097),
                ..eligible_policy()
            },
        ),
        (
            MetadataOnlyReason::OwnerBudgetUnknown,
            MediaPolicyInput {
                owner_remaining_bytes: None,
                ..eligible_policy()
            },
        ),
        (
            MetadataOnlyReason::OwnerBudgetExceeded,
            MediaPolicyInput {
                owner_remaining_bytes: Some(1023),
                ..eligible_policy()
            },
        ),
    ];

    for (expected, input) in cases {
        let mut fetches = 0_u32;
        let decision = observe_media(input, |_| fetches += 1);
        assert_eq!(decision, MediaRetentionDecision::MetadataOnly(expected));
        assert_eq!(fetches, 0, "guard {expected:?} must refuse before I/O");
    }
}
