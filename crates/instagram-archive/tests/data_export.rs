//! Hostile archive, versioned parser, reconciliation, and completeness behavior.

#![expect(
    clippy::expect_used,
    reason = "synthetic archive and disposable-database tests fail immediately on broken setup"
)]

use std::convert::Infallible;
use std::path::{Path, PathBuf};

use futures_util::stream;
use proptest::prelude::*;
use uuid::Uuid;
use zip::CompressionMethod;

use ratatoskr_instagram_archive::Config;
use ratatoskr_instagram_archive::data_export::{
    ArchiveFailureClass, ArchiveLimits, DataExportStore, DataExportWorker, ImportError,
    ParsedExport, inspect_archive, read_archive_entry,
};
use ratatoskr_instagram_archive::test_support::{TestDatabase, data_export_zip};

const OWNER: &str = "018f1a2b-3c4d-7e6f-8a9b-0c1d2e3f4a5b";

fn test_root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "ratatoskr-instagram-data-export-domain-{}",
        Uuid::now_v7()
    ))
}

async fn parse_received(
    archive: Vec<u8>,
) -> (
    ParsedExport,
    Vec<(String, String, serde_json::Value, Option<Vec<u8>>)>,
) {
    let test = TestDatabase::create().await.expect("fresh database");
    let root = test_root();
    let store = DataExportStore::new(&test.database, &data_export_config(&root))
        .expect("validated store builds");
    let receipt = store
        .receive(
            Uuid::parse_str(OWNER).expect("owner UUID"),
            stream::iter([Ok::<Vec<u8>, Infallible>(archive)]),
        )
        .await
        .expect("synthetic export is received");
    let run_id = receipt.receipt().run_id;
    store
        .inspect(run_id, ArchiveLimits::default())
        .await
        .expect("synthetic export inspects");
    let parsed = store
        .parse(run_id, ArchiveLimits::default())
        .await
        .expect("supported export parses");
    let state: String =
        sqlx::query_scalar("select state from instagram_archive.import_runs where run_id = $1")
            .bind(run_id)
            .fetch_one(test.database.pool())
            .await
            .expect("run state reads");
    assert_eq!(state, "parsed");
    let staged: Vec<(String, String, serde_json::Value, Option<Vec<u8>>)> = sqlx::query_as(
        "select evidence_key, record_kind, payload, entry_digest
         from instagram_archive.export_records
         where run_id = $1 order by evidence_key",
    )
    .bind(run_id)
    .fetch_all(test.database.pool())
    .await
    .expect("staged records read");

    tokio::fs::remove_dir_all(&root)
        .await
        .expect("test-owned raw archive cleans up");
    test.cleanup().await.expect("cleanup must drop");
    (parsed, staged)
}

async fn parsed_run(test: &TestDatabase, root: &Path, archive: Vec<u8>) -> (DataExportStore, Uuid) {
    let store = DataExportStore::new(&test.database, &data_export_config(root))
        .expect("validated store builds");
    let receipt = store
        .receive(
            Uuid::parse_str(OWNER).expect("owner UUID"),
            stream::iter([Ok::<Vec<u8>, Infallible>(archive)]),
        )
        .await
        .expect("synthetic export is received");
    let run_id = receipt.receipt().run_id;
    store
        .inspect(run_id, ArchiveLimits::default())
        .await
        .expect("synthetic export inspects");
    store
        .parse(run_id, ArchiveLimits::default())
        .await
        .expect("synthetic export parses");
    (store, run_id)
}

async fn insert_capture(test: &TestDatabase, canonical_url: &str, status: &str) -> Uuid {
    let capture_id = Uuid::now_v7();
    sqlx::query(
        "insert into instagram_archive.captures
         (capture_id, user_ref, canonical_url, acquisition_method, saved_authority,
          client_source, status, captured_at)
         values ($1, $2, $3, 'share_extension', 'explicit_user_capture',
                 'ios_share_extension', $4, now())",
    )
    .bind(capture_id)
    .bind(Uuid::parse_str(OWNER).expect("owner UUID"))
    .bind(canonical_url)
    .bind(status)
    .execute(test.database.pool())
    .await
    .expect("synthetic capture inserts");
    capture_id
}

