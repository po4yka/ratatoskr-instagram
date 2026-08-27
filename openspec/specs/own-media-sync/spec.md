# own-media-sync Specification

## Purpose

Synchronizes the connected account's supported own-media catalogue through bounded official-API scans while preserving durable progress, truthful capability limits, and atomic visible authority.

## Requirements

### Requirement: Own-media work is capability-gated before provider contact

The service SHALL evaluate the connected account's current connection state and complete `own_media_read` capability generation before decrypting a credential or requesting media. An unavailable capability SHALL produce a terminal no-op run carrying the observed closed reason and capability generation, SHALL issue no provider media request, and SHALL leave the prior checkpoint and visible own-media authority unchanged.

#### Scenario: Unsupported account type records a truthful no-op

- **WHEN** a scheduled own-media job targets a personal or unknown account whose current `own_media_read` capability is unavailable
- **THEN** the job records a capability no-op with the account-type reason, makes zero provider media requests, and changes neither the watermark nor the visible own-media set

#### Scenario: Permission downgrade prevents stale authorization

- **WHEN** a job targets an account whose current generation no longer grants the required read permission
- **THEN** the job records the observed permission reason, makes zero provider media requests, and does not exercise an older available generation

### Requirement: Scheduled scans persist resumable bounded progress

The service SHALL schedule due connected accounts at a finite cadence, admit at most one active own-media run per account, reserve durable provider budget before each page request, and persist the provider continuation cursor plus staged metadata after each accepted page. A later scheduled pass SHALL resume the same incomplete run from its durable cursor rather than starting a competing authority generation.

#### Scenario: A retry resumes the durable page checkpoint

- **WHEN** a multi-page own-media run commits one page and then ends retryably before the next page succeeds
- **THEN** its staged metadata and next-page cursor remain durable, the account has no second active run, and the next scheduled pass requests the recorded continuation

#### Scenario: Budget refusal preserves resumable progress

- **WHEN** the next page cannot reserve a provider-attempt unit within the run's finite budget
- **THEN** no request for that page occurs, the last committed cursor remains available for retry, and no authority or watermark advances

### Requirement: The observation watermark advances only after a proven incremental traversal

Own-media pages SHALL be interpreted in provider newest-first order. An initial run SHALL reach the documented end of the collection; a later incremental run SHALL reach its committed provider-media watermark. Only such a successfully completed traversal SHALL replace the watermark with the first provider-media identifier observed by that run. Failure, cancellation, invalid response, capability drift, or bounded exhaustion before the prior watermark SHALL retain the old watermark.

#### Scenario: Completed incremental traversal advances the watermark

- **WHEN** a scan observes newer media and then reaches the account's committed provider-media watermark without error
- **THEN** completion records the newest observed provider-media identifier as the new watermark

#### Scenario: Incomplete traversal retains the watermark

- **WHEN** a scan stops before reaching its prior watermark because a page fails, is refused, or violates the response contract
- **THEN** the prior watermark remains current and no incomplete run is reported as authoritative

### Requirement: Completed runs replace visible own-media authority atomically

Each run SHALL stage one candidate account-owned projection separately from the currently visible generation. A later incremental candidate SHALL retain previously authoritative media that the bounded prefix did not revisit and replace or add only provider identities observed in the run; absence from that prefix SHALL NOT infer deletion. The final transaction SHALL apply the staged metadata and change the account's authority pointer together, so observers see either the complete prior generation or the complete new generation and never a partially fetched mix.

#### Scenario: Completion swaps the whole generation

- **WHEN** a completed run retains one prior item, refreshes one item, and adds one new item
- **THEN** the visible projection changes in one commit from the prior generation to the candidate containing all three results

#### Scenario: Failed staging leaves prior authority visible

- **WHEN** a run has staged only part of its candidate set and then fails before completion
- **THEN** every current-own-media query still returns the prior authoritative generation and none of the partial candidate is visible through that query

### Requirement: Provider evidence and media completeness remain truthful

The service SHALL strictly validate bounded own-media page responses, preserve each accepted raw page as service-owned content-addressed evidence, and link normalized metadata to that evidence. Provider media URLs SHALL remain protected, expiring observations and SHALL NOT be represented as `BlobRef` values unless the named bytes were actually stored with verifiable digest, media type, and length under the workspace `blob-references` contract. Metadata-only synchronization SHALL publish partial completeness with a warning that media bytes were not archived.

#### Scenario: Metadata-only media does not fabricate a blob

- **WHEN** an accepted provider item supplies metadata and an expiring media URL but this lane stores no media bytes
- **THEN** its normalized source links the raw response BlobRef, contains no media-byte BlobRef, and reports partial completeness with a missing-media warning

#### Scenario: Unknown or oversized provider data is refused

- **WHEN** a provider page exceeds its byte bound or contains a media shape outside the accepted schema
- **THEN** the page is rejected before staging, the raw input is not normalized, and no watermark or authority advances

### Requirement: Own-media scope excludes ephemeral and foreign content

The own-media request SHALL address only the connected provider account identity, SHALL request only the reviewed non-ephemeral media fields, and SHALL reject any returned item whose ownership evidence names another account. Stories and other ephemeral content SHALL remain absent unless a later capability-matrix change explicitly adds them.

#### Scenario: Foreign media cannot enter account authority

- **WHEN** a provider response item identifies an owner other than the connected provider account
- **THEN** the run rejects the response and neither stages nor publishes that item

#### Scenario: Scheduled requests omit ephemeral fields

- **WHEN** the official own-media adapter builds a page request
- **THEN** the requested field set contains no story or ephemeral-content field and the request targets only the connected account's own-media edge
