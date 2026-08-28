# Instagram connector domain model

## Terms

- **Account connection:** supported Instagram identity, capabilities, scopes, and credential status.
- **Acquisition method:** `OfficialApi`, `ShareExtension`, `BrowserExtension`, `DataExport`, or `LegacyImport`.
- **Saved authority:** `ExplicitUserCapture`, `ExportObservation`, or documented authoritative provider state.
- **Capture:** explicit local request containing canonical URL, time, source, note, and collections.
- **Resolved media:** public metadata/caption/media references returned by a supported mechanism.
- **Unavailable state:** private, deleted, expired, unsupported, blocked, or unresolved.
- **Export snapshot:** immutable user-provided archive and parser report.
- **Media retention decision:** reference-only or explicitly admitted bounded byte storage, never an
  implied complete backup.
- **Privacy deletion:** owner-authorized capture or account-connection erasure with content-free
  audit, local resurrection guard, BlobStore convergence, and downstream deletion request.
- **Re-resolution:** budgeted refresh of a recent due public capture through the existing resolver.
- **Parser reprocessing:** deterministic dry-run/apply over one retained immutable export receipt;
  not a database migration or legacy import.

## Invariants

1. Local capture is never labeled native Saved state.
2. Official account data and captured third-party content are separate lanes.
3. Provider privacy is not bypassed.
4. User-uploaded screenshot/file is a separate artifact with separate authority.
5. Raw exports precede parsing; unknown sections are not discarded.
6. Missing export data does not prove deletion.
7. Shared evidence survives deletion of one holding or account connection.
8. Local privacy commit and downstream Knowledge deletion completion are separate evidence states.
9. Re-resolution never starts after a privacy removal or exhausted finite budget.
10. Parser omission retains prior raw and normalized evidence with `ExportObservation` authority.
