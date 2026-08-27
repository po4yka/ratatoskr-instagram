//! Versioned deterministic parser for one explicit Instagram export shape.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use time::OffsetDateTime;

use crate::permalink;

use super::archive;
use super::{ArchiveError, ArchiveInventory, ArchiveLimits, inspect_archive, read_archive_entry};

pub(super) const SAVED_POSTS_PATH: &str = "your_instagram_activity/saved/saved_posts.json";
const DETECTED_LAYOUT: &str = "instagram-json-saved-posts";

/// Exact identifier persisted with output from the first supported parser.
pub const DATA_EXPORT_PARSER_ID: &str = "instagram-saved-posts-json-v1";

/// One normalized export observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParsedExportRecord {
    /// Stable Instagram shortcode identity.
    pub shortcode: String,
    /// Canonical Instagram permalink.
    pub canonical_url: String,
    /// Optional bounded display author supplied by the export.
    pub display_author: Option<String>,
    /// Provider-export observation timestamp in Unix seconds.
    pub observed_at_unix: i64,
    /// Acquisition method fixed by this adapter.
    pub acquisition_method: &'static str,
    /// Maximum authority established by an export observation.
    pub saved_authority: &'static str,
    /// Stable SHA-256 of canonical normalized record content.
    pub semantic_digest: String,
}

/// Unknown archive entry retained by reference to the immutable archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnknownExportEntry {
    /// Exact safely normalized entry identity.
    pub path: String,
    /// Declared compressed size.
    pub compressed_size: u64,
    /// Declared decompressed size.
    pub decompressed_size: u64,
    /// Whether the path is a provider media-tree reference, without MIME validation.
    pub media_reference: bool,
    /// Explicit byte-handling status; no separate media object is created.
    pub byte_status: &'static str,
}

/// Bounded unrecognized JSON record retained inside the known raw section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnknownExportRecord {
    /// Stable digest-derived evidence key, independent of JSON array order.
    pub evidence_key: String,
    /// SHA-256 of canonical JSON value bytes.
    pub semantic_digest: String,
    /// Exact bounded JSON value retained for a future parser version.
    pub raw: Value,
}

/// Closed parser warning carrying only a deterministic evidence identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParserWarning {
    /// Closed warning code.
    pub code: &'static str,
    /// Stable non-content evidence key.
    pub evidence_key: String,
}

/// Pure, sorted output of one versioned parser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParsedExport {
    /// Exact parser implementation identifier.
    pub parser_id: &'static str,
    /// Exact detected provider layout identifier.
    pub detected_layout: &'static str,
    /// Normalized records sorted by stable identity.
    pub records: Vec<ParsedExportRecord>,
    /// Categories actually observed in this archive.
    pub categories: Vec<String>,
    /// Unknown entries sorted by path and retained through the archive `BlobRef`.
    pub unknown_entries: Vec<UnknownExportEntry>,
    /// Unknown bounded records sorted by canonical digest/evidence key.
    pub unknown_records: Vec<UnknownExportRecord>,
    /// Typed warnings sorted by code/evidence identity.
    pub warnings: Vec<ParserWarning>,
    /// Semantic digest of the recognized bounded JSON section.
    pub known_entry_digest: String,
}

/// Closed version/layout/JSON parser refusal.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParserError {
    /// Archive safety validation or bounded entry read failed.
    #[error("archive safety refusal")]
    Archive(#[from] ArchiveError),
    /// The one explicit saved-post layout was absent.
    #[error("unsupported Instagram export layout")]
    UnsupportedLayout,
    /// The recognized bounded JSON section was malformed.
    #[error("invalid saved-post JSON")]
    InvalidJson,
}

impl ParserError {
    /// Closed database and telemetry failure spelling.
    #[must_use]
    pub const fn class(&self) -> &'static str {
        match self {
            Self::Archive(_) => "parser_archive_refused",
            Self::UnsupportedLayout => "parser_unsupported_layout",
            Self::InvalidJson => "parser_invalid_json",
        }
    }
}

/// Parses the exact supported saved-post export shape.
///
/// # Errors
///
/// Returns [`ParserError`] for unsafe archive input, unsupported layout, or
/// malformed recognized JSON.
pub fn parse_export(bytes: &[u8], limits: ArchiveLimits) -> Result<ParsedExport, ParserError> {
    let inventory = inspect_archive(bytes, limits)?;
    let json = read_archive_entry(bytes, SAVED_POSTS_PATH, limits)
        .map_err(|error| missing_or_archive(&inventory, error))?;
    parse_saved_posts(&json, inventory)
}

pub(super) fn parse_file(path: &Path, limits: ArchiveLimits) -> Result<ParsedExport, ParserError> {
    let inventory = archive::inspect_file(path, limits)?;
    let json = archive::read_file_entry(path, SAVED_POSTS_PATH, limits)
        .map_err(|error| missing_or_archive(&inventory, error))?;
    parse_saved_posts(&json, inventory)
}

