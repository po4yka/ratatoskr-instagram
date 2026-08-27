//! Metadata-first hostile ZIP validation and bounded entry reads.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde::Serialize;
use zip::{CompressionMethod, ZipArchive};

/// Finite ZIP inspection and decompression limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveLimits {
    /// Largest accepted entry count.
    pub max_entries: usize,
    /// Largest accepted raw entry-name byte length.
    pub max_entry_path_bytes: usize,
    /// Largest accepted normalized path component count.
    pub max_path_depth: usize,
    /// Largest cumulative declared compressed bytes.
    pub max_total_compressed_bytes: u64,
    /// Largest cumulative declared and emitted decompressed bytes.
    pub max_total_decompressed_bytes: u64,
    /// Largest declared and emitted decompressed bytes for one entry.
    pub max_entry_decompressed_bytes: u64,
    /// Largest decompressed-to-compressed ratio.
    pub max_compression_ratio: u64,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            max_entries: 20_000,
            max_entry_path_bytes: 1_024,
            max_path_depth: 12,
            max_total_compressed_bytes: 5 * 1024 * 1024 * 1024,
            max_total_decompressed_bytes: 20 * 1024 * 1024 * 1024,
            max_entry_decompressed_bytes: 64 * 1024 * 1024,
            max_compression_ratio: 200,
        }
    }
}

/// One validated regular-file ZIP entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InspectedEntry {
    /// Exact validated UTF-8 archive path.
    pub path: String,
    /// Declared compressed bytes.
    pub compressed_size: u64,
    /// Declared decompressed bytes.
    pub decompressed_size: u64,
}

/// Complete metadata inventory accepted before parsing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArchiveInventory {
    /// Entries sorted by exact validated path.
    pub entries: Vec<InspectedEntry>,
    /// Checked cumulative declared compressed bytes.
    pub total_compressed_bytes: u64,
    /// Checked cumulative declared decompressed bytes.
    pub total_decompressed_bytes: u64,
}

/// Closed hostile-archive failure class safe for persistence and telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveFailureClass {
    /// ZIP headers or directory structures are malformed or truncated.
    Malformed,
    /// An entry path is unsafe or ambiguous.
    UnsafePath,
    /// Two entries resolve to the same accepted identity.
    DuplicateEntry,
    /// An entry is not a regular file.
    UnsupportedEntryType,
    /// An entry uses unsupported compression or encryption.
    UnsupportedEncoding,
    /// One exact configured resource ceiling was exceeded.
    ResourceLimit,
}

impl ArchiveFailureClass {
    /// Stable database and metric spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Malformed => "archive_malformed",
            Self::UnsafePath => "archive_unsafe_path",
            Self::DuplicateEntry => "archive_duplicate_entry",
            Self::UnsupportedEntryType => "archive_unsupported_entry_type",
            Self::UnsupportedEncoding => "archive_unsupported_encoding",
            Self::ResourceLimit => "archive_resource_limit",
        }
    }
}

/// Typed hostile-archive refusal without attacker-controlled text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Data Export archive refused: {class:?}")]
pub struct ArchiveError {
    /// Closed failure class.
    pub class: ArchiveFailureClass,
    /// Closed violated rule, never an entry name or payload.
    pub rule: &'static str,
}

/// Inspects ZIP metadata before any parser reads entry bytes.
///
/// # Errors
///
/// Returns [`ArchiveError`] for malformed, ambiguous, encoded, typed, or
/// resource-unsafe input.
pub fn inspect_archive(
    bytes: &[u8],
    limits: ArchiveLimits,
) -> Result<ArchiveInventory, ArchiveError> {
    inspect_reader(std::io::Cursor::new(bytes), limits)
}

/// Reads one already-inspected archive entry with actual emitted-byte counters.
///
/// # Errors
///
/// Returns [`ArchiveError`] if archive metadata, the requested identity, the
/// decompressor, CRC evidence, or actual byte ceilings are invalid.
pub fn read_archive_entry(
    bytes: &[u8],
    entry_path: &str,
    limits: ArchiveLimits,
) -> Result<Vec<u8>, ArchiveError> {
    let inventory = inspect_archive(bytes, limits)?;
    let mut archive =
        ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| malformed("zip_structure"))?;
    read_inspected_entry(&mut archive, &inventory, entry_path, limits)
}

