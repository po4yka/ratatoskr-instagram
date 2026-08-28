//! Operational parser reprocessing, separate from database schema evolution.

use std::collections::BTreeMap;

use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::{Database, PersistenceError};

/// The supported retained-export layout.
pub const SUPPORTED_REPROCESSING_LAYOUT: &str = "instagram-json-saved-posts";
/// The exact parser registered for that layout.
pub const SUPPORTED_REPROCESSING_PARSER: &str = "instagram-saved-posts-json-v1";

/// Retained immutable export evidence required before reprocessing.
#[derive(Debug, Clone, Copy)]
pub struct RetainedExportReceipt<'a> {
    /// Original immutable archive bytes.
    pub bytes: &'a [u8],
    /// Digest recorded at receipt time.
    pub expected_hash: [u8; 32],
    /// Byte length recorded at receipt time.
    pub expected_length: u64,
    /// Detected export layout retained by the import run.
    pub detected_layout: &'a str,
}

/// Safe refusal before any derived projection is planned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReprocessingError {
    /// Retained bytes disagree with immutable receipt evidence.
    #[error("retained export receipt integrity check failed")]
    ReceiptIntegrity,
    /// No exact parser is registered for the detected export layout.
    #[error("the requested parser is not registered for this export layout")]
    UnsupportedParser,
}

/// Integrity-checked retained receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedReprocessingReceipt {
    /// Verified immutable SHA-256 digest.
    pub archive_hash: [u8; 32],
    /// Verified archive length.
    pub archive_length: u64,
}

/// Deterministic classification produced for one retained-export item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReprocessClassification {
    /// A known record would normalize successfully.
    Normalized,
    /// An unknown record remains retained as raw evidence.
    UnknownRecord,
    /// An unknown archive section remains retained as raw evidence.
    UnknownSection,
    /// An ambiguous reconciliation remains an explicit conflict.
    Conflict,
    /// A bounded parser warning was produced.
    Warning,
    /// The named parser intentionally omits this category without deleting prior state.
    Omitted,
}

impl ReprocessClassification {
    /// Returns the stable database and report vocabulary value.
    #[must_use]
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Normalized => "normalized",
            Self::UnknownRecord => "unknown_record",
            Self::UnknownSection => "unknown_section",
            Self::Conflict => "conflict",
            Self::Warning => "warning",
            Self::Omitted => "omitted",
        }
    }
}

/// Content-free planning input for one retained-export item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReprocessInput {
    /// Stable archive item key, never a private filesystem path.
    pub item_key: String,
    /// Deterministic parser and reconciliation classification.
    pub classification: ReprocessClassification,
    /// Prospective normalized digest, with no body.
    pub prospective_digest: Option<String>,
}

/// One ordered content-free report item shared by dry-run and apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReprocessReportItem {
    /// Stable archive item key.
    pub item_key: String,
    /// Deterministic outcome class.
    pub classification: ReprocessClassification,
    /// Prospective digest in dry-run and the identical applied digest in apply.
    pub digest: Option<String>,
    /// Whether apply deliberately retains an existing projection for this item.
    pub retained_prior_state: bool,
}

/// Canonical deterministic report excluding operation identity and timestamps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReprocessReport {
    /// Items ordered by stable key.
    pub items: Vec<ReprocessReportItem>,
    /// Counts keyed by closed classification vocabulary.
    pub counts: BTreeMap<String, u64>,
    /// Stable keys classified as warnings.
    pub warnings: Vec<String>,
    /// Stable keys classified as conflicts.
    pub conflicts: Vec<String>,
    /// Completeness evidence keyed by the same closed vocabulary.
    pub completeness_evidence: BTreeMap<String, u64>,
    /// Canonical plan fingerprint.
    pub plan_fingerprint: String,
    /// Caller-supplied current-state fingerprint.
    pub state_fingerprint: String,
}

/// Database-backed reprocessing operations over one retained export receipt.
#[derive(Debug)]
pub struct ReprocessingStore<'a> {
    database: &'a Database,
}

