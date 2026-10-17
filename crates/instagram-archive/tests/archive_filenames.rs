//! Filename decoding at the public archive inspection boundary.
#![expect(clippy::expect_used, reason = "synthetic ZIP setup must fail loudly")]

use ratatoskr_instagram_archive::data_export::{
    ArchiveFailureClass, ArchiveLimits, inspect_archive,
};
use ratatoskr_instagram_archive::test_support::data_export_zip;

#[test]
fn inspector_refuses_malformed_utf8_filename() {
    let archive = data_export_zip(&[("entry.json", b"{}")], zip::CompressionMethod::Stored)
        .expect("synthetic archive writes");
    let archive = malformed_utf8_name(archive);
    let error = inspect_archive(&archive, ArchiveLimits::default())
        .expect_err("malformed UTF-8 name must be refused");
    assert_eq!(error.class, ArchiveFailureClass::UnsafePath);
}

fn malformed_utf8_name(mut bytes: Vec<u8>) -> Vec<u8> {
    for (signature, flags_offset, name_offset) in [(b"PK\x03\x04", 7, 30), (b"PK\x01\x02", 9, 46)] {
        let header = bytes
            .windows(4)
            .position(|window| window == signature)
            .expect("synthetic ZIP header exists");
        *bytes
            .get_mut(header + flags_offset)
            .expect("flags byte exists") |= 0x08;
        *bytes
            .get_mut(header + name_offset)
            .expect("filename byte exists") = 0xff;
    }
    bytes
}