fn missing_or_archive(inventory: &ArchiveInventory, error: ArchiveError) -> ParserError {
    if inventory
        .entries
        .iter()
        .any(|entry| entry.path == SAVED_POSTS_PATH)
    {
        ParserError::Archive(error)
    } else {
        ParserError::UnsupportedLayout
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one pure parser function keeps the exact admitted grammar and unknown retention visible"
)]
fn parse_saved_posts(
    bytes: &[u8],
    inventory: ArchiveInventory,
) -> Result<ParsedExport, ParserError> {
    let Value::Object(mut root) =
        serde_json::from_slice(bytes).map_err(|_| ParserError::InvalidJson)?
    else {
        return Err(ParserError::InvalidJson);
    };
    let saved = root
        .remove("saved_saved_media")
        .ok_or(ParserError::UnsupportedLayout)?;
    let Value::Array(saved) = saved else {
        return Err(ParserError::InvalidJson);
    };
    let mut records = Vec::new();
    let mut pending_unknown = Vec::new();
    let mut warnings = Vec::new();
    for value in saved {
        let Ok(wire): Result<SavedMedia, _> = serde_json::from_value(value.clone()) else {
            pending_unknown.push(("unknown_saved_record", value));
            continue;
        };
        let Ok(canonical) = permalink::canonicalize(&wire.string_map_data.saved_on.href) else {
            pending_unknown.push(("invalid_saved_permalink", value));
            continue;
        };
        if OffsetDateTime::from_unix_timestamp(wire.string_map_data.saved_on.timestamp).is_err() {
            pending_unknown.push(("invalid_observation_timestamp", value));
            continue;
        }
        let display_author = bounded_display_author(&wire.title);
        let digest_input = SemanticRecord {
            shortcode: &canonical.shortcode,
            canonical_url: &canonical.url,
            display_author: display_author.as_deref(),
            observed_at_unix: wire.string_map_data.saved_on.timestamp,
            acquisition_method: "data_export",
            saved_authority: "export_observation",
        };
        let semantic_digest = digest_json(&digest_input)?;
        records.push(ParsedExportRecord {
            shortcode: canonical.shortcode,
            canonical_url: canonical.url,
            display_author,
            observed_at_unix: wire.string_map_data.saved_on.timestamp,
            acquisition_method: "data_export",
            saved_authority: "export_observation",
            semantic_digest,
        });
    }
    records.sort_by(|left, right| {
        left.shortcode
            .cmp(&right.shortcode)
            .then_with(|| left.semantic_digest.cmp(&right.semantic_digest))
    });
    for (field, value) in root {
        let mut raw = serde_json::Map::new();
        raw.insert(field, value);
        pending_unknown.push(("unknown_saved_section_field", Value::Object(raw)));
    }
    let mut pending_unknown = pending_unknown
        .into_iter()
        .map(|(code, raw)| Ok((code, digest_json(&raw)?, raw)))
        .collect::<Result<Vec<_>, ParserError>>()?;
    pending_unknown.sort_by(|left, right| left.1.cmp(&right.1));
    let mut unknown_records = Vec::with_capacity(pending_unknown.len());
    for (ordinal, (code, semantic_digest, raw)) in pending_unknown.into_iter().enumerate() {
        let evidence_key = format!("unknown_record:{semantic_digest}:{ordinal}");
        warnings.push(ParserWarning {
            code,
            evidence_key: evidence_key.clone(),
        });
        unknown_records.push(UnknownExportRecord {
            evidence_key,
            semantic_digest,
            raw,
        });
    }
    let mut unknown_entries = inventory
        .entries
        .into_iter()
        .filter(|entry| entry.path != SAVED_POSTS_PATH)
        .map(|entry| {
            let media_reference = media_reference_path(&entry.path);
            UnknownExportEntry {
                path: entry.path,
                compressed_size: entry.compressed_size,
                decompressed_size: entry.decompressed_size,
                media_reference,
                byte_status: if media_reference {
                    "not_archived_separately"
                } else {
                    "not_applicable"
                },
            }
        })
        .collect::<Vec<_>>();
    unknown_entries.sort_by(|left, right| left.path.cmp(&right.path));
    warnings.extend(unknown_entries.iter().map(|entry| ParserWarning {
        code: "unknown_archive_section",
        evidence_key: format!("entry:{}", entry.path),
    }));
    warnings.extend(
        unknown_entries
            .iter()
            .filter(|entry| entry.media_reference)
            .map(|entry| ParserWarning {
                code: "media_bytes_reference_only",
                evidence_key: format!("entry:{}", entry.path),
            }),
    );
    warnings.sort_by(|left, right| {
        left.code
            .cmp(right.code)
            .then_with(|| left.evidence_key.cmp(&right.evidence_key))
    });
    let known_entry_digest = digest_json(&(&records, &unknown_records))?;
    Ok(ParsedExport {
        parser_id: DATA_EXPORT_PARSER_ID,
        detected_layout: DETECTED_LAYOUT,
        records,
        categories: vec!["saved_posts".to_owned()],
        unknown_entries,
        unknown_records,
        warnings,
        known_entry_digest,
    })
}

fn bounded_display_author(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty() && trimmed.len() <= 256 && !trimmed.chars().any(char::is_control))
        .then(|| trimmed.to_owned())
}

fn media_reference_path(path: &str) -> bool {
    let extension = path
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase());
    path.starts_with("media/")
        && extension.as_deref().is_some_and(|extension| {
            matches!(
                extension,
                "jpg" | "jpeg" | "png" | "webp" | "gif" | "mp4" | "mov"
            )
        })
}

fn digest_json<T: Serialize>(value: &T) -> Result<String, ParserError> {
    let bytes = serde_json::to_vec(value).map_err(|_| ParserError::InvalidJson)?;
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    Ok(output)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedMedia {
    title: String,
    string_map_data: SavedStringMap,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedStringMap {
    #[serde(rename = "Saved on")]
    saved_on: SavedOn,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedOn {
    href: String,
    timestamp: i64,
}

#[derive(Serialize)]
struct SemanticRecord<'a> {
    shortcode: &'a str,
    canonical_url: &'a str,
    display_author: Option<&'a str>,
    observed_at_unix: i64,
    acquisition_method: &'static str,
    saved_authority: &'static str,
}
