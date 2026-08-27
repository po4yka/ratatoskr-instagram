//! Create-new staging and no-overwrite content-addressed archive publication.

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use futures_util::{Stream, StreamExt as _};
use sha2::{Digest as _, Sha256};
use tokio::fs::{self, OpenOptions};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use uuid::Uuid;

use super::{ReceiptError, blob_ref};

#[derive(Debug, Clone)]
pub(super) struct ArchiveStore {
    blob_root: PathBuf,
    staging_root: PathBuf,
    max_body_bytes: u64,
}

#[derive(Debug)]
pub(super) struct StoredArchive {
    pub(super) blob_ref: ratatoskr_identifiers::BlobRef,
    pub(super) digest: Vec<u8>,
    pub(super) byte_size: i64,
}

impl ArchiveStore {
    pub(super) fn new(blob_root: PathBuf, staging_root: PathBuf, max_body_bytes: u64) -> Self {
        Self {
            blob_root,
            staging_root,
            max_body_bytes,
        }
    }

    pub(super) async fn store<S, B, E>(&self, chunks: S) -> Result<StoredArchive, ReceiptError>
    where
        S: Stream<Item = Result<B, E>>,
        B: AsRef<[u8]>,
    {
        ensure_private_directory(&self.staging_root).await?;
        let temporary = self.staging_root.join(Uuid::now_v7().to_string());
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut output = options
            .open(&temporary)
            .await
            .map_err(|_| ReceiptError::RawStorage)?;
        let copied = self.copy_and_hash(chunks, &mut output).await;
        let (digest, byte_size) = match copied {
            Ok(value) => value,
            Err(error) => {
                drop(output);
                remove_temporary(&temporary).await?;
                return Err(error);
            }
        };
        if output.sync_all().await.is_err() {
            drop(output);
            remove_temporary(&temporary).await?;
            return Err(ReceiptError::RawStorage);
        }
        drop(output);

        let digest_hex = hex(&digest);
        let destination = self.blob_root.join("sha256").join(&digest_hex);
        let publication =
            Box::pin(self.publish_or_verify(&temporary, &destination, &digest, byte_size)).await;
        if publication.is_err() && fs::try_exists(&temporary).await.unwrap_or(false) {
            remove_temporary(&temporary).await?;
        }
        publication?;
        let length_bytes = u64::try_from(byte_size).map_err(|_| ReceiptError::BodyLimit)?;
        Ok(StoredArchive {
            blob_ref: blob_ref(&digest_hex, length_bytes)?,
            digest,
            byte_size,
        })
    }

    pub(super) async fn verified_path(
        &self,
        blob_ref: &ratatoskr_identifiers::BlobRef,
        expected_digest: &[u8],
        expected_size: i64,
    ) -> Result<PathBuf, ReceiptError> {
        let digest_hex = hex(expected_digest);
        if blob_ref.owner_service.as_str() != "ratatoskr-instagram"
            || blob_ref.digest.algorithm != ratatoskr_identifiers::DigestAlgorithm::Sha256
            || blob_ref.digest.hex.as_str() != digest_hex
            || blob_ref.media_type.as_str() != "application/zip"
            || i64::try_from(blob_ref.length_bytes).ok() != Some(expected_size)
        {
            return Err(ReceiptError::CorruptEvidence);
        }
        let path = self.blob_root.join("sha256").join(digest_hex);
        Box::pin(verify_existing(&path, expected_digest, expected_size)).await?;
        Ok(path)
    }

    async fn copy_and_hash<S, B, E>(
        &self,
        chunks: S,
        output: &mut tokio::fs::File,
    ) -> Result<(Vec<u8>, i64), ReceiptError>
    where
        S: Stream<Item = Result<B, E>>,
        B: AsRef<[u8]>,
    {
        let mut chunks = Box::pin(chunks);
        let mut hasher = Sha256::new();
        let mut byte_size = 0_u64;
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk.map_err(|_| ReceiptError::BodyStream)?;
            let bytes = chunk.as_ref();
            byte_size = byte_size
                .checked_add(u64::try_from(bytes.len()).map_err(|_| ReceiptError::BodyLimit)?)
                .ok_or(ReceiptError::BodyLimit)?;
            if byte_size > self.max_body_bytes || byte_size > i64::MAX as u64 {
                return Err(ReceiptError::BodyLimit);
            }
            hasher.update(bytes);
            output
                .write_all(bytes)
                .await
                .map_err(|_| ReceiptError::RawStorage)?;
        }
        let byte_size = i64::try_from(byte_size).map_err(|_| ReceiptError::BodyLimit)?;
        Ok((hasher.finalize().to_vec(), byte_size))
    }

    async fn publish_or_verify(
        &self,
        temporary: &Path,
        destination: &Path,
        digest: &[u8],
        byte_size: i64,
    ) -> Result<(), ReceiptError> {
        let parent = destination.parent().ok_or(ReceiptError::RawStorage)?;
        ensure_private_directory(&self.blob_root).await?;
        ensure_private_directory(parent).await?;
        match fs::hard_link(temporary, destination).await {
            Ok(()) => {
                sync_directory(parent).await?;
                remove_temporary(temporary).await
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Box::pin(verify_existing(destination, digest, byte_size)).await?;
                remove_temporary(temporary).await
            }
            Err(_) => Err(ReceiptError::RawStorage),
        }
    }
}

async fn verify_existing(
    path: &Path,
    expected_digest: &[u8],
    expected_size: i64,
) -> Result<(), ReceiptError> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|_| ReceiptError::ImmutableConflict)?;
    if !metadata.file_type().is_file() {
        return Err(ReceiptError::ImmutableConflict);
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(ReceiptError::ImmutableConflict);
    }
    let mut file = fs::File::open(path)
        .await
        .map_err(|_| ReceiptError::ImmutableConflict)?;
    let mut hasher = Sha256::new();
    let mut size = 0_i64;
    let mut buffer = [0_u8; 16_384];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|_| ReceiptError::ImmutableConflict)?;
        if read == 0 {
            break;
        }
        size = size
            .checked_add(i64::try_from(read).map_err(|_| ReceiptError::ImmutableConflict)?)
            .ok_or(ReceiptError::ImmutableConflict)?;
        let bytes = buffer.get(..read).ok_or(ReceiptError::ImmutableConflict)?;
        hasher.update(bytes);
    }
    let digest = hasher.finalize();
    if size == expected_size && digest.as_slice() == expected_digest {
        Ok(())
    } else {
        Err(ReceiptError::ImmutableConflict)
    }
}

async fn ensure_private_directory(path: &Path) -> Result<(), ReceiptError> {
    fs::create_dir_all(path)
        .await
        .map_err(|_| ReceiptError::RawStorage)?;
    #[cfg(unix)]
    fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .await
        .map_err(|_| ReceiptError::RawStorage)?;
    Ok(())
}

async fn remove_temporary(path: &Path) -> Result<(), ReceiptError> {
    fs::remove_file(path)
        .await
        .map_err(|_| ReceiptError::RawStorage)
}

async fn sync_directory(path: &Path) -> Result<(), ReceiptError> {
    fs::File::open(path)
        .await
        .map_err(|_| ReceiptError::RawStorage)?
        .sync_all()
        .await
        .map_err(|_| ReceiptError::RawStorage)
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}
