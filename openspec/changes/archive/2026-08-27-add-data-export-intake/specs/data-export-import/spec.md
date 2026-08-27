## Purpose

Defines owner-authenticated, immutable, safely bounded ingestion of Instagram Data Export archives and the evidence required to parse, reconcile, and report their coverage truthfully.

## ADDED Requirements

### Requirement: Export receipt authenticates the owner before reading archive bytes
The product plane SHALL expose `POST /v1/data-exports` for `application/zip` bodies and SHALL resolve a bearer credential to exactly one `user_ref` before reading any request-body byte. Missing, malformed, and unknown credentials SHALL receive the same disclosure-free refusal. The request SHALL NOT accept a caller-supplied owner identifier, and an accepted archive and every derived record SHALL remain scoped to the authenticated owner.

#### Scenario: Unknown credential is refused before body consumption
- **WHEN** a client submits an archive stream with an unknown bearer credential
- **THEN** the service returns the unauthenticated response without polling the archive stream or creating a receipt

#### Scenario: Credential fixes the archive owner
- **WHEN** a configured bearer credential uploads a valid ZIP
- **THEN** the resulting receipt belongs to the one owner mapped from that credential and contains no owner value supplied by the body

### Requirement: Receipt preserves exact immutable bytes before inspection
The service SHALL stream every authenticated body through a finite compressed-byte budget while computing SHA-256, SHALL store the exact received bytes under an Instagram-owned content address, and SHALL record a `BlobRef` whose digest and byte length verify those bytes before archive inspection begins. Receipt SHALL be idempotent on `(user_ref, archive_digest)`: an owner retry returns the existing run and reference without replacing bytes, while a different owner receives a distinct owner-scoped run even when the archive bytes match.

#### Scenario: Chunking does not change receipt evidence
- **WHEN** identical ZIP bytes are uploaded for one owner using different stream chunk boundaries
- **THEN** both attempts report the same SHA-256, byte length, BlobRef, and import-run identity and only one immutable archive object is written

#### Scenario: Receipt budget stops an overgrown stream
- **WHEN** an authenticated upload emits more bytes than the configured archive-receipt limit
- **THEN** the service returns the typed payload-too-large response and creates neither an immutable archive object nor an import run

#### Scenario: Identical bytes do not cross the owner boundary
- **WHEN** two owners upload byte-identical archives
- **THEN** each owner receives a different import-run identity and cannot read the other owner's run or report

### Requirement: Every import follows one durable ordered state machine
An accepted run SHALL begin in `received` and advance only through `inspected`, `parsed`, and `reconciled` in that order. `reconciled` SHALL be terminal and SHALL contain the exact parser evidence and completeness report for the committed projections. A failure at any non-terminal stage SHALL transition the run to terminal `failed` with a bounded typed reason while retaining the immutable archive; no retry or concurrent worker SHALL skip, reverse, or duplicate a completed transition.

#### Scenario: Successful import records every stage in order
- **WHEN** a supported archive is processed successfully
- **THEN** its durable transition history is exactly `received`, `inspected`, `parsed`, `reconciled`, and its terminal report names the parser version used

#### Scenario: Parser failure retains raw evidence without projection
- **WHEN** an inspected archive cannot be parsed by a supported parser
- **THEN** the run becomes `failed`, the immutable archive remains verifiable, and no normalized record or SocialSource fact from that run exists

### Requirement: Archive inspection rejects hostile paths, duplicates, and resource exhaustion
Before parsing, the service SHALL reject absolute paths, parent traversal, platform-ambiguous backslash paths, duplicate normalized entry names, links or unsupported entry types, excessive path depth, excessive entry count, configured compressed or decompressed byte totals, and configured compression ratio. Declared metadata and bytes actually emitted SHALL both be bounded. Any optional extraction SHALL write only beneath a newly created private root, SHALL never overwrite an existing path, and SHALL leave no output outside that root.

#### Scenario: Zip-slip entries cannot escape extraction
- **WHEN** a ZIP contains a `../`, absolute, or backslash-ambiguous entry path
- **THEN** inspection fails with the path-safety reason, no path outside the private root is created, and no parser or reconciliation output exists