pub(super) fn read_file_entry(
    path: &Path,
    entry_path: &str,
    limits: ArchiveLimits,
) -> Result<Vec<u8>, ArchiveError> {
    let inventory = inspect_file(path, limits)?;
    let file = File::open(path).map_err(|_| malformed("archive_open"))?;
    let mut archive = ZipArchive::new(file).map_err(|_| malformed("zip_structure"))?;
    read_inspected_entry(&mut archive, &inventory, entry_path, limits)
}

fn read_inspected_entry<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    inventory: &ArchiveInventory,
    entry_path: &str,
    limits: ArchiveLimits,
) -> Result<Vec<u8>, ArchiveError> {
    let inspected = inventory
        .entries
        .iter()
        .find(|entry| entry.path == entry_path)
        .ok_or_else(|| malformed("entry_missing"))?;
    let mut entry = archive
        .by_name(entry_path)
        .map_err(|_| malformed("entry_open"))?;
    let mut output = Vec::new();
    let mut actual = 0_u64;
    let mut buffer = [0_u8; 16_384];
    loop {
        let read = entry
            .read(&mut buffer)
            .map_err(|_| malformed("entry_read"))?;
        if read == 0 {
            break;
        }
        actual = actual
            .checked_add(u64::try_from(read).map_err(|_| resource("actual_entry_bytes"))?)
            .ok_or_else(|| resource("actual_entry_bytes"))?;
        if actual > limits.max_entry_decompressed_bytes {
            return Err(resource("actual_entry_decompressed_bytes"));
        }
        if actual > limits.max_total_decompressed_bytes {
            return Err(resource("actual_total_decompressed_bytes"));
        }
        if exceeds_ratio(
            actual,
            inspected.compressed_size,
            limits.max_compression_ratio,
        ) {
            return Err(resource("actual_compression_ratio"));
        }
        output.extend_from_slice(
            buffer
                .get(..read)
                .ok_or_else(|| malformed("entry_buffer"))?,
        );
    }
    if actual != inspected.decompressed_size {
        return Err(malformed("declared_actual_size_mismatch"));
    }
    Ok(output)
}

pub(super) fn inspect_file(
    path: &Path,
    limits: ArchiveLimits,
) -> Result<ArchiveInventory, ArchiveError> {
    let file = File::open(path).map_err(|_| malformed("archive_open"))?;
    inspect_reader(file, limits)
}

fn inspect_reader<R: Read + Seek>(
    mut reader: R,
    limits: ArchiveLimits,
) -> Result<ArchiveInventory, ArchiveError> {
    let declared_entries = preflight_headers(&mut reader, limits)?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|_| malformed("archive_seek"))?;
    let mut archive = ZipArchive::new(reader).map_err(|_| malformed("zip_structure"))?;
    if archive.len() != declared_entries {
        return Err(ArchiveError {
            class: ArchiveFailureClass::DuplicateEntry,
            rule: "central_directory_duplicate",
        });
    }
    if archive.len() > limits.max_entries {
        return Err(resource("entry_count"));
    }
    let mut paths = BTreeSet::new();
    let mut entries = Vec::with_capacity(archive.len());
    let mut total_compressed_bytes = 0_u64;
    let mut total_decompressed_bytes = 0_u64;
    for index in 0..archive.len() {
        let file = archive
            .by_index(index)
            .map_err(|_| malformed("entry_header"))?;
        if file.encrypted() {
            return Err(ArchiveError {
                class: ArchiveFailureClass::UnsupportedEncoding,
                rule: "encrypted_entry",
            });
        }
        if !matches!(
            file.compression(),
            CompressionMethod::Stored | CompressionMethod::Deflated
        ) {
            return Err(ArchiveError {
                class: ArchiveFailureClass::UnsupportedEncoding,
                rule: "compression_method",
            });
        }
        if !file.is_file() || special_unix_type(file.unix_mode()) {
            return Err(ArchiveError {
                class: ArchiveFailureClass::UnsupportedEntryType,
                rule: "regular_files_only",
            });
        }
        let path = validate_path(file.name_raw(), limits)?;
        if !paths.insert(path.clone()) {
            return Err(ArchiveError {
                class: ArchiveFailureClass::DuplicateEntry,
                rule: "duplicate_path",
            });
        }
        if file.size() > limits.max_entry_decompressed_bytes {
            return Err(resource("entry_decompressed_bytes"));
        }
        if exceeds_ratio(
            file.size(),
            file.compressed_size(),
            limits.max_compression_ratio,
        ) {
            return Err(resource("compression_ratio"));
        }
        total_compressed_bytes = checked_total(
            total_compressed_bytes,
            file.compressed_size(),
            "total_compressed_bytes",
        )?;
        total_decompressed_bytes = checked_total(
            total_decompressed_bytes,
            file.size(),
            "total_decompressed_bytes",
        )?;
        if total_compressed_bytes > limits.max_total_compressed_bytes {
            return Err(resource("total_compressed_bytes"));
        }
        if total_decompressed_bytes > limits.max_total_decompressed_bytes {
            return Err(resource("total_decompressed_bytes"));
        }
        entries.push(InspectedEntry {
            path,
            compressed_size: file.compressed_size(),
            decompressed_size: file.size(),
        });
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(ArchiveInventory {
        entries,
        total_compressed_bytes,
        total_decompressed_bytes,
    })
}

