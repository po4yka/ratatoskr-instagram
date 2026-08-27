//! Authenticated raw-first Instagram Data Export intake.

use futures_util::Stream;
use ratatoskr_identifiers::{
    BlobOwner, BlobRef, ContentDigest, DigestAlgorithm, DigestHex, MediaType,
};
use serde::Serialize;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{DataExportConfig, Database, PersistenceError};

mod archive;
mod blob;
mod parser;
mod reconcile;
mod worker;

use blob::{ArchiveStore, StoredArchive};

pub use archive::{
    ArchiveError, ArchiveFailureClass, ArchiveInventory, ArchiveLimits, inspect_archive,
    read_archive_entry,
};
pub use parser::{
    DATA_EXPORT_PARSER_ID, ParsedExport, ParsedExportRecord, ParserError, ParserWarning,
    UnknownExportEntry, UnknownExportRecord, parse_export,
};
pub use reconcile::CompletenessReport;
pub use worker::{DataExportWorker, WorkerPass};

/// Durable Data Export import state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportState {
    /// Exact archive bytes and owner receipt are durable.
    Received,
    /// ZIP metadata and bounded entry reads passed inspection.
    Inspected,
    /// A supported versioned parser produced staging records.
    Parsed,
    /// Staging records and completeness evidence were reconciled atomically.
    Reconciled,
    /// One stage failed; the immutable raw archive remains retained.
    Failed,
}

impl ImportState {
    fn parse(value: &str) -> Result<Self, ReceiptError> {
        match value {
            "received" => Ok(Self::Received),
            "inspected" => Ok(Self::Inspected),
            "parsed" => Ok(Self::Parsed),
            "reconciled" => Ok(Self::Reconciled),
            "failed" => Ok(Self::Failed),
            _ => Err(ReceiptError::CorruptEvidence),
        }
    }
}

/// Durable evidence returned for a newly received or replayed archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportReceipt {
    /// Stable import-run identity.
    pub run_id: Uuid,
    /// Current durable state; receipt always begins at `received`.
    pub state: ImportState,
    /// Typed immutable raw archive reference.
    pub archive: BlobRef,
}

/// Whether receipt created new owner evidence or replayed exact prior bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiptOutcome {
    /// A new snapshot, run, and initial transition were committed.
    Created(ExportReceipt),
    /// The same owner had already supplied the exact archive digest.
    Replayed(ExportReceipt),
}

impl ReceiptOutcome {
    /// Returns the durable receipt carried by either outcome.
    #[must_use]
    pub fn receipt(&self) -> &ExportReceipt {
        match self {
            Self::Created(receipt) | Self::Replayed(receipt) => receipt,
        }
    }
}

/// Owner-scoped status returned by the authenticated no-store endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImportStatus {
    /// Stable import-run identity.
    pub run_id: Uuid,
    /// Current durable state.
    pub state: ImportState,
    /// Typed immutable raw archive reference.
    pub archive: BlobRef,
    /// Detected export layout, once inspection establishes it.
    pub detected_layout: Option<String>,
    /// Exact versioned parser identifier, once selected.
    pub parser_id: Option<String>,
    /// Number of deterministic staged records processed.
    pub records_processed: i64,
    /// Number of typed warnings retained.
    pub warning_count: i64,
    /// Closed terminal failure class, without private details.
    pub failure_class: Option<String>,
    /// Completeness evidence after reconciliation.
    pub completeness_report: Option<serde_json::Value>,
}

