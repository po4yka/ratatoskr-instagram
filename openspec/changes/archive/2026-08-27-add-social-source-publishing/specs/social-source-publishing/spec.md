## Purpose

Publishes each preserved Instagram source as a normalized SocialSource fact so other Ratatoskr services can index and analyse it: `social.source.captured.v1` when a capture's source is first preserved through supported public resolution, `social.source.updated.v1` when its normalized record changes, both built only from stored provenance and delivered through the transactional outbox.

## ADDED Requirements

### Requirement: A successfully resolved capture publishes exactly one captured event

When a capture's first supported public resolution succeeds, the service SHALL append exactly one `social.source.captured.v1` fact to the transactional outbox, whose envelope carries `event_type = social.source.captured.v1` and whose payload holds the whole snapshot. Repeating or replaying the resolution SHALL NOT add another captured fact for that capture.

#### Scenario: first successful resolution emits one captured fact

- **WHEN** a capture is resolved successfully for the first time
- **THEN** exactly one unpublished outbox row exists for that capture with envelope event type `social.source.captured.v1`, and reading the payload back through the envelope yields a snapshot equal to the stored state

#### Scenario: replaying the same resolution stays one fact

- **WHEN** the same canonical URL is delivered again for the same user and the existing resolution is replayed
- **THEN** no additional captured fact is appended and the original fact is unchanged

### Requirement: The snapshot states stored provenance verbatim

The published snapshot SHALL take acquisition method and saved authority from the capture record alone. A share-style client source SHALL map to `share_extension` or `browser_extension` acquisition with saved authority `explicit_user_capture`; a capture resolved through the supported public surface SHALL keep that authority unchanged. No published field SHALL assert membership in a provider-native Saved list, and any value outside the published closed vocabularies SHALL fail construction rather than being coerced.

#### Scenario: share-style capture keeps the explicit ceiling

- **WHEN** a capture delivered through `ios_share_extension` is resolved and published
- **THEN** the snapshot's acquisition is `share_extension`, its saved authority is `explicit_user_capture`, and no field claims native Saved membership

#### Scenario: an impossible provenance stops publication

- **WHEN** snapshot construction is attempted from a record whose provenance token is not in the published vocabulary
- **THEN** construction fails with an unknown-variant error and no event is emitted

### Requirement: One capture keeps one stable Ratatoskr identity across every event

Each capture SHALL publish under one `social_source_id`, a bare canonical lowercase UUID, that is stable across its captured fact and every later updated fact for the same `(owner, source)` pair. Two different users who capture the same canonical URL SHALL publish under different `social_source_id` values, and the same user re-capturing the same URL SHALL reuse the existing identity.

#### Scenario: identity survives updates

- **WHEN** a capture publishes a captured fact and later an updated fact
- **THEN** both payloads carry byte-identical canonical `social_source_id` values

#### Scenario: the same URL under two owners stays two identities

- **WHEN** two different users capture the same canonical URL and both resolve successfully
- **THEN** the two captured facts carry different `social_source_id` values and identical `external_post_id` values

### Requirement: The snapshot reflects observed content and availability truthfully

On successful resolution the snapshot SHALL carry the permalink, the provider shortcode as `external_post_id`, the owner, the author and text as exposed by the approved resolver surface, media items as BlobRef references with no embedded bytes, a content digest over the normalized content, and `upstream_availability = available`. Publication time SHALL appear only when the provider supplied it and SHALL never be inferred from capture time. A metadata-only capture that archived no media SHALL declare `completeness = partial` with at least one warning naming the missing media.

#### Scenario: resolved content matches the stored revision

- **WHEN** a resolution produced a raw revision and normalized media
- **THEN** the published snapshot's text, media references, and raw blob reference match the stored revision, and no base64 or byte array appears anywhere in the payload

#### Scenario: metadata-only capture declares partial completeness

- **WHEN** a resolution succeeded but no media was archived under the media policy
- **THEN** the snapshot declares `completeness = partial` and carries at least one warning naming the missing media, satisfying the contract invariant that rejects partial without warnings

### Requirement: Changed normalized records publish updated facts carrying the whole record