fn data_export_config(root: &Path) -> ratatoskr_instagram_archive::DataExportConfig {
    let config = Config::from_environment([
        (
            "RATATOSKR__BUS__URL".to_owned(),
            "nats://127.0.0.1:4222".to_owned(),
        ),
        (
            "RATATOSKR__DATA_EXPORT__ENABLED".to_owned(),
            "true".to_owned(),
        ),
        (
            "RATATOSKR__DATA_EXPORT__BLOB_ROOT".to_owned(),
            root.join("blobs").display().to_string(),
        ),
        (
            "RATATOSKR__DATA_EXPORT__STAGING_ROOT".to_owned(),
            root.join("staging").display().to_string(),
        ),
        (
            "RATATOSKR__DATA_EXPORT__BEARER_TOKENS".to_owned(),
            format!("{OWNER}:synthetic-domain-token-abcdefghijklmnopqrstuvwxyz"),
        ),
        (
            "RATATOSKR__DATA_EXPORT__MAX_BODY_BYTES".to_owned(),
            "1048576".to_owned(),
        ),
        (
            "RATATOSKR__DATA_EXPORT__MAX_TOTAL_COMPRESSED_BYTES".to_owned(),
            "1048576".to_owned(),
        ),
        (
            "RATATOSKR__DATA_EXPORT__MAX_TOTAL_DECOMPRESSED_BYTES".to_owned(),
            "4194304".to_owned(),
        ),
        (
            "RATATOSKR__DATA_EXPORT__MAX_ENTRY_DECOMPRESSED_BYTES".to_owned(),
            "1048576".to_owned(),
        ),
    ])
    .expect("bounded synthetic Data Export config");
    config.data_export
}

async fn assert_archive_refused(archive: Vec<u8>, expected: ArchiveFailureClass) {
    let test = TestDatabase::create().await.expect("fresh database");
    let root = test_root();
    let store = DataExportStore::new(&test.database, &data_export_config(&root))
        .expect("validated store builds");
    let receipt = store
        .receive(
            Uuid::parse_str(OWNER).expect("owner UUID"),
            stream::iter([Ok::<Vec<u8>, Infallible>(archive)]),
        )
        .await
        .expect("hostile bytes are preserved before inspection");
    let run_id = receipt.receipt().run_id;
    let error = store
        .inspect(run_id, ArchiveLimits::default())
        .await
        .expect_err("hostile archive must be refused");
    assert!(
        matches!(error, ImportError::Archive(ref archive_error) if archive_error.class == expected),
        "unexpected refusal: {error:?}"
    );
    let state: String =
        sqlx::query_scalar("select state from instagram_archive.import_runs where run_id = $1")
            .bind(run_id)
            .fetch_one(test.database.pool())
            .await
            .expect("run remains queryable");
    assert_eq!(state, "failed");
    let transitions: Vec<String> = sqlx::query_scalar(
        "select to_state from instagram_archive.import_run_transitions
         where run_id = $1 order by ordinal",
    )
    .bind(run_id)
    .fetch_all(test.database.pool())
    .await
    .expect("transition history reads");
    assert_eq!(transitions, ["received", "failed"]);
    let projections: i64 = sqlx::query_scalar(
        "select (select count(*) from instagram_archive.export_records where run_id = $1)
              + (select count(*) from instagram_archive.outbox_events where aggregate_id = $1)",
    )
    .bind(run_id)
    .fetch_one(test.database.pool())
    .await
    .expect("side-effect count answers");
    assert_eq!(projections, 0, "hostile archive produced projections");

    tokio::fs::remove_dir_all(&root)
        .await
        .expect("test-owned raw archive cleans up");
    test.cleanup().await.expect("cleanup must drop");
}