/// Restartable apply progress for one stable operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReprocessApplyOutcome {
    /// Stable persisted run identity.
    pub reprocessing_run_id: Uuid,
    /// Shared deterministic dry-run and apply report.
    pub report: ReprocessReport,
    /// Whether every planned item has a committed checkpoint.
    pub completed: bool,
}

impl<'a> ReprocessingStore<'a> {
    /// Creates a store over Instagram-owned persistence.
    #[must_use]
    pub const fn new(database: &'a Database) -> Self {
        Self { database }
    }

    /// Builds a reprocessing dry-run report for an owner-authorized receipt.
    ///
    /// # Errors
    ///
    /// Returns a persistence failure when receipt ownership cannot be read.
    pub async fn dry_run(
        &self,
        owner: Uuid,
        import_run_id: Uuid,
        inputs: &[ReprocessInput],
        state_fingerprint: &str,
    ) -> Result<ReprocessReport, PersistenceError> {
        let authorized: bool = sqlx::query_scalar(
            "select exists(select 1 from instagram_archive.import_runs \
             where run_id = $1 and user_ref = $2)",
        )
        .bind(import_run_id)
        .bind(owner)
        .fetch_one(self.database.pool())
        .await
        .map_err(PersistenceError::Query)?;
        if !authorized {
            return Err(PersistenceError::Query(sqlx::Error::RowNotFound));
        }
        Ok(migration_dry_run(inputs, state_fingerprint))
    }

