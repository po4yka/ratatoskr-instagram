//! Owner-scoped Data Export reconciliation and completeness reporting.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, PgPool};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{ImportError, ReceiptError, decode_blob_ref};
use crate::{PersistenceError, permalink, publishing};

/// Honest boundary attached to every completeness report.
pub(super) const AUTHORITY_DISCLAIMER: &str = "An export is one observation; it does not prove complete account history or Instagram native Saved membership, and absence does not prove unsave or deletion.";

/// Exact set-based comparison between one export and existing explicit captures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletenessReport {
    /// Stable identities observed in both sources.
    pub matched: Vec<String>,
    /// Stable identities present only in the export.
    pub export_only: Vec<String>,
    /// Comparable explicit captures absent from the export.
    pub capture_only: Vec<String>,
    /// Capture identifiers whose URL has no comparable stable post identity.
    pub non_comparable: Vec<String>,
    /// Parsed and unknown archive categories.
    pub categories: Vec<String>,
    /// Closed parser warning codes.
    pub warnings: Vec<String>,
    /// Explicit limit on what the comparison proves.
    pub authority_disclaimer: String,
}

impl CompletenessReport {
    /// Number of identities observed in both sources.
    #[must_use]
    pub fn matched_count(&self) -> usize {
        self.matched.len()
    }

    /// Number of identities observed only in the export.
    #[must_use]
    pub fn export_only_count(&self) -> usize {
        self.export_only.len()
    }

    /// Number of comparable captures absent from the export.
    #[must_use]
    pub fn capture_only_count(&self) -> usize {
        self.capture_only.len()
    }

    /// Number of captures lacking a stable comparable identity.
    #[must_use]
    pub fn non_comparable_count(&self) -> usize {
        self.non_comparable.len()
    }
}

type RunRow = (String, Uuid, OffsetDateTime, String);
type NormalizedRow = (Uuid, String, String, serde_json::Value, Vec<u8>, i64);
type MediaRow = (Uuid, String, String, Option<Uuid>);