fn duplicate_name_archive(path: &str, replacement_path: &str) -> Vec<u8> {
    assert_eq!(
        path.len(),
        replacement_path.len(),
        "fixture names must have equal byte length"
    );
    let mut archive = data_export_zip(
        &[(path, b"{}"), (replacement_path, b"[]")],
        CompressionMethod::Stored,
    )
    .expect("two-name synthetic ZIP builds");
    let needle = replacement_path.as_bytes();
    let mut offset = 0_usize;
    let mut replaced = 0_usize;
    while let Some(relative) = archive.get(offset..).and_then(|remaining| {
        remaining
            .windows(needle.len())
            .position(|part| part == needle)
    }) {
        let start = offset + relative;
        let end = start + needle.len();
        archive
            .get_mut(start..end)
            .expect("located name range exists")
            .copy_from_slice(path.as_bytes());
        offset = end;
        replaced += 1;
    }
    assert_eq!(replaced, 2, "local and central entry names are rewritten");
    archive
}

fn forge_central_uncompressed_size(mut archive: Vec<u8>, size: u32) -> Vec<u8> {
    let signature = [0x50, 0x4b, 0x01, 0x02];
    let position = archive
        .windows(signature.len())
        .position(|window| window == signature)
        .expect("central directory header exists");
    archive
        .get_mut(position + 24..position + 28)
        .expect("central size field exists")
        .copy_from_slice(&size.to_le_bytes());
    archive
}

fn set_local_encryption_flag(mut archive: Vec<u8>) -> Vec<u8> {
    let signature = [0x50, 0x4b, 0x03, 0x04];
    let position = archive
        .windows(signature.len())
        .position(|window| window == signature)
        .expect("local entry header exists");
    let flags = archive
        .get(position + 6..position + 8)
        .and_then(|bytes| <[u8; 2]>::try_from(bytes).ok())
        .map(u16::from_le_bytes)
        .expect("local flags exist")
        | 1;
    archive
        .get_mut(position + 6..position + 8)
        .expect("local flags remain writable")
        .copy_from_slice(&flags.to_le_bytes());
    archive
}

fn forge_encryption_flags(mut archive: Vec<u8>) -> Vec<u8> {
    for (signature, offset) in [([0x50, 0x4b, 0x03, 0x04], 6), ([0x50, 0x4b, 0x01, 0x02], 8)] {
        let position = archive
            .windows(signature.len())
            .position(|window| window == signature)
            .expect("ZIP header exists");
        archive
            .get_mut(position + offset..position + offset + 2)
            .expect("flags field exists")
            .copy_from_slice(&1_u16.to_le_bytes());
    }
    archive
}

fn forge_compression_method(mut archive: Vec<u8>, method: u16) -> Vec<u8> {
    for (signature, offset) in [
        ([0x50, 0x4b, 0x03, 0x04], 8),
        ([0x50, 0x4b, 0x01, 0x02], 10),
    ] {
        let position = archive
            .windows(signature.len())
            .position(|window| window == signature)
            .expect("ZIP header exists");
        archive
            .get_mut(position + offset..position + offset + 2)
            .expect("method field exists")
            .copy_from_slice(&method.to_le_bytes());
    }
    archive
}

fn forge_symlink_entry(mut archive: Vec<u8>) -> Vec<u8> {
    let signature = [0x50, 0x4b, 0x01, 0x02];
    let position = archive
        .windows(signature.len())
        .position(|window| window == signature)
        .expect("central header exists");
    *archive
        .get_mut(position + 5)
        .expect("creator system byte exists") = 3;
    archive
        .get_mut(position + 38..position + 42)
        .expect("external attributes exist")
        .copy_from_slice(&(0o120_777_u32 << 16).to_le_bytes());
    archive
}

fn forge_directory_offset(mut archive: Vec<u8>, offset: u32) -> Vec<u8> {
    let signature = [0x50, 0x4b, 0x05, 0x06];
    let position = archive
        .windows(signature.len())
        .rposition(|window| window == signature)
        .expect("end-of-central-directory exists");
    archive
        .get_mut(position + 16..position + 20)
        .expect("directory offset exists")
        .copy_from_slice(&offset.to_le_bytes());
    archive
}

