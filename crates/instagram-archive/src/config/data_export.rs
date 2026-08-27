//! Closed, redacted configuration for authenticated Data Export intake.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::Serialize;
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use super::{Violation, parse_positive, validate_range};

/// Immutable Data Export intake and hostile-archive limits.
#[derive(Clone, Serialize)]
pub struct DataExportConfig {
    /// Whether authenticated archive receipt and worker processing are enabled.
    pub enabled: bool,
    /// Private content-addressed archive object root.
    pub blob_root: Option<PathBuf>,
    /// Private create-new upload staging root.
    pub staging_root: Option<PathBuf>,
    /// Largest accepted upload body.
    pub max_body_bytes: u64,
    /// Largest accepted ZIP entry count.
    pub max_entries: usize,
    /// Largest accepted raw ZIP entry-name length.
    pub max_entry_path_bytes: usize,
    /// Largest accepted normalized path depth.
    pub max_path_depth: usize,
    /// Largest cumulative declared compressed entry bytes.
    pub max_total_compressed_bytes: u64,
    /// Largest cumulative emitted decompressed bytes.
    pub max_total_decompressed_bytes: u64,
    /// Largest emitted bytes from one entry.
    pub max_entry_decompressed_bytes: u64,
    /// Largest permitted decompressed-to-compressed byte ratio.
    pub max_compression_ratio: u64,
    /// Milliseconds between bounded worker passes.
    pub worker_poll_interval_ms: u64,
    /// Maximum runs claimed in one worker pass.
    pub worker_batch_size: u32,
    #[serde(skip_serializing)]
    credential_hashes: BTreeMap<[u8; 32], Uuid>,
}

impl std::fmt::Debug for DataExportConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DataExportConfig")
            .field("enabled", &self.enabled)
            .field("blob_root", &self.blob_root)
            .field("staging_root", &self.staging_root)
            .field("max_body_bytes", &self.max_body_bytes)
            .field("max_entries", &self.max_entries)
            .field("max_entry_path_bytes", &self.max_entry_path_bytes)
            .field("max_path_depth", &self.max_path_depth)
            .field(
                "max_total_compressed_bytes",
                &self.max_total_compressed_bytes,
            )
            .field(
                "max_total_decompressed_bytes",
                &self.max_total_decompressed_bytes,
            )
            .field(
                "max_entry_decompressed_bytes",
                &self.max_entry_decompressed_bytes,
            )
            .field("max_compression_ratio", &self.max_compression_ratio)
            .field("worker_poll_interval_ms", &self.worker_poll_interval_ms)
            .field("worker_batch_size", &self.worker_batch_size)
            .field("credential_hashes", &"[REDACTED]")
            .finish()
    }
}

impl Default for DataExportConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            blob_root: None,
            staging_root: None,
            max_body_bytes: 5 * 1024 * 1024 * 1024,
            max_entries: 20_000,
            max_entry_path_bytes: 1_024,
            max_path_depth: 12,
            max_total_compressed_bytes: 5 * 1024 * 1024 * 1024,
            max_total_decompressed_bytes: 20 * 1024 * 1024 * 1024,
            max_entry_decompressed_bytes: 64 * 1024 * 1024,
            max_compression_ratio: 200,
            worker_poll_interval_ms: 1_000,
            worker_batch_size: 4,
            credential_hashes: BTreeMap::new(),
        }
    }
}

impl DataExportConfig {
    /// Resolves an opaque bearer credential to its configured owner.
    #[must_use]
    pub fn authenticate(&self, token: &str) -> Option<Uuid> {
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        self.credential_hashes.get(&digest).copied()
    }

    pub(super) fn credentials_configured(&self) -> bool {
        !self.credential_hashes.is_empty()
    }