pub(super) async fn reconcile(
    pool: &PgPool,
    run_id: Uuid,
) -> Result<CompletenessReport, ImportError> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(PersistenceError::Query)
        .map_err(ReceiptError::from)?;
    let run: Option<RunRow> = sqlx::query_as(
        "select r.state, r.user_ref, r.received_at, s.archive_blob_ref
         from instagram_archive.import_runs r
         join instagram_archive.export_snapshots s on s.snapshot_id = r.snapshot_id
         where r.run_id = $1 for update of r",
    )
    .bind(run_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(PersistenceError::Query)
    .map_err(ReceiptError::from)?;
    let (state, owner, observed_at, encoded_archive) = run.ok_or(ImportError::StateConflict)?;
    if state == "reconciled" {
        return load_report(&mut transaction, run_id).await;
    }
    if state != "parsed" {
        return Err(ImportError::StateConflict);
    }
    let archive = decode_blob_ref(&encoded_archive)?;
    let records: Vec<NormalizedRow> = sqlx::query_as(
        "select record_id, provider_id, canonical_url, payload, entry_digest, entry_byte_size
         from instagram_archive.export_records
         where run_id = $1 and record_kind = 'normalized'
           and provider_id is not null and canonical_url is not null
         order by provider_id, evidence_key",
    )
    .bind(run_id)
    .fetch_all(&mut *transaction)
    .await
    .map_err(PersistenceError::Query)
    .map_err(ReceiptError::from)?;

    for (record_id, provider_id, canonical_url, payload, digest, byte_size) in &records {
        reconcile_record(
            &mut transaction,
            run_id,
            owner,
            observed_at,
            archive.clone(),
            *record_id,
            provider_id,
            canonical_url,
            payload,
            digest,
            *byte_size,
        )
        .await?;
    }

    let report = build_report(&mut transaction, run_id, owner, &records).await?;
    persist_report(&mut transaction, run_id, observed_at, &report).await?;
    let updated = sqlx::query(
        "update instagram_archive.import_runs
         set state = 'reconciled', updated_at = $2, finished_at = $2
         where run_id = $1 and state = 'parsed'",
    )
    .bind(run_id)
    .bind(observed_at)
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
         values ($1, $2, 4, 'parsed', 'reconciled', $3)",
    )
    .bind(Uuid::now_v7())
    .bind(run_id)
    .bind(observed_at)
    .execute(&mut *transaction)
    .await
    .map_err(PersistenceError::Query)
    .map_err(ReceiptError::from)?;
    transaction
        .commit()
        .await
        .map_err(PersistenceError::Query)
        .map_err(ReceiptError::from)?;
    Ok(report)
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "all immutable staging evidence and its atomic projection remain explicit at the reconciliation boundary"
)]
async fn reconcile_record(
    transaction: &mut PgConnection,
    run_id: Uuid,
    owner: Uuid,
    observed_at: OffsetDateTime,
    archive: ratatoskr_identifiers::BlobRef,
    record_id: Uuid,
    provider_id: &str,
    canonical_url: &str,
    payload: &serde_json::Value,
    digest: &[u8],
    byte_size: i64,
) -> Result<(), ImportError> {
    let matches: Vec<MediaRow> = sqlx::query_as(
        "select media_id, acquisition_method, saved_authority, current_revision_id
         from instagram_archive.media
         where provider_media_id = $1 or permalink = $2
         order by media_id",
    )
    .bind(provider_id)
    .bind(canonical_url)
    .fetch_all(&mut *transaction)
    .await
    .map_err(PersistenceError::Query)
    .map_err(ReceiptError::from)?;
    if matches.len() > 1 {
        persist_conflict(
            transaction,
            run_id,
            record_id,
            provider_id,
            canonical_url,
            payload,
            digest,
            byte_size,
            observed_at,
        )
        .await?;
        return Ok(());
    }

    let media_id = matches.first().map_or_else(
        || publishing::source_identity(owner, canonical_url),
        |row| row.0,
    );
    if matches.is_empty() {
        sqlx::query(
            "insert into instagram_archive.media
             (media_id, provider_media_id, permalink, media_type, acquisition_method,
              saved_authority, upstream_status, created_at, updated_at)
             values ($1, $2, $3, 'unknown', 'data_export', 'export_observation',
                     'unknown', $4, $4)",
        )
        .bind(media_id)
        .bind(provider_id)
        .bind(canonical_url)
        .bind(observed_at)
        .execute(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)
        .map_err(ReceiptError::from)?;
    }

    let raw_record_id = Uuid::new_v5(&record_id, b"normalized-export-evidence");
    let revision_id = Uuid::new_v5(&record_id, b"media-revision");
    let body = serde_json::to_vec(payload).map_err(|_| ImportError::Reconciliation)?;
    let digest_hex = encode_hex(digest);
    sqlx::query(
        "insert into instagram_archive.raw_records
         (raw_record_id, record_kind, blob_ref, content_hash, byte_size, body, observed_at)
         values ($1, 'export_section', $2, $3, $4, $5, $6)
         on conflict (raw_record_id) do nothing",
    )
    .bind(raw_record_id)
    .bind(&digest_hex)
    .bind(digest)
    .bind(byte_size)
    .bind(body)
    .bind(observed_at)
    .execute(&mut *transaction)
    .await
    .map_err(PersistenceError::Query)
    .map_err(ReceiptError::from)?;
    sqlx::query(
        "insert into instagram_archive.media_revisions
         (revision_id, media_id, raw_record_id, parser_version, resolved_at)
         values ($1, $2, $3, $4, $5)
         on conflict (revision_id) do nothing",
    )
    .bind(revision_id)
    .bind(media_id)
    .bind(raw_record_id)
    .bind(super::DATA_EXPORT_PARSER_ID)
    .bind(observed_at)
    .execute(&mut *transaction)
    .await
    .map_err(PersistenceError::Query)
    .map_err(ReceiptError::from)?;

    let stronger = matches.first().is_some_and(|row| {
        !matches!(row.1.as_str(), "data_export" | "legacy_import")
            || !matches!(row.2.as_str(), "export_observation" | "legacy_observation")
    });
    if !stronger {
        sqlx::query(
            "update instagram_archive.media
             set provider_media_id = $2, permalink = $3,
                 acquisition_method = 'data_export', saved_authority = 'export_observation',
                 current_revision_id = $4, updated_at = $5
             where media_id = $1",
        )
        .bind(media_id)
        .bind(provider_id)
        .bind(canonical_url)
        .bind(revision_id)
        .bind(observed_at)
        .execute(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)
        .map_err(ReceiptError::from)?;
    }
    publishing::append_data_export_fact(
        transaction,
        media_id,
        owner,
        provider_id,
        canonical_url,
        &digest_hex,
        archive,
        observed_at,
    )
    .await?;
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "conflict rows retain every exact staging identity without hidden lookup"
)]
async fn persist_conflict(
    transaction: &mut PgConnection,
    run_id: Uuid,
    record_id: Uuid,
    provider_id: &str,
    canonical_url: &str,
    payload: &serde_json::Value,
    digest: &[u8],
    byte_size: i64,
    observed_at: OffsetDateTime,
) -> Result<(), ImportError> {
    let conflict_id = Uuid::new_v5(&record_id, b"identity-conflict");
    let evidence_key = format!("conflict:{provider_id}:{}", encode_hex(digest));
    let conflict = serde_json::json!({
        "reason": "provider_and_permalink_resolve_to_distinct_media",
        "provider_id": provider_id,
        "canonical_url": canonical_url,
        "record": payload,
    });
    sqlx::query(
        "insert into instagram_archive.export_records
         (record_id, run_id, evidence_key, record_kind, category, entry_path,
          entry_digest, entry_byte_size, provider_id, canonical_url, payload, processed_at)
         values ($1, $2, $3, 'conflict', 'saved_posts', $4, $5, $6, $7, $8, $9, $10)
         on conflict (run_id, evidence_key) do nothing",
    )
    .bind(conflict_id)
    .bind(run_id)
    .bind(evidence_key)
    .bind(super::parser::SAVED_POSTS_PATH)
    .bind(digest)
    .bind(byte_size)
    .bind(provider_id)
    .bind(canonical_url)
    .bind(conflict)
    .bind(observed_at)
    .execute(&mut *transaction)
    .await
    .map_err(PersistenceError::Query)
    .map_err(ReceiptError::from)?;
    Ok(())
}