    /// Applies at most `max_items` deterministic plan items for one stable operation.
    ///
    /// # Errors
    ///
    /// Returns a persistence failure when the run or checkpoint cannot commit.
    pub async fn apply_chunk(
        &self,
        owner: Uuid,
        import_run_id: Uuid,
        operation_id: Uuid,
        inputs: &[ReprocessInput],
        state_fingerprint: &str,
        max_items: usize,
    ) -> Result<ReprocessApplyOutcome, PersistenceError> {
        let report = migration_apply(inputs, state_fingerprint);
        let mut transaction = self
            .database
            .pool()
            .begin()
            .await
            .map_err(PersistenceError::Query)?;
        let (reprocessing_run_id, previous_state) = ensure_reprocessing_run(
            &mut transaction,
            owner,
            import_run_id,
            operation_id,
            &report,
        )
        .await?;
        if previous_state == "completed" || previous_state == "completed_with_warnings" {
            transaction
                .commit()
                .await
                .map_err(PersistenceError::Query)?;
            return Ok(ReprocessApplyOutcome {
                reprocessing_run_id,
                report,
                completed: true,
            });
        }
        let existing_keys: std::collections::BTreeSet<String> = sqlx::query_scalar(
            "select item_key from instagram_archive.export_reprocessing_items \
             where reprocessing_run_id = $1",
        )
        .bind(reprocessing_run_id)
        .fetch_all(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?
        .into_iter()
        .collect();
        let mut checkpoint = None;
        for item in report
            .items
            .iter()
            .filter(|item| !existing_keys.contains(&item.item_key))
            .take(max_items)
        {
            sqlx::query(
                "insert into instagram_archive.export_reprocessing_items \
                 (reprocessing_run_id, item_key, classification, state, \
                  prospective_digest, applied_digest) values ($1, $2, $3, $4, $5, $5)",
            )
            .bind(reprocessing_run_id)
            .bind(&item.item_key)
            .bind(item.classification.wire_name())
            .bind(item_state(item.classification))
            .bind(&item.digest)
            .execute(&mut *transaction)
            .await
            .map_err(PersistenceError::Query)?;
            checkpoint = Some(item.item_key.clone());
        }
        let completed =
            finish_reprocessing_run(&mut transaction, reprocessing_run_id, checkpoint, &report)
                .await?;
        transaction
            .commit()
            .await
            .map_err(PersistenceError::Query)?;
        Ok(ReprocessApplyOutcome {
            reprocessing_run_id,
            report,
            completed,
        })
    }
}

async fn ensure_reprocessing_run(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    owner: Uuid,
    import_run_id: Uuid,
    operation_id: Uuid,
    report: &ReprocessReport,
) -> Result<(Uuid, String), PersistenceError> {
    let receipt: Option<(String, String)> = sqlx::query_as(
        "select detected_layout, parser_id from instagram_archive.import_runs \
         where run_id = $1 and user_ref = $2 and state = 'reconciled'",
    )
    .bind(import_run_id)
    .bind(owner)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    if !matches!(
        receipt.as_ref(),
        Some((layout, parser))
            if layout == SUPPORTED_REPROCESSING_LAYOUT
                && parser == SUPPORTED_REPROCESSING_PARSER
    ) {
        return Err(PersistenceError::Query(sqlx::Error::RowNotFound));
    }
    let existing: Option<(Uuid, Uuid, String, String, String)> = sqlx::query_as(
        "select reprocessing_run_id, import_run_id, plan_fingerprint, state_fingerprint, state \
         from instagram_archive.export_reprocessing_runs \
         where user_ref = $1 and operation_id = $2 for update",
    )
    .bind(owner)
    .bind(operation_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    if let Some((run_id, stored_import_run_id, stored_plan, stored_state, run_state)) = existing {
        if stored_import_run_id != import_run_id
            || stored_plan != report.plan_fingerprint
            || stored_state != report.state_fingerprint
        {
            return Err(PersistenceError::Query(sqlx::Error::Protocol(
                "reprocessing plan precondition changed".to_owned(),
            )));
        }
        return Ok((run_id, run_state));
    }
    let run_id = Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.export_reprocessing_runs \
         (reprocessing_run_id, operation_id, import_run_id, user_ref, detected_layout, \
          parser_id, state, plan_fingerprint, state_fingerprint) \
         values ($1, $2, $3, $4, $5, $6, 'running', $7, $8)",
    )
    .bind(run_id)
    .bind(operation_id)
    .bind(import_run_id)
    .bind(owner)
    .bind(SUPPORTED_REPROCESSING_LAYOUT)
    .bind(SUPPORTED_REPROCESSING_PARSER)
    .bind(&report.plan_fingerprint)
    .bind(&report.state_fingerprint)
    .execute(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    Ok((run_id, "running".to_owned()))
}

async fn finish_reprocessing_run(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    reprocessing_run_id: Uuid,
    checkpoint: Option<String>,
    report: &ReprocessReport,
) -> Result<bool, PersistenceError> {
    let committed_items: i64 = sqlx::query_scalar(
        "select count(*) from instagram_archive.export_reprocessing_items \
         where reprocessing_run_id = $1",
    )
    .bind(reprocessing_run_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    let completed = usize::try_from(committed_items).is_ok_and(|count| count == report.items.len());
    let with_warnings = !report.warnings.is_empty() || !report.conflicts.is_empty();
    let state = if completed {
        if with_warnings {
            "completed_with_warnings"
        } else {
            "completed"
        }
    } else {
        "running"
    };
    let report_json = serde_json::json!({
        "counts": report.counts,
        "warnings": report.warnings,
        "conflicts": report.conflicts,
        "completeness_evidence": report.completeness_evidence,
        "plan_fingerprint": report.plan_fingerprint,
        "state_fingerprint": report.state_fingerprint,
    });
    sqlx::query(
        "update instagram_archive.export_reprocessing_runs set state = $2, \
         checkpoint_item_key = coalesce($3, checkpoint_item_key), report = $4, \
         updated_at = now(), finished_at = case when $5 then now() else null end \
         where reprocessing_run_id = $1",
    )
    .bind(reprocessing_run_id)
    .bind(state)
    .bind(checkpoint)
    .bind(report_json)
    .bind(completed)
    .execute(&mut **transaction)
    .await
    .map_err(PersistenceError::Query)?;
    Ok(completed)
}

const fn item_state(classification: ReprocessClassification) -> &'static str {
    match classification {
        ReprocessClassification::Conflict => "conflict",
        ReprocessClassification::Warning => "warning",
        ReprocessClassification::Omitted => "skipped",
        ReprocessClassification::Normalized
        | ReprocessClassification::UnknownRecord
        | ReprocessClassification::UnknownSection => "applied",
    }
}

/// Renders a read-only parser-version reprocessing report.
#[must_use]
pub fn migration_dry_run(inputs: &[ReprocessInput], state_fingerprint: &str) -> ReprocessReport {
    render_report(inputs, state_fingerprint)
}

/// Renders the report returned after applying the same proposed inputs.
#[must_use]
pub fn migration_apply(inputs: &[ReprocessInput], state_fingerprint: &str) -> ReprocessReport {
    render_report(inputs, state_fingerprint)
}

fn render_report(inputs: &[ReprocessInput], state_fingerprint: &str) -> ReprocessReport {
    let mut items = inputs
        .iter()
        .map(|input| ReprocessReportItem {
            item_key: input.item_key.clone(),
            classification: input.classification,
            digest: input.prospective_digest.clone(),
            retained_prior_state: input.classification == ReprocessClassification::Omitted,
        })
        .collect::<Vec<_>>();
    items.sort_by(|left, right| left.item_key.cmp(&right.item_key));
    let mut counts = BTreeMap::new();
    let mut warnings = Vec::new();
    let mut conflicts = Vec::new();
    for item in &items {
        *counts
            .entry(item.classification.wire_name().to_owned())
            .or_insert(0) += 1;
        match item.classification {
            ReprocessClassification::Warning => warnings.push(item.item_key.clone()),
            ReprocessClassification::Conflict => conflicts.push(item.item_key.clone()),
            _ => {}
        }
    }
    let mut canonical_plan = String::new();
    for item in &items {
        canonical_plan.push_str(&item.item_key.len().to_string());
        canonical_plan.push(':');
        canonical_plan.push_str(&item.item_key);
        canonical_plan.push('|');
        canonical_plan.push_str(item.classification.wire_name());
        canonical_plan.push('|');
        canonical_plan.push_str(item.digest.as_deref().unwrap_or("-"));
        canonical_plan.push('|');
        canonical_plan.push_str(if item.retained_prior_state {
            "retain"
        } else {
            "apply"
        });
        canonical_plan.push('\n');
    }
    let plan_fingerprint = hex(&Sha256::digest(canonical_plan.as_bytes()));
    ReprocessReport {
        items,
        completeness_evidence: counts.clone(),
        counts,
        warnings,
        conflicts,
        plan_fingerprint,
        state_fingerprint: state_fingerprint.to_owned(),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut output, byte| {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
        output
    })
}

/// Verifies the receipt and parser registry before invoking projection planning.
///
/// # Errors
///
/// Returns a typed refusal when archive identity or parser identity is not exact.
pub fn begin_reprocessing<F>(
    receipt: RetainedExportReceipt<'_>,
    parser_id: &str,
    mut projection: F,
) -> Result<VerifiedReprocessingReceipt, ReprocessingError>
where
    F: FnMut(),
{
    let actual_hash: [u8; 32] = Sha256::digest(receipt.bytes).into();
    let actual_length =
        u64::try_from(receipt.bytes.len()).map_err(|_| ReprocessingError::ReceiptIntegrity)?;
    if actual_hash != receipt.expected_hash || actual_length != receipt.expected_length {
        return Err(ReprocessingError::ReceiptIntegrity);
    }
    if receipt.detected_layout != SUPPORTED_REPROCESSING_LAYOUT
        || parser_id != SUPPORTED_REPROCESSING_PARSER
    {
        return Err(ReprocessingError::UnsupportedParser);
    }
    projection();
    Ok(VerifiedReprocessingReceipt {
        archive_hash: actual_hash,
        archive_length: actual_length,
    })
}