#### Scenario: Duplicate normalized entries are refused
- **WHEN** a ZIP contains two entries that resolve to the same normalized archive path
- **THEN** inspection fails with the duplicate-entry reason before either entry reaches the parser

#### Scenario: Archive bomb is refused by declared and actual limits
- **WHEN** a ZIP exceeds the configured entry, nesting, compressed-byte, decompressed-byte, or compression-ratio limit either in metadata or actual emitted bytes
- **THEN** the run fails with the exact violated limit and no normalized record or SocialSource fact is committed

### Requirement: One detected export shape selects one deterministic parser version
The inspector SHALL classify a supported Instagram export structure without executing or rendering any entry, and parsing SHALL record one explicit parser version. Equivalent supported fixture archives SHALL produce byte-equivalent ordered normalized records, raw-evidence references, warnings, and category classifications regardless of ZIP entry order or JSON object order. Every normalized record SHALL carry `data_export` acquisition and `export_observation` saved authority. Unsupported layouts and unknown sections or records SHALL remain raw evidence with explicit warnings and SHALL never be guessed as native Saved authority.

#### Scenario: Reordered fixture archives parse identically
- **WHEN** two supported synthetic/redacted fixture ZIPs contain equivalent export data with different entry and JSON-object ordering
- **THEN** both runs produce the same ordered normalized records, category results, warnings, raw-reference digests, and parser version

#### Scenario: Unknown material stays visible and non-authoritative
- **WHEN** a supported fixture contains a recognized section and one unknown section or record
- **THEN** the recognized records are normalized, the unknown material is retained by raw reference and warning, and no output claims native Saved membership or a newer parser version

### Requirement: Reconciliation is idempotent and export absence is non-destructive
The service SHALL reconcile parsed records for the authenticated owner using stable provider identity first and canonical permalink second, retaining ambiguous records separately with a conflict. Applying the same run again SHALL not duplicate a normalized record, raw record, relation, import fact, or SocialSource fact. A capture, normalized source, or relation absent from an export SHALL remain unchanged and SHALL produce no tombstone, unsave, deletion, unavailable, or removal observation.

#### Scenario: Replaying reconciliation preserves one projection
- **WHEN** the same parsed run is reconciled again after a worker retry
- **THEN** normalized-record, raw-record, relation, report, and source-fact counts remain unchanged

#### Scenario: Capture absent from export remains preserved
- **WHEN** an owner capture has a comparable provider identity that does not appear in the archive
- **THEN** the capture and any preserved source remain unchanged and no removal or upstream-unavailable fact is published

### Requirement: Completeness reports count and list exact comparable gaps
Every reconciled run SHALL persist and expose to its authenticated owner a deterministic report containing parsed and unknown categories, warnings, distinct export identities, matching existing capture identities, export-only identities, capture-only comparable identities, and captures that lack a stable comparable provider identity. The report SHALL include sorted owner-scoped lists for each gap class, its counts SHALL equal the cardinalities of those lists, and overlap/difference arithmetic SHALL be set-based. The report SHALL state that it describes one archive observation and is not proof of complete account history, native Saved membership, unsave, or deletion; no gap SHALL be filled or hidden automatically.

#### Scenario: Coverage math separates overlap and every gap class
- **WHEN** an owner has two comparable captures matching the fixture export, one comparable capture absent from it, and one non-comparable capture while the export has one additional identity
- **THEN** the report lists and counts two matches, one export-only identity, one capture-only comparable identity, and one non-comparable capture, and its authority disclaimer makes no deletion or native-Saved claim

#### Scenario: Report access is owner-scoped
- **WHEN** a credential belonging to another owner requests an existing run's state or report
- **THEN** the service returns the same not-found response used for an unknown run and discloses no count, identifier, URL, warning, or digest from that run

### Requirement: Import does not archive referenced media bytes
This change SHALL preserve the uploaded ZIP and bounded raw section evidence, but SHALL treat media paths and URLs found inside the export only as metadata references. It SHALL NOT fetch provider URLs, expand media payloads into normalized media-byte blobs, execute media helpers, or report a metadata-only record as a complete media backup.

#### Scenario: Referenced media stays a reference
- **WHEN** a supported export record names a media file or provider URL
- **THEN** the normalized record carries only the validated reference and reports media bytes as not archived by this import