#[derive(Debug, Clone, Copy)]
struct EndOfCentralDirectory {
    entry_count: usize,
    directory_offset: u64,
    directory_size: u64,
}

#[expect(
    clippy::too_many_lines,
    reason = "central/local header consistency is one ordered security preflight"
)]
fn preflight_headers<R: Read + Seek>(
    reader: &mut R,
    limits: ArchiveLimits,
) -> Result<usize, ArchiveError> {
    const CENTRAL_SIGNATURE: &[u8] = &[0x50, 0x4b, 0x01, 0x02];
    const LOCAL_SIGNATURE: &[u8] = &[0x50, 0x4b, 0x03, 0x04];
    let directory = end_of_central_directory(reader, limits.max_entries)?;
    let directory_end = directory
        .directory_offset
        .checked_add(directory.directory_size)
        .ok_or_else(|| malformed("central_directory_overflow"))?;
    reader
        .seek(SeekFrom::Start(directory.directory_offset))
        .map_err(|_| malformed("central_directory_seek"))?;
    let mut names = BTreeSet::new();
    for _ in 0..directory.entry_count {
        let mut central = [0_u8; 46];
        reader
            .read_exact(&mut central)
            .map_err(|_| malformed("central_header_read"))?;
        if central.get(..4) != Some(CENTRAL_SIGNATURE) {
            return Err(malformed("central_header_signature"));
        }
        let central_flags = field_u16(&central, 8, "central_flags")?;
        let central_method = field_u16(&central, 10, "central_method")?;
        let name_length = usize::from(field_u16(&central, 28, "central_name_length")?);
        let extra_length = usize::from(field_u16(&central, 30, "central_extra_length")?);
        let comment_length = usize::from(field_u16(&central, 32, "central_comment_length")?);
        if name_length == 0 || name_length > limits.max_entry_path_bytes {
            return Err(resource("entry_path_bytes"));
        }
        let local_offset = u64::from(field_u32(&central, 42, "local_header_offset")?);
        let mut central_name = vec![0_u8; name_length];
        reader
            .read_exact(&mut central_name)
            .map_err(|_| malformed("central_name_read"))?;
        if !names.insert(central_name.clone()) {
            return Err(ArchiveError {
                class: ArchiveFailureClass::DuplicateEntry,
                rule: "central_directory_duplicate",
            });
        }
        let skip = i64::try_from(
            extra_length
                .checked_add(comment_length)
                .ok_or_else(|| malformed("central_variable_overflow"))?,
        )
        .map_err(|_| malformed("central_variable_overflow"))?;
        reader
            .seek(SeekFrom::Current(skip))
            .map_err(|_| malformed("central_variable_seek"))?;
        let next_central = reader
            .stream_position()
            .map_err(|_| malformed("central_position"))?;
        if next_central > directory_end {
            return Err(malformed("central_directory_bounds"));
        }

        reader
            .seek(SeekFrom::Start(local_offset))
            .map_err(|_| malformed("local_header_seek"))?;
        let mut local = [0_u8; 30];
        reader
            .read_exact(&mut local)
            .map_err(|_| malformed("local_header_read"))?;
        if local.get(..4) != Some(LOCAL_SIGNATURE) {
            return Err(malformed("local_header_signature"));
        }
        let local_flags = field_u16(&local, 6, "local_flags")?;
        let local_method = field_u16(&local, 8, "local_method")?;
        if central_flags & 1 != 0 || local_flags & 1 != 0 {
            return Err(ArchiveError {
                class: ArchiveFailureClass::UnsupportedEncoding,
                rule: "encrypted_entry",
            });
        }
        if central_flags != local_flags {
            return Err(malformed("header_flags_mismatch"));
        }
        if central_method != local_method {
            return Err(malformed("header_method_mismatch"));
        }
        if !matches!(central_method, 0 | 8) {
            return Err(ArchiveError {
                class: ArchiveFailureClass::UnsupportedEncoding,
                rule: "compression_method",
            });
        }
        let local_name_length = usize::from(field_u16(&local, 26, "local_name_length")?);
        let local_extra_length = usize::from(field_u16(&local, 28, "local_extra_length")?);
        if local_name_length != central_name.len() {
            return Err(malformed("header_name_length_mismatch"));
        }
        let mut local_name = vec![0_u8; local_name_length];
        reader
            .read_exact(&mut local_name)
            .map_err(|_| malformed("local_name_read"))?;
        if local_name != central_name {
            return Err(malformed("header_name_mismatch"));
        }
        let data_start = reader
            .stream_position()
            .map_err(|_| malformed("local_position"))?
            .checked_add(
                u64::try_from(local_extra_length).map_err(|_| malformed("local_extra_overflow"))?,
            )
            .ok_or_else(|| malformed("local_extra_overflow"))?;
        if data_start > directory.directory_offset {
            return Err(malformed("local_data_bounds"));
        }
        reader
            .seek(SeekFrom::Start(next_central))
            .map_err(|_| malformed("central_resume"))?;
    }
    let observed_end = reader
        .stream_position()
        .map_err(|_| malformed("central_end"))?;
    if observed_end != directory_end {
        return Err(malformed("central_directory_size"));
    }
    Ok(directory.entry_count)
}

