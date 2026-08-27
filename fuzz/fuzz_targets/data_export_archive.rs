#![no_main]

use libfuzzer_sys::fuzz_target;
use ratatoskr_instagram_archive::data_export::{ArchiveLimits, inspect_archive};

fuzz_target!(|bytes: &[u8]| {
    let _ = inspect_archive(
        bytes,
        ArchiveLimits {
            max_entries: 32,
            max_entry_path_bytes: 256,
            max_path_depth: 8,
            max_total_compressed_bytes: 1_048_576,
            max_total_decompressed_bytes: 4_194_304,
            max_entry_decompressed_bytes: 1_048_576,
            max_compression_ratio: 200,
        },
    );
});
