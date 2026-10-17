## Why

The standalone fuzz workspace has a locked dependency graph that production checks do not inspect. Its full dependency audit fails for Rustls advisories and license metadata. The owner approved SEC-030 D5 on 2026-10-17.

## What Changes

- Align fuzz Rustls with the reviewed production version 0.23.46.
- Declare the existing BSD-3-Clause license for the local fuzz package.
- Permit NCSA only for libfuzzer-sys 0.4.13 through a package-specific exception.
- Run locked fuzz metadata and the full dependency policy in CI and document the commands.

## Capabilities

### New Capabilities

- `fuzz-dependency-policy`: Apply the repository dependency policy to the standalone fuzz graph.

### Modified Capabilities

None.

## Impact

Only fuzz metadata, dependency policy, CI and developer instructions change. Production dependencies, advisory rules and the global license allowlist stay unchanged. Fleet coordination belongs to ratatoskr-workspace SEC-030 and OBS-029.