fn end_of_central_directory<R: Read + Seek>(
    reader: &mut R,
    maximum: usize,
) -> Result<EndOfCentralDirectory, ArchiveError> {
    const EOCD_MINIMUM: u64 = 22;
    const EOCD_SEARCH: u64 = EOCD_MINIMUM + u16::MAX as u64;
    const SIGNATURE: &[u8] = &[0x50, 0x4b, 0x05, 0x06];
    let length = reader
        .seek(SeekFrom::End(0))
        .map_err(|_| malformed("archive_length"))?;
    if length < EOCD_MINIMUM {
        return Err(malformed("eocd_missing"));
    }
    let tail_length = length.min(EOCD_SEARCH);
    reader
        .seek(SeekFrom::End(
            -i64::try_from(tail_length).map_err(|_| malformed("eocd_offset"))?,
        ))
        .map_err(|_| malformed("eocd_seek"))?;
    let mut tail = vec![0_u8; usize::try_from(tail_length).map_err(|_| malformed("eocd_size"))?];
    reader
        .read_exact(&mut tail)
        .map_err(|_| malformed("eocd_read"))?;
    let position = tail
        .windows(SIGNATURE.len())
        .enumerate()
        .rev()
        .find_map(|(position, window)| {
            if window != SIGNATURE {
                return None;
            }
            let comment_length = le_u16(tail.get(position + 20..position + 22)?)?;
            (position + 22 + usize::from(comment_length) == tail.len()).then_some(position)
        })
        .ok_or_else(|| malformed("eocd_missing"))?;
    let disk = le_u16(
        tail.get(position + 4..position + 6)
            .ok_or_else(|| malformed("eocd_disk"))?,
    )
    .ok_or_else(|| malformed("eocd_disk"))?;
    let directory_disk = le_u16(
        tail.get(position + 6..position + 8)
            .ok_or_else(|| malformed("eocd_disk"))?,
    )
    .ok_or_else(|| malformed("eocd_disk"))?;
    let disk_entries = le_u16(
        tail.get(position + 8..position + 10)
            .ok_or_else(|| malformed("eocd_count"))?,
    )
    .ok_or_else(|| malformed("eocd_count"))?;
    let total_entries = le_u16(
        tail.get(position + 10..position + 12)
            .ok_or_else(|| malformed("eocd_count"))?,
    )
    .ok_or_else(|| malformed("eocd_count"))?;
    if disk != 0 || directory_disk != 0 || disk_entries != total_entries {
        return Err(malformed("multi_disk"));
    }
    if total_entries == u16::MAX {
        return Err(resource("zip64_entry_count"));
    }
    let count = usize::from(total_entries);
    if count > maximum {
        return Err(resource("entry_count"));
    }
    let directory_size = u64::from(
        le_u32(
            tail.get(position + 12..position + 16)
                .ok_or_else(|| malformed("eocd_directory_size"))?,
        )
        .ok_or_else(|| malformed("eocd_directory_size"))?,
    );
    let directory_offset = u64::from(
        le_u32(
            tail.get(position + 16..position + 20)
                .ok_or_else(|| malformed("eocd_directory_offset"))?,
        )
        .ok_or_else(|| malformed("eocd_directory_offset"))?,
    );
    if directory_size == u64::from(u32::MAX) || directory_offset == u64::from(u32::MAX) {
        return Err(resource("zip64_directory"));
    }
    let eocd_absolute = length
        .checked_sub(tail_length)
        .and_then(|start| start.checked_add(u64::try_from(position).ok()?))
        .ok_or_else(|| malformed("eocd_position"))?;
    if directory_offset
        .checked_add(directory_size)
        .is_none_or(|end| end != eocd_absolute)
    {
        return Err(malformed("central_directory_bounds"));
    }
    Ok(EndOfCentralDirectory {
        entry_count: count,
        directory_offset,
        directory_size,
    })
}