/// Raw storage or receipt persistence refusal.
#[derive(Debug, thiserror::Error)]
pub enum ReceiptError {
    /// The request-body stream failed before completion.
    #[error("archive body stream failed")]
    BodyStream,
    /// The exact body grew past the configured byte ceiling.
    #[error("archive body exceeds configured limit")]
    BodyLimit,
    /// Protected raw storage could not durably preserve the bytes.
    #[error("immutable archive storage failed")]
    RawStorage,
    /// An existing content-addressed object disagreed with its digest or size.
    #[error("existing immutable archive disagrees with receipt evidence")]
    ImmutableConflict,
    /// Durable receipt state could not be stored or read.
    #[error("Data Export receipt persistence failed")]
    Persistence(#[from] PersistenceError),
    /// Stored state violated the first-version evidence contract.
    #[error("stored Data Export evidence is inconsistent")]
    CorruptEvidence,
    /// A fixed local contract value could not construct a `BlobRef`.
    #[error("local BlobRef contract construction failed")]
    BlobContract,
}

/// Inspection-stage refusal after an archive has already been received safely.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// Hostile or malformed archive metadata/content was refused.
    #[error("archive inspection refused")]
    Archive(#[from] ArchiveError),
    /// The inspected archive does not match the supported parser grammar.
    #[error("Data Export parser refused the inspected archive")]
    Parser(#[from] ParserError),
    /// A normalized `SocialSource` fact could not be built or appended.
    #[error("Data Export publication failed")]
    Publish(#[from] crate::publishing::PublishError),
    /// Set/report evidence could not be encoded or violated its contract.
    #[error("Data Export reconciliation evidence is inconsistent")]
    Reconciliation,
    /// Raw receipt evidence could not be loaded or verified.
    #[error("archive receipt evidence failed")]
    Receipt(#[from] ReceiptError),
    /// A durable state transition lost its compare-and-swap precondition.
    #[error("archive import state transition conflict")]
    StateConflict,
}

impl ReceiptError {
    /// Closed class safe for logs and typed HTTP refusals.
    #[must_use]
    pub const fn class(&self) -> &'static str {
        match self {
            Self::BodyStream => "body_stream",
            Self::BodyLimit => "body_limit",
            Self::RawStorage => "raw_storage",
            Self::ImmutableConflict => "immutable_conflict",
            Self::Persistence(_) => "persistence",
            Self::CorruptEvidence => "corrupt_evidence",
            Self::BlobContract => "blob_contract",
        }
    }
}

/// Owner-scoped raw receipt and import-status store.
#[derive(Debug, Clone)]
pub struct DataExportStore {
    pool: PgPool,
    archives: ArchiveStore,
}

impl DataExportStore {
    /// Creates a store from already validated enabled configuration.
    ///
    /// # Errors
    ///
    /// Returns [`ReceiptError::CorruptEvidence`] if required private roots are absent.
    pub fn new(database: &Database, config: &DataExportConfig) -> Result<Self, ReceiptError> {
        let blob_root = config
            .blob_root
            .clone()
            .ok_or(ReceiptError::CorruptEvidence)?;
        let staging_root = config
            .staging_root
            .clone()
            .ok_or(ReceiptError::CorruptEvidence)?;
        Ok(Self {
            pool: database.pool().clone(),
            archives: ArchiveStore::new(blob_root, staging_root, config.max_body_bytes),
        })
    }

    /// Streams, hashes, immutably stores, and durably receipts one archive.
    ///
    /// No database transaction remains open while request or filesystem I/O runs.
    ///
    /// # Errors
    ///
    /// Returns [`ReceiptError`] for body, size, immutable-storage, contract, or
    /// persistence refusal.
    pub async fn receive<S, B, E>(
        &self,
        user_ref: Uuid,
        chunks: S,
    ) -> Result<ReceiptOutcome, ReceiptError>
    where
        S: Stream<Item = Result<B, E>>,
        B: AsRef<[u8]>,
    {
        let stored = self.archives.store(chunks).await?;
        self.persist_receipt(user_ref, stored).await
    }

    /// Loads one import only when the authenticated owner matches.
    ///
    /// # Errors
    ///
    /// Returns [`ReceiptError`] for persistence or inconsistent stored evidence.
    pub async fn status(
        &self,
        user_ref: Uuid,
        run_id: Uuid,
    ) -> Result<Option<ImportStatus>, ReceiptError> {
        type Row = (
            String,
            String,
            Option<String>,
            Option<String>,
            i64,
            i64,
            Option<String>,
            Option<serde_json::Value>,
        );
        let row: Option<Row> = sqlx::query_as(
            "select r.state, s.archive_blob_ref, r.detected_layout, r.parser_id,
                    r.records_processed, r.warning_count, r.failure_class,
                    case when c.report_id is null then null else jsonb_build_object(
                        'matched', c.matched, 'export_only', c.export_only,
                        'capture_only', c.capture_only, 'non_comparable', c.non_comparable,
                        'matched_count', c.matched_count,
                        'export_only_count', c.export_only_count,
                        'capture_only_count', c.capture_only_count,
                        'non_comparable_count', c.non_comparable_count,
                        'categories', c.categories, 'warnings', c.warnings,
                        'authority_disclaimer', c.authority_disclaimer) end
             from instagram_archive.import_runs r
             join instagram_archive.export_snapshots s on s.snapshot_id = r.snapshot_id
             left join instagram_archive.export_completeness_reports c on c.run_id = r.run_id
             where r.run_id = $1 and r.user_ref = $2",
        )
        .bind(run_id)
        .bind(user_ref)
        .fetch_optional(&self.pool)
        .await
        .map_err(PersistenceError::Query)?;
        row.map(|row| {
            Ok(ImportStatus {
                run_id,
                state: ImportState::parse(&row.0)?,
                archive: decode_blob_ref(&row.1)?,
                detected_layout: row.2,
                parser_id: row.3,
                records_processed: row.4,
                warning_count: row.5,
                failure_class: row.6,
                completeness_report: row.7,
            })
        })
        .transpose()
    }

    /// Inspects one received archive and advances its durable state.
    ///
    /// # Errors
    ///
    /// Returns [`ImportError`] for hostile archive input, raw evidence failure,
    /// or a lost state transition.
    pub async fn inspect(
        &self,
        run_id: Uuid,
        limits: ArchiveLimits,
    ) -> Result<ArchiveInventory, ImportError> {
        let row: Option<(String, Vec<u8>, String, i64)> = sqlx::query_as(
            "select r.state, s.archive_hash, s.archive_blob_ref, s.archive_byte_size
             from instagram_archive.import_runs r
             join instagram_archive.export_snapshots s on s.snapshot_id = r.snapshot_id
             where r.run_id = $1",
        )
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(PersistenceError::Query)
        .map_err(ReceiptError::from)?;
        let (state, digest, encoded_ref, byte_size) = row.ok_or(ImportError::StateConflict)?;
        if state != "received" {
            return Err(ImportError::StateConflict);
        }
        let blob_ref = decode_blob_ref(&encoded_ref)?;
        let path = Box::pin(self.archives.verified_path(&blob_ref, &digest, byte_size)).await?;
        let inspection = tokio::task::spawn_blocking(move || archive::inspect_file(&path, limits))
            .await
            .map_err(|_| ReceiptError::RawStorage)?;
        match inspection {
            Ok(inventory) => {
                self.finish_inspection(run_id, None).await?;
                Ok(inventory)
            }
            Err(error) => {
                self.finish_inspection(run_id, Some(error.class)).await?;
                Err(ImportError::Archive(error))
            }
        }
    }

    /// Parses one inspected archive into deterministic staging state.
    ///
    /// # Errors
    ///
    /// Returns [`ImportError`] for unsupported parser input, raw evidence
    /// failure, or a lost state transition.
    pub async fn parse(
        &self,
        run_id: Uuid,
        limits: ArchiveLimits,
    ) -> Result<ParsedExport, ImportError> {
        let row: Option<(String, Vec<u8>, String, i64)> = sqlx::query_as(
            "select r.state, s.archive_hash, s.archive_blob_ref, s.archive_byte_size
             from instagram_archive.import_runs r
             join instagram_archive.export_snapshots s on s.snapshot_id = r.snapshot_id
             where r.run_id = $1",
        )
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(PersistenceError::Query)
        .map_err(ReceiptError::from)?;
        let (state, digest, encoded_ref, byte_size) = row.ok_or(ImportError::StateConflict)?;
        if state != "inspected" {
            return Err(ImportError::StateConflict);
        }
        let blob_ref = decode_blob_ref(&encoded_ref)?;
        let path = Box::pin(self.archives.verified_path(&blob_ref, &digest, byte_size)).await?;
        let parsed = tokio::task::spawn_blocking(move || parser::parse_file(&path, limits))
            .await
            .map_err(|_| ReceiptError::RawStorage)?;
        match parsed {
            Ok(parsed) => {
                self.persist_parsed(run_id, &parsed).await?;
                Ok(parsed)
            }
            Err(error) => {
                self.fail_parse(run_id, error.class()).await?;
                Err(ImportError::Parser(error))
            }
        }
    }

    /// Reconciles one parsed run into owner-scoped projections, source facts,
    /// and an exact completeness report.
    ///
    /// # Errors
    ///
    /// Returns [`ImportError`] for persistence, contract, or state failures.
    pub async fn reconcile(&self, run_id: Uuid) -> Result<CompletenessReport, ImportError> {
        reconcile::reconcile(&self.pool, run_id).await
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one transaction makes every deterministic staging row and parsed transition atomic"
    )]
    async fn persist_parsed(&self, run_id: Uuid, parsed: &ParsedExport) -> Result<(), ImportError> {
        let processed_at = OffsetDateTime::now_utc();
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(PersistenceError::Query)
            .map_err(ReceiptError::from)?;
        let state: Option<String> = sqlx::query_scalar(
            "select state from instagram_archive.import_runs where run_id = $1 for update",
        )
        .bind(run_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)
        .map_err(ReceiptError::from)?;
        if state.as_deref() != Some("inspected") {
            return Err(ImportError::StateConflict);
        }
        for record in &parsed.records {
            let evidence_key =
                format!("normalized:{}:{}", record.shortcode, record.semantic_digest);
            let payload = serde_json::to_value(record).map_err(|_| ParserError::InvalidJson)?;
            insert_staged_record(
                &mut transaction,
                StagedRecord {
                    run_id,
                    evidence_key: &evidence_key,
                    record_kind: "normalized",
                    category: "saved_posts",
                    entry_path: parser::SAVED_POSTS_PATH,
                    entry_digest: Some(decode_sha256(&record.semantic_digest)?),
                    entry_byte_size: json_byte_size(&payload)?,
                    provider_id: Some(&record.shortcode),
                    canonical_url: Some(&record.canonical_url),
                    payload: &payload,
                    processed_at,
                },
            )
            .await?;
        }
        for entry in &parsed.unknown_entries {
            let evidence_key = format!("unknown_section:{}", entry.path);
            let payload = serde_json::to_value(entry).map_err(|_| ParserError::InvalidJson)?;
            insert_staged_record(
                &mut transaction,
                StagedRecord {
                    run_id,
                    evidence_key: &evidence_key,
                    record_kind: "unknown_section",
                    category: "unknown",
                    entry_path: &entry.path,
                    entry_digest: None,
                    entry_byte_size: i64::try_from(entry.decompressed_size)
                        .map_err(|_| ParserError::InvalidJson)?,
                    provider_id: None,
                    canonical_url: None,
                    payload: &payload,
                    processed_at,
                },
            )
            .await?;
        }
        for record in &parsed.unknown_records {
            let payload = record.raw.clone();
            insert_staged_record(
                &mut transaction,
                StagedRecord {
                    run_id,
                    evidence_key: &record.evidence_key,
                    record_kind: "unknown_record",
                    category: "saved_posts",
                    entry_path: parser::SAVED_POSTS_PATH,
                    entry_digest: Some(decode_sha256(&record.semantic_digest)?),
                    entry_byte_size: json_byte_size(&payload)?,
                    provider_id: None,
                    canonical_url: None,
                    payload: &payload,
                    processed_at,
                },
            )
            .await?;
        }
        for warning in &parsed.warnings {
            let evidence_key = format!("warning:{}:{}", warning.code, warning.evidence_key);
            let payload = serde_json::to_value(warning).map_err(|_| ParserError::InvalidJson)?;
            insert_staged_record(
                &mut transaction,
                StagedRecord {
                    run_id,
                    evidence_key: &evidence_key,
                    record_kind: "warning",
                    category: "warning",
                    entry_path: parser::SAVED_POSTS_PATH,
                    entry_digest: None,
                    entry_byte_size: json_byte_size(&payload)?,
                    provider_id: None,
                    canonical_url: None,
                    payload: &payload,
                    processed_at,
                },
            )
            .await?;
        }
        let records_processed = parsed
            .records
            .len()
            .checked_add(parsed.unknown_entries.len())
            .and_then(|count| count.checked_add(parsed.unknown_records.len()))
            .and_then(|count| count.checked_add(parsed.warnings.len()))
            .and_then(|count| i64::try_from(count).ok())
            .ok_or(ParserError::InvalidJson)?;
        let warning_count =
            i64::try_from(parsed.warnings.len()).map_err(|_| ParserError::InvalidJson)?;
        let updated = sqlx::query(
            "update instagram_archive.import_runs
             set state = 'parsed', detected_layout = $2, parser_id = $3,
                 records_processed = $4, warning_count = $5, updated_at = $6
             where run_id = $1 and state = 'inspected'",
        )
        .bind(run_id)
        .bind(parsed.detected_layout)
        .bind(parsed.parser_id)
        .bind(records_processed)
        .bind(warning_count)
        .bind(processed_at)
        .execute(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)
        .map_err(ReceiptError::from)?;
        if updated.rows_affected() != 1 {
            return Err(ImportError::StateConflict);
        }
        sqlx::query(
            "insert into instagram_archive.import_run_transitions
             (transition_id, run_id, ordinal, from_state, to_state, occurred_at)
             values ($1, $2, 3, 'inspected', 'parsed', $3)",
        )
        .bind(Uuid::now_v7())
        .bind(run_id)
        .bind(processed_at)
        .execute(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)
        .map_err(ReceiptError::from)?;
        transaction
            .commit()
            .await
            .map_err(PersistenceError::Query)
            .map_err(ReceiptError::from)?;
        Ok(())
    }

    async fn fail_parse(&self, run_id: Uuid, failure_class: &str) -> Result<(), ImportError> {
        let occurred_at = OffsetDateTime::now_utc();
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(PersistenceError::Query)
            .map_err(ReceiptError::from)?;
        let updated = sqlx::query(
            "update instagram_archive.import_runs
             set state = 'failed', failure_class = $2, updated_at = $3, finished_at = $3
             where run_id = $1 and state = 'inspected'",
        )
        .bind(run_id)
        .bind(failure_class)
        .bind(occurred_at)
        .execute(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)
        .map_err(ReceiptError::from)?;
        if updated.rows_affected() != 1 {
            return Err(ImportError::StateConflict);
        }
        sqlx::query(
            "insert into instagram_archive.import_run_transitions
             (transition_id, run_id, ordinal, from_state, to_state, failure_class, occurred_at)
             values ($1, $2, 3, 'inspected', 'failed', $3, $4)",
        )
        .bind(Uuid::now_v7())
        .bind(run_id)
        .bind(failure_class)
        .bind(occurred_at)
        .execute(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)
        .map_err(ReceiptError::from)?;
        transaction
            .commit()
            .await
            .map_err(PersistenceError::Query)
            .map_err(ReceiptError::from)?;
        Ok(())
    }

    async fn finish_inspection(
        &self,
        run_id: Uuid,
        failure: Option<ArchiveFailureClass>,
    ) -> Result<(), ImportError> {
        let occurred_at = OffsetDateTime::now_utc();
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(PersistenceError::Query)
            .map_err(ReceiptError::from)?;
        let (to_state, failure_class, finished_at) = match failure {
            Some(class) => ("failed", Some(class.as_str()), Some(occurred_at)),
            None => ("inspected", None, None),
        };
        let updated = sqlx::query(
            "update instagram_archive.import_runs
             set state = $2, failure_class = $3, updated_at = $4, finished_at = $5
             where run_id = $1 and state = 'received'",
        )
        .bind(run_id)
        .bind(to_state)
        .bind(failure_class)
        .bind(occurred_at)
        .bind(finished_at)
        .execute(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)
        .map_err(ReceiptError::from)?;
        if updated.rows_affected() != 1 {
            return Err(ImportError::StateConflict);
        }
        sqlx::query(
            "insert into instagram_archive.import_run_transitions
             (transition_id, run_id, ordinal, from_state, to_state, failure_class, occurred_at)
             values ($1, $2, 2, 'received', $3, $4, $5)",
        )
        .bind(Uuid::now_v7())
        .bind(run_id)
        .bind(to_state)
        .bind(failure_class)
        .bind(occurred_at)
        .execute(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)
        .map_err(ReceiptError::from)?;
        transaction
            .commit()
            .await
            .map_err(PersistenceError::Query)
            .map_err(ReceiptError::from)?;
        Ok(())
    }

    async fn persist_receipt(
        &self,
        user_ref: Uuid,
        stored: StoredArchive,
    ) -> Result<ReceiptOutcome, ReceiptError> {
        let received_at = OffsetDateTime::now_utc();
        let snapshot_id = Uuid::now_v7();
        let archive_ref =
            serde_json::to_string(&stored.blob_ref).map_err(|_| ReceiptError::BlobContract)?;
        let mut transaction = self.pool.begin().await.map_err(PersistenceError::Query)?;
        let created: Option<Uuid> = sqlx::query_scalar(
            "insert into instagram_archive.export_snapshots
             (snapshot_id, user_ref, archive_hash, archive_blob_ref, archive_byte_size, received_at)
             values ($1, $2, $3, $4, $5, $6)
             on conflict (user_ref, archive_hash) do nothing returning snapshot_id",
        )
        .bind(snapshot_id)
        .bind(user_ref)
        .bind(&stored.digest)
        .bind(&archive_ref)
        .bind(stored.byte_size)
        .bind(received_at)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;

        let outcome = if let Some(snapshot_id) = created {
            let run_id = Uuid::now_v7();
            sqlx::query(
                "insert into instagram_archive.import_runs
                 (run_id, snapshot_id, user_ref, state, received_at, updated_at)
                 values ($1, $2, $3, 'received', $4, $4)",
            )
            .bind(run_id)
            .bind(snapshot_id)
            .bind(user_ref)
            .bind(received_at)
            .execute(&mut *transaction)
            .await
            .map_err(PersistenceError::Query)?;
            sqlx::query(
                "insert into instagram_archive.import_run_transitions
                 (transition_id, run_id, ordinal, from_state, to_state, occurred_at)
                 values ($1, $2, 1, null, 'received', $3)",
            )
            .bind(Uuid::now_v7())
            .bind(run_id)
            .bind(received_at)
            .execute(&mut *transaction)
            .await
            .map_err(PersistenceError::Query)?;
            ReceiptOutcome::Created(ExportReceipt {
                run_id,
                state: ImportState::Received,
                archive: stored.blob_ref,
            })
        } else {
            let row: Option<(Uuid, String, String)> = sqlx::query_as(
                "select r.run_id, r.state, s.archive_blob_ref
                 from instagram_archive.export_snapshots s
                 join instagram_archive.import_runs r on r.snapshot_id = s.snapshot_id
                 where s.user_ref = $1 and s.archive_hash = $2",
            )
            .bind(user_ref)
            .bind(&stored.digest)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(PersistenceError::Query)?;
            let (run_id, state, stored_ref) = row.ok_or(ReceiptError::CorruptEvidence)?;
            let existing = decode_blob_ref(&stored_ref)?;
            if existing != stored.blob_ref {
                return Err(ReceiptError::CorruptEvidence);
            }
            ReceiptOutcome::Replayed(ExportReceipt {
                run_id,
                state: ImportState::parse(&state)?,
                archive: existing,
            })
        };
        transaction
            .commit()
            .await
            .map_err(PersistenceError::Query)?;
        Ok(outcome)
    }
}

fn decode_blob_ref(value: &str) -> Result<BlobRef, ReceiptError> {
    serde_json::from_str(value).map_err(|_| ReceiptError::CorruptEvidence)
}

struct StagedRecord<'a> {
    run_id: Uuid,
    evidence_key: &'a str,
    record_kind: &'static str,
    category: &'static str,
    entry_path: &'a str,
    entry_digest: Option<Vec<u8>>,
    entry_byte_size: i64,
    provider_id: Option<&'a str>,
    canonical_url: Option<&'a str>,
    payload: &'a serde_json::Value,
    processed_at: OffsetDateTime,
}

async fn insert_staged_record(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    row: StagedRecord<'_>,
) -> Result<(), ImportError> {
    let record_id = Uuid::new_v5(&row.run_id, row.evidence_key.as_bytes());
    sqlx::query(
        "insert into instagram_archive.export_records
         (record_id, run_id, evidence_key, record_kind, category, entry_path,
          entry_digest, entry_byte_size, provider_id, canonical_url, payload, processed_at)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(record_id)
    .bind(row.run_id)
    .bind(row.evidence_key)
    .bind(row.record_kind)
    .bind(row.category)
    .bind(row.entry_path)
    .bind(row.entry_digest.as_deref())
    .bind(row.entry_byte_size)
    .bind(row.provider_id)
    .bind(row.canonical_url)
    .bind(row.payload)
    .bind(row.processed_at)
    .execute(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)
    .map_err(ReceiptError::from)?;
    Ok(())
}

fn json_byte_size(value: &serde_json::Value) -> Result<i64, ParserError> {
    serde_json::to_vec(value)
        .map_err(|_| ParserError::InvalidJson)
        .and_then(|bytes| i64::try_from(bytes.len()).map_err(|_| ParserError::InvalidJson))
}

fn decode_sha256(value: &str) -> Result<Vec<u8>, ParserError> {
    if value.len() != 64 {
        return Err(ParserError::InvalidJson);
    }
    let mut output = Vec::with_capacity(32);
    let mut pairs = value.as_bytes().chunks_exact(2);
    for pair in &mut pairs {
        let pair = std::str::from_utf8(pair).map_err(|_| ParserError::InvalidJson)?;
        output.push(u8::from_str_radix(pair, 16).map_err(|_| ParserError::InvalidJson)?);
    }
    if !pairs.remainder().is_empty() {
        return Err(ParserError::InvalidJson);
    }
    Ok(output)
}

fn blob_ref(digest_hex: &str, length_bytes: u64) -> Result<BlobRef, ReceiptError> {
    Ok(BlobRef {
        owner_service: BlobOwner::parse("ratatoskr-instagram")
            .map_err(|_| ReceiptError::BlobContract)?,
        digest: ContentDigest {
            algorithm: DigestAlgorithm::Sha256,
            hex: DigestHex::parse(digest_hex).map_err(|_| ReceiptError::BlobContract)?,
        },
        media_type: MediaType::parse("application/zip").map_err(|_| ReceiptError::BlobContract)?,
        length_bytes,
    })
}
