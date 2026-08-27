//! Bounded restartable Data Export import worker.

use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::watch;
use uuid::Uuid;

use crate::telemetry::{
    DataExportCategory, DataExportFailure, DataExportGap, DataExportOutcome, DataExportStage,
    DataExportWarning, record_data_export_category, record_data_export_failure,
    record_data_export_gap, record_data_export_stage, record_data_export_warning,
};
use crate::{DataExportConfig, Database, PersistenceError};

use super::{
    ArchiveFailureClass, ArchiveLimits, CompletenessReport, DataExportStore, ImportError,
    ParsedExport, ParserError, ReceiptError,
};

/// Outcome of one bounded worker pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WorkerPass {
    /// Runs selected for processing.
    pub selected: u32,
    /// Runs advanced to `reconciled`.
    pub reconciled: u32,
    /// Runs advanced to terminal `failed`.
    pub failed: u32,
}

/// Restartable stage orchestrator for enabled Data Export intake.
#[derive(Debug, Clone)]
pub struct DataExportWorker {
    pool: PgPool,
    store: DataExportStore,
    limits: ArchiveLimits,
    batch_size: u32,
    poll_interval: Duration,
}

impl DataExportWorker {
    /// Builds a worker from validated enabled configuration.
    ///
    /// # Errors
    ///
    /// Returns [`ReceiptError`] if protected storage configuration is absent.
    pub fn new(database: &Database, config: &DataExportConfig) -> Result<Self, ReceiptError> {
        Ok(Self {
            pool: database.pool().clone(),
            store: DataExportStore::new(database, config)?,
            limits: ArchiveLimits {
                max_entries: config.max_entries,
                max_entry_path_bytes: config.max_entry_path_bytes,
                max_path_depth: config.max_path_depth,
                max_total_compressed_bytes: config.max_total_compressed_bytes,
                max_total_decompressed_bytes: config.max_total_decompressed_bytes,
                max_entry_decompressed_bytes: config.max_entry_decompressed_bytes,
                max_compression_ratio: config.max_compression_ratio,
            },
            batch_size: config.worker_batch_size,
            poll_interval: Duration::from_millis(config.worker_poll_interval_ms),
        })
    }

    /// Advances one stable bounded batch without holding a transaction across I/O.
    ///
    /// Compare-and-swap transitions make overlap with another worker harmless:
    /// the loser re-reads the terminal/intermediate state and does not append a
    /// second transition or projection.
    ///
    /// # Errors
    ///
    /// Returns [`ImportError`] when selection or non-input stage work fails.
    pub async fn run_once(&self) -> Result<WorkerPass, ImportError> {
        let rows: Vec<(Uuid, String)> = sqlx::query_as(
            "select run_id, state from instagram_archive.import_runs
             where state in ('received', 'inspected', 'parsed')
             order by updated_at, run_id limit $1",
        )
        .bind(i64::from(self.batch_size))
        .fetch_all(&self.pool)
        .await
        .map_err(PersistenceError::Query)
        .map_err(ReceiptError::from)?;
        let mut pass = WorkerPass {
            selected: u32::try_from(rows.len()).unwrap_or(u32::MAX),
            ..WorkerPass::default()
        };
        for (run_id, state) in rows {
            match Box::pin(self.advance(run_id, &state)).await {
                Ok("reconciled") => pass.reconciled = pass.reconciled.saturating_add(1),
                Ok("failed") => pass.failed = pass.failed.saturating_add(1),
                Ok(_) | Err(ImportError::StateConflict) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(pass)
    }

    /// Polls until shutdown, retrying transient pass failures on the next tick.
    ///
    /// Cancelling this future is safe: filesystem/decompression I/O occurs
    /// outside transactions and every durable update has a state precondition.
    pub async fn run(&self, mut shutdown: watch::Receiver<bool>) {
        let mut ticker = tokio::time::interval(self.poll_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return;
                    }
                }
                _ = ticker.tick() => {
                    if let Err(error) = Box::pin(self.run_once()).await {
                        tracing::error!(
                            error_class = worker_error_class(&error),
                            "Data Export worker pass failed"
                        );
                    }
                }
            }
        }
    }

    async fn advance(&self, run_id: Uuid, initial: &str) -> Result<&'static str, ImportError> {
        match initial {
            "received" => {
                if let Err(error) = Box::pin(self.inspect(run_id)).await {
                    return handled_input_failure(error);
                }
                if let Err(error) = Box::pin(self.parse(run_id)).await {
                    return handled_input_failure(error);
                }
                self.reconcile(run_id).await?;
                Ok("reconciled")
            }
            "inspected" => {
                if let Err(error) = Box::pin(self.parse(run_id)).await {
                    return handled_input_failure(error);
                }
                self.reconcile(run_id).await?;
                Ok("reconciled")
            }
            "parsed" => {
                self.reconcile(run_id).await?;
                Ok("reconciled")
            }
            _ => Ok(initial_state(initial)),
        }
    }

    async fn inspect(&self, run_id: Uuid) -> Result<(), ImportError> {
        let started = std::time::Instant::now();
        match Box::pin(self.store.inspect(run_id, self.limits)).await {
            Ok(_) => {
                record_data_export_stage(
                    DataExportStage::Inspect,
                    DataExportOutcome::Succeeded,
                    started.elapsed(),
                );
                Ok(())
            }
            Err(error) => {
                record_data_export_failure(import_failure(&error));
                record_data_export_stage(
                    DataExportStage::Inspect,
                    DataExportOutcome::Refused,
                    started.elapsed(),
                );
                Err(error)
            }
        }
    }

    async fn parse(&self, run_id: Uuid) -> Result<(), ImportError> {
        let started = std::time::Instant::now();
        match Box::pin(self.store.parse(run_id, self.limits)).await {
            Ok(parsed) => {
                record_parsed_counts(&parsed);
                record_data_export_stage(
                    DataExportStage::Parse,
                    DataExportOutcome::Succeeded,
                    started.elapsed(),
                );
                Ok(())
            }
            Err(error) => {
                record_data_export_failure(import_failure(&error));
                record_data_export_stage(
                    DataExportStage::Parse,
                    DataExportOutcome::Refused,
                    started.elapsed(),
                );
                Err(error)
            }
        }
    }

    async fn reconcile(&self, run_id: Uuid) -> Result<(), ImportError> {
        let started = std::time::Instant::now();
        match self.store.reconcile(run_id).await {
            Ok(report) => {
                record_report_counts(&report);
                record_data_export_stage(
                    DataExportStage::Reconcile,
                    DataExportOutcome::Succeeded,
                    started.elapsed(),
                );
                Ok(())
            }
            Err(error) => {
                record_data_export_failure(import_failure(&error));
                record_data_export_stage(
                    DataExportStage::Reconcile,
                    DataExportOutcome::Refused,
                    started.elapsed(),
                );
                Err(error)
            }
        }
    }
}

