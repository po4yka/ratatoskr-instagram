## Purpose

Defines truthful reference-only media retention and the finite policy required before Instagram-owned storage may archive or erase provider media bytes.

## Requirements

### Requirement: Provider media is reference-only unless every archival guard admits bytes

The service SHALL treat provider media URLs and export paths as metadata references by default. It SHALL archive bytes only after an explicit owner-authorized action and a policy decision establishes supported acquisition provenance, sufficient rights, an HTTPS source, allowed media type, known finite object size, finite retention deadline, and available owner/object storage budgets. An unknown or exhausted guard MUST yield a typed metadata-only result before network or storage I/O.

#### Scenario: Unknown eligibility remains metadata-only

- **WHEN** a media reference lacks rights, media-type, size, lifetime, or owner-budget evidence
- **THEN** the policy reports the corresponding metadata-only reason and neither fetches nor stores media bytes

### Requirement: Archived bytes remain bounded and independently verifiable

An admitted fetch SHALL enforce a finite deadline and byte ceiling while streaming, reject redirects or final URLs outside the approved HTTPS policy, validate declared and observed media type and length, compute the content digest, and attach a `BlobRef` only after the complete stored object verifies against that evidence. A failed or partial fetch SHALL leave no live media-byte reference and SHALL NOT be reported as an archived media backup.

#### Scenario: Mismatched media cannot become archived evidence

- **WHEN** a response exceeds its lease or disagrees with the admitted URL, media type, length, or content digest
- **THEN** no media-byte reference is committed, partial bytes are unavailable, and the record remains truthfully metadata-only

### Requirement: Expiry erases only unreferenced Instagram-owned bytes

When a retention deadline expires or privacy deletion removes a reference, the service SHALL first detach that live reference and durably schedule exact BlobStore work. Physical deletion SHALL occur only after a fresh inventory proves that no live Instagram record references the same object; failures remain retryable and visible without retaining source content in diagnostics.

#### Scenario: Shared content survives one reference expiry

- **WHEN** two live records reference the same content-addressed object and one reference expires
- **THEN** the expired reference is removed, the object remains present for the other record, and no completed deletion is reported

#### Scenario: Final-reference deletion converges after a transient failure

- **WHEN** the final reference is removed and the first physical deletion attempt fails
- **THEN** durable work remains pending, a later retry verifies absence, and only then is the deletion marked complete