When a later re-resolution appends a new revision that changes the normalized record, or a new availability observation changes the observed state, the service SHALL append one `social.source.updated.v1` fact whose payload alone is sufficient to index the source without consulting any earlier event.

#### Scenario: a richer re-resolution republishes the full record

- **WHEN** a re-resolution produces a new revision with changed normalized content
- **THEN** exactly one updated fact is appended whose snapshot equals the new stored state and whose `social_source_id` equals the captured fact's

#### Scenario: an updated event needs no history

- **WHEN** a consumer receives only the updated fact
- **THEN** the payload contains the complete current snapshot and parses without any prior event

### Requirement: Observed upstream deletion republishes without touching captured content

When an availability observation establishes that the provider removed the source, the service SHALL publish an updated fact with `upstream_availability = deleted_upstream` while the snapshot's text and media references remain untouched.

#### Scenario: a deleted post keeps its preserved content

- **WHEN** a preserved source is later observed deleted upstream
- **THEN** the updated fact reports `deleted_upstream` and its text and media references equal the previously published values

### Requirement: Publication is transactional, at-least-once, and idempotent

Event facts SHALL be appended in the same database transaction that writes the triggering state change, and the publisher loop SHALL deliver each fact at least once, marking it published exactly once. A crash between delivery and acknowledgement SHALL result in a redelivery of a byte-identical payload, so consumers converge by `event_id`.

#### Scenario: failure after delivery replays identical bytes

- **WHEN** the publisher delivers a fact but crashes before marking it published
- **THEN** the next run delivers a payload byte-identical to the first delivery

#### Scenario: the outbox row commits or aborts with the state change

- **WHEN** the transaction that resolves a capture rolls back
- **THEN** no event fact for that attempt remains in the outbox

### Requirement: User notes stay private

The user's capture note and any private capture data SHALL NOT appear in any published payload, envelope header, or publication log line.

#### Scenario: a noted capture publishes without its note

- **WHEN** a capture carrying a user note resolves and publishes
- **THEN** the serialized payload and envelope contain neither the note body nor a fragment of it

### Requirement: An unavailable outcome publishes nothing under the current contract

When a resolution ends in an unavailable fallback, the service SHALL NOT emit any social-source fact, because the published snapshot shape cannot represent an authorless capture without fabricating identity. The preserved unavailable record SHALL remain local exactly as it is today, and closing this representation gap SHALL go through the cross-repository contract process before any event is added.

#### Scenario: a failed resolution leaves the outbox untouched

- **WHEN** a capture's resolution ends in an unavailable fallback
- **THEN** no social-source fact is appended for that capture and the stored capture record is unchanged apart from its own status and observation rows

### Requirement: A Knowledge completion links the matching preserved revision once

The service SHALL consume `knowledge.analysis.completed.v1` through its
transactional inbox. It SHALL link a completion only when the owner,
`social_source_id`, and `content_digest` match one live resolved capture. The
stored link SHALL contain only the capture id, digest, and completion time;
the analysis result remains owned by Knowledge. Replaying the same envelope id
SHALL not create another link.

#### Scenario: matching completion round-trips to the capture

- **WHEN** Knowledge completes analysis for a captured source revision
- **THEN** the matching capture receives exactly one local digest/completion-time link and the inbox records the delivery as processed

#### Scenario: repeated completion delivery converges

- **WHEN** the same completion envelope is delivered again
- **THEN** the inbox accepts it as a duplicate and no second linkage row exists

### Requirement: Local tombstones propagate a removal fact without asserting upstream deletion

When the user or retention policy tombstones a capture, the service SHALL
commit the tombstone and exactly one `social.source.removed.v1` outbox event in
the same transaction. The event SHALL carry the stable source identity, owner,
reason, and removal time only; it SHALL not describe an Instagram deletion or
native Saved-list state. A completion received after the tombstone SHALL be
recorded as skipped and SHALL not recreate a linkage.

#### Scenario: local deletion emits one typed removal fact

- **WHEN** a preserved capture is tombstoned twice
- **THEN** one typed removal event and one tombstone exist, with no duplicate event

#### Scenario: a late completion cannot resurrect a tombstone

- **WHEN** Knowledge completes analysis after the matching capture was tombstoned
- **THEN** no analysis linkage is created for that capture