fn record_parsed_counts(parsed: &ParsedExport) {
    record_data_export_category(
        DataExportCategory::SavedPosts,
        u64::try_from(parsed.records.len()).unwrap_or(u64::MAX),
    );
    let unknown = parsed
        .unknown_entries
        .len()
        .saturating_add(parsed.unknown_records.len());
    record_data_export_category(
        DataExportCategory::Unknown,
        u64::try_from(unknown).unwrap_or(u64::MAX),
    );
    for warning in &parsed.warnings {
        let warning = match warning.code {
            "unknown_saved_record" => DataExportWarning::UnknownSavedRecord,
            "unknown_saved_section_field" => DataExportWarning::UnknownSavedSectionField,
            "unknown_archive_section" => DataExportWarning::UnknownArchiveSection,
            "media_bytes_reference_only" => DataExportWarning::MediaBytesReferenceOnly,
            _ => continue,
        };
        record_data_export_warning(warning, 1);
    }
}

fn record_report_counts(report: &CompletenessReport) {
    for (kind, count) in [
        (DataExportGap::Matched, report.matched_count()),
        (DataExportGap::ExportOnly, report.export_only_count()),
        (DataExportGap::CaptureOnly, report.capture_only_count()),
        (DataExportGap::NonComparable, report.non_comparable_count()),
    ] {
        record_data_export_gap(kind, u64::try_from(count).unwrap_or(u64::MAX));
    }
}

const fn import_failure(error: &ImportError) -> DataExportFailure {
    match error {
        ImportError::Archive(error) => match error.class {
            ArchiveFailureClass::Malformed | ArchiveFailureClass::DuplicateEntry => {
                DataExportFailure::MalformedArchive
            }
            ArchiveFailureClass::UnsafePath => DataExportFailure::UnsafeArchivePath,
            ArchiveFailureClass::UnsupportedEntryType => DataExportFailure::UnsupportedEntryType,
            ArchiveFailureClass::UnsupportedEncoding => DataExportFailure::UnsupportedEncoding,
            ArchiveFailureClass::ResourceLimit => DataExportFailure::ArchiveLimit,
        },
        ImportError::Parser(ParserError::Archive(error)) => match error.class {
            ArchiveFailureClass::Malformed | ArchiveFailureClass::DuplicateEntry => {
                DataExportFailure::MalformedArchive
            }
            ArchiveFailureClass::UnsafePath => DataExportFailure::UnsafeArchivePath,
            ArchiveFailureClass::UnsupportedEntryType => DataExportFailure::UnsupportedEntryType,
            ArchiveFailureClass::UnsupportedEncoding => DataExportFailure::UnsupportedEncoding,
            ArchiveFailureClass::ResourceLimit => DataExportFailure::ArchiveLimit,
        },
        ImportError::Parser(ParserError::UnsupportedLayout) => DataExportFailure::UnsupportedLayout,
        ImportError::Parser(ParserError::InvalidJson) => DataExportFailure::InvalidJson,
        ImportError::Publish(_) => DataExportFailure::Publish,
        ImportError::Reconciliation | ImportError::Receipt(_) => DataExportFailure::Persistence,
        ImportError::StateConflict => DataExportFailure::StateConflict,
    }
}

fn handled_input_failure(error: ImportError) -> Result<&'static str, ImportError> {
    match error {
        ImportError::Archive(_) | ImportError::Parser(_) => Ok("failed"),
        other => Err(other),
    }
}

const fn initial_state(state: &str) -> &'static str {
    match state.as_bytes() {
        b"failed" => "failed",
        b"reconciled" => "reconciled",
        _ => "intermediate",
    }
}

const fn worker_error_class(error: &ImportError) -> &'static str {
    match error {
        ImportError::Archive(_) => "archive",
        ImportError::Parser(_) => "parser",
        ImportError::Publish(_) => "publish",
        ImportError::Reconciliation => "reconciliation",
        ImportError::Receipt(_) => "receipt",
        ImportError::StateConflict => "state_conflict",
    }
}
