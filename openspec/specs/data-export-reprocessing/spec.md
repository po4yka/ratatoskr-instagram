## Purpose

Defines deterministic dry-run and restartable apply of an explicit parser version over retained immutable Instagram Data Export evidence.

## Requirements

### Requirement: Reprocessing verifies immutable input and selects an exact supported parser

Reprocessing SHALL verify the authenticated owner's retained archive receipt, digest, byte length, detected layout, and safe archive limits before planning. The caller SHALL select an exact parser identifier supported for that layout; unknown parsers, fallback selection, tampered receipts, or unverifiable bytes MUST be refused before any projection, checkpoint, BlobStore, or outbox mutation.

#### Scenario: Tampered receipt fails closed

- **WHEN** retained bytes no longer match the receipt digest or size
- **THEN** dry-run and apply return an integrity refusal and all durable projection, blob, outbox, audit, and checkpoint state remains unchanged

### Requirement: Dry-run and apply share one deterministic plan

Dry-run and apply SHALL derive the same ordered classifications, counts, warnings, conflicts, completeness evidence, prospective record/source digests, and canonical plan fingerprint from the same immutable archive, parser, and owner-state snapshot. Run identities and timestamps SHALL be excluded from the fidelity comparison. Dry-run MUST remain read-only and MUST NOT expose raw bodies, notes, credentials, full private paths, or archive bytes.

#### Scenario: Dry-run faithfully predicts apply

- **WHEN** dry-run is followed by apply with unchanged archive, parser, and owner state
- **THEN** the reports match in every planned/applied classification, count, warning, conflict, completeness field, digest, and fingerprint except operation identity and timestamps

#### Scenario: Dry-run has zero durable effects

- **WHEN** dry-run evaluates normalized, unknown, warning, and conflicting records
- **THEN** database rows, BlobStore inventory, outbox rows, checkpoints, and retained evidence are unchanged

### Requirement: Apply checkpoints bounded work and resumes idempotently

Apply SHALL use a stable owner-scoped operation identity and commit deterministic bounded chunks. Each chunk SHALL atomically record item outcome, projection changes, and outbox effects. Resume SHALL verify the archive digest, parser identity, plan fingerprint, and current-state preconditions before continuing from the last committed checkpoint. Replaying a completed operation SHALL return the stored report without duplicate rows or facts.

#### Scenario: Interrupted apply resumes to the fresh result

- **WHEN** apply stops after one committed chunk and resumes with unchanged preconditions
- **THEN** its final report and durable row/fact counts equal an uninterrupted fresh apply and no completed item runs twice

### Requirement: Parser omission is non-destructive

Records or categories absent from a selected parser's output SHALL be reported as omissions or coverage changes and SHALL NOT delete existing captures, normalized media, prior revisions, raw evidence, or source facts. Reprocessed Data Export records SHALL retain `data_export` acquisition and `export_observation` saved authority.

#### Scenario: New parser omits an old category

- **WHEN** a selected parser no longer emits a category preserved by an earlier import
- **THEN** the report lists the omission, the earlier state remains retrievable, and no removal or upstream-unavailable fact is appended