    pub(super) fn set_bearer_tokens(&mut self, value: &str) -> Result<(), &'static str> {
        let mut hashes = BTreeMap::new();
        let mut owners = BTreeSet::new();
        let entries = value.split(',').collect::<Vec<_>>();
        if entries.is_empty() || entries.len() > 100 {
            return Err("must contain between one and 100 owner:token entries");
        }
        for entry in entries {
            let Some((owner, token)) = entry.split_once(':') else {
                return Err("must contain owner-uuid:opaque-token entries");
            };
            let owner = Uuid::parse_str(owner)
                .map_err(|_| "must contain valid owner UUIDs without echoing values")?;
            if !owners.insert(owner) {
                return Err("must not repeat an owner UUID");
            }
            if !(32..=256).contains(&token.len())
                || !token
                    .bytes()
                    .all(|byte| byte.is_ascii_graphic() && byte != b':' && byte != b',')
            {
                return Err(
                    "tokens must be 32 to 256 visible ASCII characters without colon or comma",
                );
            }
            let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
            if hashes.insert(digest, owner).is_some() {
                return Err("must not repeat a bearer token");
            }
        }
        self.credential_hashes = hashes;
        Ok(())
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one validation inventory keeps every finite Data Export bound reviewable together"
    )]
    pub(super) fn validate(&self, violations: &mut Vec<Violation>) {
        if self.enabled {
            for (present, key, rule) in [
                (
                    self.blob_root.is_some(),
                    "RATATOSKR__DATA_EXPORT__BLOB_ROOT",
                    "is required when Data Export intake is enabled",
                ),
                (
                    self.staging_root.is_some(),
                    "RATATOSKR__DATA_EXPORT__STAGING_ROOT",
                    "is required when Data Export intake is enabled",
                ),
                (
                    self.credentials_configured(),
                    "RATATOSKR__DATA_EXPORT__BEARER_TOKENS",
                    "must contain at least one owner-bound bearer credential",
                ),
            ] {
                if !present {
                    violations.push(Violation {
                        key: key.to_owned(),
                        rule,
                    });
                }
            }
        }
        if self
            .blob_root
            .as_ref()
            .zip(self.staging_root.as_ref())
            .is_some_and(|(blob, staging)| {
                blob == staging || blob.starts_with(staging) || staging.starts_with(blob)
            })
        {
            violations.push(Violation {
                key: "RATATOSKR__DATA_EXPORT__STAGING_ROOT".to_owned(),
                rule: "must be disjoint from the immutable blob root",
            });
        }
        for (value, minimum, maximum, key) in [
            (
                self.max_body_bytes,
                1_024,
                10 * 1024 * 1024 * 1024,
                "RATATOSKR__DATA_EXPORT__MAX_BODY_BYTES",
            ),
            (
                u64::try_from(self.max_entries).unwrap_or(u64::MAX),
                1,
                100_000,
                "RATATOSKR__DATA_EXPORT__MAX_ENTRIES",
            ),
            (
                u64::try_from(self.max_entry_path_bytes).unwrap_or(u64::MAX),
                16,
                4_096,
                "RATATOSKR__DATA_EXPORT__MAX_ENTRY_PATH_BYTES",
            ),
            (
                u64::try_from(self.max_path_depth).unwrap_or(u64::MAX),
                1,
                32,
                "RATATOSKR__DATA_EXPORT__MAX_PATH_DEPTH",
            ),
            (
                self.max_total_compressed_bytes,
                1_024,
                10 * 1024 * 1024 * 1024,
                "RATATOSKR__DATA_EXPORT__MAX_TOTAL_COMPRESSED_BYTES",
            ),
            (
                self.max_total_decompressed_bytes,
                1_024,
                100 * 1024 * 1024 * 1024,
                "RATATOSKR__DATA_EXPORT__MAX_TOTAL_DECOMPRESSED_BYTES",
            ),
            (
                self.max_entry_decompressed_bytes,
                1_024,
                1024 * 1024 * 1024,
                "RATATOSKR__DATA_EXPORT__MAX_ENTRY_DECOMPRESSED_BYTES",
            ),
            (
                self.max_compression_ratio,
                1,
                1_000,
                "RATATOSKR__DATA_EXPORT__MAX_COMPRESSION_RATIO",
            ),
            (
                self.worker_poll_interval_ms,
                100,
                60_000,
                "RATATOSKR__DATA_EXPORT__WORKER_POLL_INTERVAL_MS",
            ),
            (
                u64::from(self.worker_batch_size),
                1,
                100,
                "RATATOSKR__DATA_EXPORT__WORKER_BATCH_SIZE",
            ),
        ] {
            validate_range(value, minimum, maximum, key, violations);
        }
        for (valid, key, rule) in [
            (
                self.max_total_compressed_bytes <= self.max_body_bytes,
                "RATATOSKR__DATA_EXPORT__MAX_TOTAL_COMPRESSED_BYTES",
                "must not exceed the upload body limit",
            ),
            (
                self.max_entry_decompressed_bytes <= self.max_total_decompressed_bytes,
                "RATATOSKR__DATA_EXPORT__MAX_ENTRY_DECOMPRESSED_BYTES",
                "must not exceed the total decompressed limit",
            ),
        ] {
            if !valid {
                violations.push(Violation {
                    key: key.to_owned(),
                    rule,
                });
            }
        }
    }

    pub(super) fn apply_environment(
        &mut self,
        key: &str,
        value: &str,
        violations: &mut Vec<Violation>,
    ) {
        let refused = |rule| Violation {
            key: key.to_owned(),
            rule,
        };
        match key {
            "RATATOSKR__DATA_EXPORT__ENABLED" => match value {
                "true" => self.enabled = true,
                "false" => self.enabled = false,
                _ => violations.push(refused("must be true or false")),
            },
            "RATATOSKR__DATA_EXPORT__BLOB_ROOT" => {
                self.blob_root = private_absolute_path(key, value, violations);
            }
            "RATATOSKR__DATA_EXPORT__STAGING_ROOT" => {
                self.staging_root = private_absolute_path(key, value, violations);
            }
            "RATATOSKR__DATA_EXPORT__BEARER_TOKENS" => {
                if let Err(rule) = self.set_bearer_tokens(value) {
                    violations.push(refused(rule));
                }
            }
            "RATATOSKR__DATA_EXPORT__MAX_BODY_BYTES" => {
                apply_positive(key, value, &mut self.max_body_bytes, violations);
            }
            "RATATOSKR__DATA_EXPORT__MAX_ENTRIES" => {
                apply_positive(key, value, &mut self.max_entries, violations);
            }
            "RATATOSKR__DATA_EXPORT__MAX_ENTRY_PATH_BYTES" => {
                apply_positive(key, value, &mut self.max_entry_path_bytes, violations);
            }
            "RATATOSKR__DATA_EXPORT__MAX_PATH_DEPTH" => {
                apply_positive(key, value, &mut self.max_path_depth, violations);
            }
            "RATATOSKR__DATA_EXPORT__MAX_TOTAL_COMPRESSED_BYTES" => {
                apply_positive(key, value, &mut self.max_total_compressed_bytes, violations);
            }
            "RATATOSKR__DATA_EXPORT__MAX_TOTAL_DECOMPRESSED_BYTES" => {
                apply_positive(
                    key,
                    value,
                    &mut self.max_total_decompressed_bytes,
                    violations,
                );
            }
            "RATATOSKR__DATA_EXPORT__MAX_ENTRY_DECOMPRESSED_BYTES" => {
                apply_positive(
                    key,
                    value,
                    &mut self.max_entry_decompressed_bytes,
                    violations,
                );
            }
            "RATATOSKR__DATA_EXPORT__MAX_COMPRESSION_RATIO" => {
                apply_positive(key, value, &mut self.max_compression_ratio, violations);
            }
            "RATATOSKR__DATA_EXPORT__WORKER_POLL_INTERVAL_MS" => {
                apply_positive(key, value, &mut self.worker_poll_interval_ms, violations);
            }
            "RATATOSKR__DATA_EXPORT__WORKER_BATCH_SIZE" => {
                apply_positive(key, value, &mut self.worker_batch_size, violations);
            }
            _ => violations.push(refused("is not recognized")),
        }
    }
}

fn private_absolute_path(
    key: &str,
    value: &str,
    violations: &mut Vec<Violation>,
) -> Option<PathBuf> {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        Some(path)
    } else {
        violations.push(Violation {
            key: key.to_owned(),
            rule: "must be an absolute private directory path",
        });
        None
    }
}

fn apply_positive<T>(key: &str, value: &str, target: &mut T, violations: &mut Vec<Violation>)
where
    T: std::str::FromStr + Default + PartialOrd,
{
    match parse_positive::<T>(value) {
        Ok(parsed) => *target = parsed,
        Err(rule) => violations.push(Violation {
            key: key.to_owned(),
            rule,
        }),
    }
}