#[tokio::test]
async fn zip_slip_and_ambiguous_paths_are_refused_without_writes() {
    for name in [
        "../escape.json",
        "/absolute.json",
        "drive\\escape.json",
        "a//ambiguous.json",
        "a/./ambiguous.json",
        "a/../ambiguous.json",
        "C:/windows.json",
    ] {
        let archive = data_export_zip(
            &[(name, br#"{"synthetic":true}"#)],
            CompressionMethod::Stored,
        )
        .expect("hostile synthetic ZIP builds");
        assert_archive_refused(archive, ArchiveFailureClass::UnsafePath).await;
    }
}

#[tokio::test]
async fn duplicate_normalized_entries_are_refused_before_parser() {
    let path = "your_instagram_activity/saved/saved_posts.json";
    let archive = duplicate_name_archive(path, "your_instagram_activity/saved/saved_postx.json");
    assert_archive_refused(archive, ArchiveFailureClass::DuplicateEntry).await;
}

#[test]
fn entry_count_limit_is_exact() {
    let archive = data_export_zip(
        &[("one.json", b"1"), ("two.json", b"2")],
        CompressionMethod::Stored,
    )
    .expect("two-entry synthetic ZIP builds");
    let accepted = inspect_archive(
        &archive,
        ArchiveLimits {
            max_entries: 2,
            ..ArchiveLimits::default()
        },
    )
    .expect("exact entry-count boundary is accepted");
    assert_eq!(accepted.entries.len(), 2);
    let refused = inspect_archive(
        &archive,
        ArchiveLimits {
            max_entries: 1,
            ..ArchiveLimits::default()
        },
    )
    .expect_err("one excessive entry is refused");
    assert_eq!(refused.class, ArchiveFailureClass::ResourceLimit);
    assert_eq!(refused.rule, "entry_count");
}

#[test]
fn declared_and_actual_decompressed_limits_are_exact() {
    let path = "your_instagram_activity/saved/saved_posts.json";
    let body = vec![b'x'; 128];
    let honest = data_export_zip(&[(path, &body)], CompressionMethod::Deflated)
        .expect("bounded deflated ZIP builds");
    let exact_limits = ArchiveLimits {
        max_total_decompressed_bytes: 128,
        max_entry_decompressed_bytes: 128,
        max_compression_ratio: 1_000,
        ..ArchiveLimits::default()
    };
    let emitted = read_archive_entry(&honest, path, exact_limits)
        .expect("exact actual decompressed boundary is accepted");
    assert_eq!(emitted, body);

    let forged = forge_central_uncompressed_size(honest, 1);
    inspect_archive(
        &forged,
        ArchiveLimits {
            max_total_decompressed_bytes: 32,
            max_entry_decompressed_bytes: 32,
            max_compression_ratio: 1_000,
            ..ArchiveLimits::default()
        },
    )
    .expect("forged declared size alone appears within bounds");
    let refused = read_archive_entry(
        &forged,
        path,
        ArchiveLimits {
            max_total_decompressed_bytes: 32,
            max_entry_decompressed_bytes: 32,
            max_compression_ratio: 1_000,
            ..ArchiveLimits::default()
        },
    )
    .expect_err("actual emitted bytes must enforce the same exact boundary");
    assert_eq!(refused.class, ArchiveFailureClass::ResourceLimit);
    assert_eq!(refused.rule, "actual_entry_decompressed_bytes");
}

#[test]
fn compression_ratio_limit_is_exact() {
    let archive = data_export_zip(
        &[("zeros.json", &vec![0_u8; 4_096])],
        CompressionMethod::Deflated,
    )
    .expect("high-ratio synthetic ZIP builds");
    let inventory = inspect_archive(
        &archive,
        ArchiveLimits {
            max_compression_ratio: 1_000,
            ..ArchiveLimits::default()
        },
    )
    .expect("high ceiling inventories the ratio fixture");
    let entry = inventory.entries.first().expect("one entry exists");
    let exact_ratio = entry
        .decompressed_size
        .saturating_add(entry.compressed_size.saturating_sub(1))
        / entry.compressed_size;
    inspect_archive(
        &archive,
        ArchiveLimits {
            max_compression_ratio: exact_ratio,
            ..ArchiveLimits::default()
        },
    )
    .expect("ceiling ratio is inclusive");
    let refused = inspect_archive(
        &archive,
        ArchiveLimits {
            max_compression_ratio: exact_ratio.saturating_sub(1),
            ..ArchiveLimits::default()
        },
    )
    .expect_err("one ratio unit below the ceiling is refused");
    assert_eq!(refused.class, ArchiveFailureClass::ResourceLimit);
    assert_eq!(refused.rule, "compression_ratio");
}

#[test]
fn hostile_headers_entry_types_and_truncated_streams_are_refused() {
    let regular = data_export_zip(
        &[("safe.json", br#"{"synthetic":true}"#)],
        CompressionMethod::Stored,
    )
    .expect("regular fixture builds");
    let encrypted = inspect_archive(
        &forge_encryption_flags(regular.clone()),
        ArchiveLimits::default(),
    )
    .expect_err("encrypted entry is refused");
    assert_eq!(encrypted.class, ArchiveFailureClass::UnsupportedEncoding);
    assert_eq!(encrypted.rule, "encrypted_entry");

    let unsupported = inspect_archive(
        &forge_compression_method(regular.clone(), 99),
        ArchiveLimits::default(),
    )
    .expect_err("unsupported compression is refused");
    assert_eq!(unsupported.class, ArchiveFailureClass::UnsupportedEncoding);
    assert_eq!(unsupported.rule, "compression_method");

    let symlink = inspect_archive(
        &forge_symlink_entry(regular.clone()),
        ArchiveLimits::default(),
    )
    .expect_err("symlink is refused");
    assert_eq!(symlink.class, ArchiveFailureClass::UnsupportedEntryType);
    assert_eq!(symlink.rule, "regular_files_only");

    let truncated = regular
        .get(..regular.len().saturating_sub(9))
        .expect("bounded truncation slice exists");
    let truncated = inspect_archive(truncated, ArchiveLimits::default())
        .expect_err("truncated archive is refused");
    assert_eq!(truncated.class, ArchiveFailureClass::Malformed);

    let invalid_offset = inspect_archive(
        &forge_directory_offset(regular, u32::MAX),
        ArchiveLimits::default(),
    )
    .expect_err("out-of-bounds directory arithmetic is refused");
    assert_eq!(invalid_offset.class, ArchiveFailureClass::ResourceLimit);
    assert_eq!(invalid_offset.rule, "zip64_directory");
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn arbitrary_archive_bytes_never_panic_or_escape_limits(
        arbitrary in prop::collection::vec(any::<u8>(), 0..8_192),
        payload in prop::collection::vec(any::<u8>(), 0..512),
    ) {
        let limits = ArchiveLimits {
            max_entries: 8,
            max_entry_path_bytes: 128,
            max_path_depth: 4,
            max_total_compressed_bytes: 8_192,
            max_total_decompressed_bytes: 8_192,
            max_entry_decompressed_bytes: 1_024,
            max_compression_ratio: 100,
        };
        let total = std::panic::catch_unwind(|| inspect_archive(&arbitrary, limits));
        prop_assert!(total.is_ok(), "inspector panicked on arbitrary bytes");
        if let Ok(Ok(inventory)) = total {
            prop_assert!(inventory.entries.len() <= limits.max_entries);
            prop_assert!(inventory.total_compressed_bytes <= limits.max_total_compressed_bytes);
            prop_assert!(
                inventory.total_decompressed_bytes <= limits.max_total_decompressed_bytes
            );
            for entry in inventory.entries {
                prop_assert!(entry.path.len() <= limits.max_entry_path_bytes);
                prop_assert!(entry.path.split('/').count() <= limits.max_path_depth);
                prop_assert!(!entry.path.contains(".."));
                prop_assert!(!entry.path.contains('\\'));
            }
        }

        let valid = data_export_zip(
            &[("safe.json", payload.as_slice())],
            CompressionMethod::Stored,
        )
        .expect("synthetic property ZIP builds");
        let ambiguous = set_local_encryption_flag(valid);
        let refusal = inspect_archive(&ambiguous, limits);
        prop_assert!(
            refusal.is_err(),
            "inconsistent local/central encryption flags escaped metadata inspection"
        );
    }
}

#[tokio::test]
async fn supported_fixture_parser_is_deterministic_across_entry_and_json_order() {
    const PATH: &str = "your_instagram_activity/saved/saved_posts.json";
    let first_json = br#"{
        "saved_saved_media": [
            {"title":"synthetic_beta","string_map_data":{"Saved on":{"href":"https://instagram.com/reel/SYNTHETIC02/?utm_source=fixture","timestamp":1700000100}}},
            {"title":"synthetic_alpha","string_map_data":{"Saved on":{"href":"https://www.instagram.com/p/SYNTHETIC01/","timestamp":1700000000}}}
        ]
    }"#;
    let second_json = br#"{
        "saved_saved_media": [
            {"string_map_data":{"Saved on":{"timestamp":1700000000,"href":"https://www.instagram.com/p/SYNTHETIC01/"}},"title":"synthetic_alpha"},
            {"string_map_data":{"Saved on":{"timestamp":1700000100,"href":"https://instagram.com/reel/SYNTHETIC02/"}},"title":"synthetic_beta"}
        ]
    }"#;
    let unknown = br#"{"synthetic_unknown":true}"#;
    let first = data_export_zip(
        &[
            ("account_information/profile.json", unknown),
            (PATH, first_json),
        ],
        CompressionMethod::Deflated,
    )
    .expect("first equivalent export builds");
    let second = data_export_zip(
        &[
            (PATH, second_json),
            ("account_information/profile.json", unknown),
        ],
        CompressionMethod::Deflated,
    )
    .expect("second equivalent export builds");

    let (first_parsed, first_staged) = parse_received(first).await;
    let (second_parsed, second_staged) = parse_received(second).await;
    assert_eq!(
        first_parsed, second_parsed,
        "parser output is order-independent"
    );
    assert_eq!(
        first_staged, second_staged,
        "staging order/content is deterministic"
    );
    assert_eq!(first_parsed.parser_id, "instagram-saved-posts-json-v1");
    assert_eq!(first_parsed.categories, ["saved_posts"]);
    assert_eq!(first_parsed.records.len(), 2);
    assert_eq!(first_parsed.records[0].shortcode, "SYNTHETIC01");
    assert_eq!(first_parsed.records[1].shortcode, "SYNTHETIC02");
    for record in &first_parsed.records {
        assert_eq!(record.acquisition_method, "data_export");
        assert_eq!(record.saved_authority, "export_observation");
        let serialized = serde_json::to_value(record).expect("record serializes");
        assert!(serialized.get("captured_at").is_none());
        assert!(serialized.get("published_at").is_none());
    }
}

#[tokio::test]
async fn unknown_sections_and_records_are_retained_with_warning() {
    const PATH: &str = "your_instagram_activity/saved/saved_posts.json";
    let json = br#"{
        "saved_saved_media": [
            {"title":"synthetic_alpha","string_map_data":{"Saved on":{"href":"https://www.instagram.com/p/SYNTHETIC01/","timestamp":1700000000}}},
            {"synthetic_future_shape":{"opaque":true,"count":7}}
        ]
    }"#;
    let archive = data_export_zip(
        &[
            (PATH, json),
            ("future_category/opaque.json", br#"{"future":true}"#),
        ],
        CompressionMethod::Deflated,
    )
    .expect("unknown-evidence export builds");
    let (parsed, staged) = parse_received(archive).await;

    assert_eq!(parsed.records.len(), 1);
    assert_eq!(parsed.unknown_records.len(), 1);
    assert_eq!(parsed.unknown_entries.len(), 1);
    assert!(
        parsed
            .warnings
            .iter()
            .any(|warning| warning.code == "unknown_saved_record")
    );
    assert!(
        parsed
            .warnings
            .iter()
            .any(|warning| warning.code == "unknown_archive_section")
    );
    let unknown_record = staged
        .iter()
        .find(|(_, kind, _, _)| kind == "unknown_record")
        .expect("unknown record is staged, not discarded");
    assert_eq!(unknown_record.2, parsed.unknown_records[0].raw);
    assert!(
        unknown_record.3.is_some(),
        "bounded unknown JSON has a digest"
    );
    let unknown_section = staged
        .iter()
        .find(|(_, kind, _, _)| kind == "unknown_section")
        .expect("unknown section is staged by archive reference");
    assert!(
        unknown_section.3.is_none(),
        "unknown section bytes were not expanded merely to manufacture a BlobRef"
    );
}

#[tokio::test]
async fn unsupported_layout_fails_without_guessed_output() {
    let test = TestDatabase::create().await.expect("fresh database");
    let root = test_root();
    let store = DataExportStore::new(&test.database, &data_export_config(&root))
        .expect("validated store builds");
    let archive = data_export_zip(
        &[("future_category/only.json", br#"{"future":true}"#)],
        CompressionMethod::Deflated,
    )
    .expect("unsupported synthetic export builds");
    let receipt = store
        .receive(
            Uuid::parse_str(OWNER).expect("owner UUID"),
            stream::iter([Ok::<Vec<u8>, Infallible>(archive)]),
        )
        .await
        .expect("unsupported bytes are still preserved");
    let run_id = receipt.receipt().run_id;
    let digest = receipt.receipt().archive.digest.hex.as_str().to_owned();
    store
        .inspect(run_id, ArchiveLimits::default())
        .await
        .expect("safe unknown layout inspects");
    let error = store
        .parse(run_id, ArchiveLimits::default())
        .await
        .expect_err("unsupported layout must not be guessed");
    assert!(matches!(
        error,
        ImportError::Parser(
            ratatoskr_instagram_archive::data_export::ParserError::UnsupportedLayout
        )
    ));
    let row: (String, Option<String>, i64) = sqlx::query_as(
        "select state, failure_class,
                (select count(*) from instagram_archive.export_records where run_id = $1)
         from instagram_archive.import_runs where run_id = $1",
    )
    .bind(run_id)
    .fetch_one(test.database.pool())
    .await
    .expect("failed run evidence reads");
    assert_eq!(
        row,
        (
            "failed".to_owned(),
            Some("parser_unsupported_layout".to_owned()),
            0
        )
    );
    assert!(
        root.join("blobs").join("sha256").join(digest).is_file(),
        "terminal parser refusal retains immutable raw archive"
    );

    tokio::fs::remove_dir_all(&root)
        .await
        .expect("test-owned raw archive cleans up");
    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn referenced_media_remains_metadata_only() {
    const PATH: &str = "your_instagram_activity/saved/saved_posts.json";
    let archive = data_export_zip(
        &[
            (
                PATH,
                include_bytes!("fixtures/data_export/saved_posts.json"),
            ),
            (
                "media/saved/2023/synthetic_fixture.jpg",
                b"synthetic bytes are never interpreted as image media",
            ),
        ],
        CompressionMethod::Deflated,
    )
    .expect("metadata-only media fixture export builds");
    let (parsed, staged) = parse_received(archive).await;
    let media = parsed
        .unknown_entries
        .iter()
        .find(|entry| entry.path.ends_with("synthetic_fixture.jpg"))
        .expect("media reference remains in the archive inventory");
    let media = serde_json::to_value(media).expect("media metadata serializes");
    assert_eq!(media["media_reference"], true);
    assert_eq!(media["byte_status"], "not_archived_separately");
    assert!(media.get("blob_ref").is_none());
    assert!(media.get("content_digest").is_none());
    let staged_media = staged
        .iter()
        .find(|(_, kind, payload, _)| {
            kind == "unknown_section" && payload["path"] == "media/saved/2023/synthetic_fixture.jpg"
        })
        .expect("media reference is staged as unknown section metadata");
    assert!(
        staged_media.3.is_none(),
        "media bytes must not be represented as a separate stored BlobRef"
    );
}
#[path = "data_export/reconciliation.rs"]
mod reconciliation;