async fn build_report(
    transaction: &mut PgConnection,
    run_id: Uuid,
    owner: Uuid,
    records: &[NormalizedRow],
) -> Result<CompletenessReport, ImportError> {
    let export: BTreeSet<String> = records.iter().map(|row| row.1.clone()).collect();
    let captures: Vec<(Uuid, String)> = sqlx::query_as(
        "select capture_id, canonical_url from instagram_archive.captures
         where user_ref = $1 and status <> 'tombstoned' order by capture_id",
    )
    .bind(owner)
    .fetch_all(&mut *transaction)
    .await
    .map_err(PersistenceError::Query)
    .map_err(ReceiptError::from)?;
    let mut comparable = BTreeSet::new();
    let mut non_comparable = BTreeSet::new();
    for (capture_id, canonical_url) in captures {
        match permalink::canonicalize(&canonical_url) {
            Ok(identity) => {
                comparable.insert(identity.shortcode);
            }
            Err(_) => {
                non_comparable.insert(capture_id.to_string());
            }
        }
    }
    let categories: Vec<String> = sqlx::query_scalar(
        "select distinct category from instagram_archive.export_records
         where run_id = $1 and record_kind <> 'warning' order by category",
    )
    .bind(run_id)
    .fetch_all(&mut *transaction)
    .await
    .map_err(PersistenceError::Query)
    .map_err(ReceiptError::from)?;
    let warnings: Vec<String> = sqlx::query_scalar(
        "select distinct payload ->> 'code' from instagram_archive.export_records
         where run_id = $1 and record_kind = 'warning'
           and payload ->> 'code' is not null order by payload ->> 'code'",
    )
    .bind(run_id)
    .fetch_all(&mut *transaction)
    .await
    .map_err(PersistenceError::Query)
    .map_err(ReceiptError::from)?;
    Ok(CompletenessReport {
        matched: export.intersection(&comparable).cloned().collect(),
        export_only: export.difference(&comparable).cloned().collect(),
        capture_only: comparable.difference(&export).cloned().collect(),
        non_comparable: non_comparable.into_iter().collect(),
        categories,
        warnings,
        authority_disclaimer: AUTHORITY_DISCLAIMER.to_owned(),
    })
}

