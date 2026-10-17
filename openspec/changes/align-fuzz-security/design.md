## Context

The locked metadata check first failed with exit 101 because the fuzz lock had stale contracts and ZIP entries. Those approved entries were aligned separately. The full fuzz cargo-deny check then failed with exit 5: Rustls 0.23.45 has four advisory findings, the local package has no license field, and libfuzzer-sys 0.4.13 requires NCSA. The repository LICENSE is BSD-3-Clause.

## Decisions

1. Copy only the Rustls 0.23.46 package record from the reviewed production lock. Preserve every other package and edge beyond the already approved contracts and ZIP changes.
2. Set the local fuzz package license to BSD-3-Clause. This declares the existing license and does not relicense source.
3. Use a license exception for the exact package libfuzzer-sys 0.4.13. Its expression is `(MIT OR Apache-2.0) AND NCSA`. The published manifest and README identify the bundled libfuzzer license. The SPDX NCSA text permits reuse with notices and disclaimers and prohibits implied endorsement. Keep the global allowlist unchanged and review the exception when this tool version changes.
4. Add locked fuzz metadata to the Rust gate and the full locked fuzz cargo-deny check to the separate deny job. Use the package exception syntax supported by the actual CI cargo-deny 0.20.2. Its --config option is a root option. Align the isolated local audit tool with that observed CI version. Check both root and fuzz graphs locally and in CI.

## Validation and rollback

The existing failed full audit is the pre-fix evidence. Configuration and generated lock data have no behavioral RED. Preserve that failure, then observe full root and fuzz audit success, locked metadata, all repository gates, independent review and exact-commit CI. Roll forward if a gate fails; do not publish the vulnerable graph as a normal release. This work does not claim a fuzz runtime or real provider acceptance test.