fn le_u16(bytes: &[u8]) -> Option<u16> {
    let bytes = <[u8; 2]>::try_from(bytes).ok()?;
    Some(u16::from_le_bytes(bytes))
}

fn le_u32(bytes: &[u8]) -> Option<u32> {
    let bytes = <[u8; 4]>::try_from(bytes).ok()?;
    Some(u32::from_le_bytes(bytes))
}

fn field_u16(bytes: &[u8], offset: usize, rule: &'static str) -> Result<u16, ArchiveError> {
    le_u16(
        bytes
            .get(offset..offset + 2)
            .ok_or_else(|| malformed(rule))?,
    )
    .ok_or_else(|| malformed(rule))
}

fn field_u32(bytes: &[u8], offset: usize, rule: &'static str) -> Result<u32, ArchiveError> {
    le_u32(
        bytes
            .get(offset..offset + 4)
            .ok_or_else(|| malformed(rule))?,
    )
    .ok_or_else(|| malformed(rule))
}

fn validate_path(raw: &[u8], limits: ArchiveLimits) -> Result<String, ArchiveError> {
    if raw.is_empty()
        || raw.len() > limits.max_entry_path_bytes
        || raw.contains(&0)
        || raw.contains(&b'\\')
    {
        return Err(unsafe_path("path_bytes"));
    }
    let path = std::str::from_utf8(raw).map_err(|_| unsafe_path("path_utf8"))?;
    let mut components = path.split('/');
    let first = components.next().ok_or_else(|| unsafe_path("path_empty"))?;
    if first.is_empty()
        || path.starts_with('/')
        || windows_drive_prefix(raw)
        || path
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
        || Path::new(path)
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(unsafe_path("path_components"));
    }
    if path.split('/').count() > limits.max_path_depth {
        return Err(resource("path_depth"));
    }
    Ok(path.to_owned())
}

fn windows_drive_prefix(raw: &[u8]) -> bool {
    matches!(
        (raw.first(), raw.get(1), raw.get(2)),
        (Some(first), Some(b':'), Some(b'/')) if first.is_ascii_alphabetic()
    )
}

fn special_unix_type(mode: Option<u32>) -> bool {
    mode.is_some_and(|mode| {
        let file_type = mode & 0o170_000;
        file_type != 0 && file_type != 0o100_000
    })
}

fn exceeds_ratio(decompressed: u64, compressed: u64, maximum: u64) -> bool {
    decompressed > 0 && (compressed == 0 || decompressed > compressed.saturating_mul(maximum))
}

fn checked_total(total: u64, value: u64, rule: &'static str) -> Result<u64, ArchiveError> {
    total.checked_add(value).ok_or_else(|| resource(rule))
}

fn malformed(rule: &'static str) -> ArchiveError {
    ArchiveError {
        class: ArchiveFailureClass::Malformed,
        rule,
    }
}

fn unsafe_path(rule: &'static str) -> ArchiveError {
    ArchiveError {
        class: ArchiveFailureClass::UnsafePath,
        rule,
    }
}

fn resource(rule: &'static str) -> ArchiveError {
    ArchiveError {
        class: ArchiveFailureClass::ResourceLimit,
        rule,
    }
}