async fn persist_report(
    transaction: &mut PgConnection,
    run_id: Uuid,
    created_at: OffsetDateTime,
    report: &CompletenessReport,
) -> Result<(), ImportError> {
    let matched = serde_json::to_value(&report.matched).map_err(|_| ImportError::Reconciliation)?;
    let export_only =
        serde_json::to_value(&report.export_only).map_err(|_| ImportError::Reconciliation)?;
    let capture_only =
        serde_json::to_value(&report.capture_only).map_err(|_| ImportError::Reconciliation)?;
    let non_comparable =
        serde_json::to_value(&report.non_comparable).map_err(|_| ImportError::Reconciliation)?;
    let categories =
        serde_json::to_value(&report.categories).map_err(|_| ImportError::Reconciliation)?;
    let warnings =
        serde_json::to_value(&report.warnings).map_err(|_| ImportError::Reconciliation)?;
    sqlx::query(
        "insert into instagram_archive.export_completeness_reports
         (report_id, run_id, matched, export_only, capture_only, non_comparable,
          matched_count, export_only_count, capture_only_count, non_comparable_count,
          categories, warnings, authority_disclaimer, created_at)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
    )
    .bind(Uuid::new_v5(&run_id, b"completeness-report"))
    .bind(run_id)
    .bind(matched)
    .bind(export_only)
    .bind(capture_only)
    .bind(non_comparable)
    .bind(i64::try_from(report.matched_count()).map_err(|_| ImportError::Reconciliation)?)
    .bind(i64::try_from(report.export_only_count()).map_err(|_| ImportError::Reconciliation)?)
    .bind(i64::try_from(report.capture_only_count()).map_err(|_| ImportError::Reconciliation)?)
    .bind(i64::try_from(report.non_comparable_count()).map_err(|_| ImportError::Reconciliation)?)
    .bind(categories)
    .bind(warnings)
    .bind(&report.authority_disclaimer)
    .bind(created_at)
    .execute(&mut *transaction)
    .await
    .map_err(PersistenceError::Query)
    .map_err(ReceiptError::from)?;
    Ok(())
}

async fn load_report(
    transaction: &mut PgConnection,
    run_id: Uuid,
) -> Result<CompletenessReport, ImportError> {
    type ReportRow = (
        serde_json::Value,
        serde_json::Value,
        serde_json::Value,
        serde_json::Value,
        serde_json::Value,
        serde_json::Value,
        String,
    );
    let row: Option<ReportRow> = sqlx::query_as(
        "select matched, export_only, capture_only, non_comparable,
                categories, warnings, authority_disclaimer
         from instagram_archive.export_completeness_reports where run_id = $1",
    )
    .bind(run_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(PersistenceError::Query)
    .map_err(ReceiptError::from)?;
    let row = row.ok_or(ImportError::Reconciliation)?;
    Ok(CompletenessReport {
        matched: serde_json::from_value(row.0).map_err(|_| ImportError::Reconciliation)?,
        export_only: serde_json::from_value(row.1).map_err(|_| ImportError::Reconciliation)?,
        capture_only: serde_json::from_value(row.2).map_err(|_| ImportError::Reconciliation)?,
        non_comparable: serde_json::from_value(row.3).map_err(|_| ImportError::Reconciliation)?,
        categories: serde_json::from_value(row.4).map_err(|_| ImportError::Reconciliation)?,
        warnings: serde_json::from_value(row.5).map_err(|_| ImportError::Reconciliation)?,
        authority_disclaimer: row.6,
    })
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}
