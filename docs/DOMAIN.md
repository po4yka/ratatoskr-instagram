# Instagram connector domain model

## Terms

- **Account connection:** supported Instagram identity, capabilities, scopes, and credential status.
- **Acquisition method:** `OfficialApi`, `ShareExtension`, `BrowserExtension`, `DataExport`, or `LegacyImport`.
- **Saved authority:** `ExplicitUserCapture`, `ExportObservation`, or documented authoritative provider state.
- **Capture:** explicit local request containing canonical URL, time, source, note, and collections.
- **Resolved media:** public metadata/caption/media references returned by a supported mechanism.
- **Unavailable state:** private, deleted, expired, unsupported, blocked, or unresolved.
- **Export snapshot:** immutable user-provided archive and parser report.

## Invariants

1. Local capture is never labeled native Saved state.
2. Official account data and captured third-party content are separate lanes.
3. Provider privacy is not bypassed.
4. User-uploaded screenshot/file is a separate artifact with separate authority.
5. Raw exports precede parsing; unknown sections are not discarded.
6. Missing export data does not prove deletion.
