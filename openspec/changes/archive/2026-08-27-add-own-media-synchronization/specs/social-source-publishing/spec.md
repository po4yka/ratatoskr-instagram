## ADDED Requirements

### Requirement: Completed own-media generations publish normalized official facts

When an atomically completed own-media generation first preserves a provider media identity, the service SHALL append one `social.source.captured.v1` fact; when a later completed generation changes that normalized identity, it SHALL append one `social.source.updated.v1` fact. Each fact SHALL be written in the same transaction as authority completion, carry the whole current source snapshot, use `official_api` acquisition, preserve the owner and provider publication time, and make no native-Saved assertion.

#### Scenario: First authoritative own-media observation publishes once

- **WHEN** a completed generation first makes one connected-account media identity visible
- **THEN** exactly one captured fact commits with that authority generation and replaying finalization adds no duplicate

#### Scenario: Changed own-media metadata publishes an update

- **WHEN** a later completed generation changes the caption or supported metadata for an already published provider media identity
- **THEN** exactly one updated fact carries the complete new snapshot under the same stable `social_source_id`

### Requirement: Own-media facts expose only verifiable blob references

An own-media snapshot SHALL carry the completing sync checkpoint and a `raw_blob` naming the preserved provider evidence. It SHALL include a media attachment only when its bytes are stored behind a valid workspace `BlobRef`; a provider URL alone SHALL NOT become a blob reference. When no media bytes were archived, the snapshot SHALL declare partial completeness with a warning that names that gap.

#### Scenario: Provider URL remains metadata rather than a blob

- **WHEN** a completed own-media generation preserved metadata and raw response bytes but did not archive the referenced image or video bytes
- **THEN** the published snapshot carries the raw-response BlobRef and sync checkpoint, has no fabricated media attachment, and reports partial completeness with a missing-media warning

### Requirement: Own-media source identity is stable and owner-scoped

Each own-media fact SHALL derive one stable Ratatoskr `social_source_id` from the Ratatoskr owner plus the provider media identity, independent of mutable permalink, caption, username, checkpoint, or authority generation. The same provider identity under another Ratatoskr owner SHALL produce a different source identity.

#### Scenario: Identity survives authority generations

- **WHEN** the same provider media identity appears in two completed generations for one owner
- **THEN** captured and updated facts carry the same `social_source_id` despite changed metadata or checkpoint

#### Scenario: Shared provider identity remains tenant-scoped

- **WHEN** two Ratatoskr owners independently connect accounts that yield the same opaque provider media identifier
- **THEN** their published facts carry different `social_source_id` values
