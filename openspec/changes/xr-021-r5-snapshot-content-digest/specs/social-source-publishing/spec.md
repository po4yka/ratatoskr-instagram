## ADDED Requirements

### Requirement: The content digest describes the post

The `content_digest` of every Instagram source snapshot SHALL be the SHA-256 digest of the canonical JSON of `{"domain":"instagram.snapshot-content","permalink":<canonical permalink>,"external_post_id":<shortcode>,"caption":<caption or null>,"upstream":"available"|"deleted"|"unavailable","media":[]}` computed from the snapshot itself by one function, and no constant digest SHALL remain (XR-021 CONTRACTS.md R5-09).

#### Scenario: A changed caption is a different digest

- **WHEN** two snapshots of one post differ only in the caption
- **THEN** their digests differ, and two builds of the same stored rows have equal digests

#### Scenario: A post deleted upstream is a third digest

- **WHEN** the snapshot of a post is built before and after the post is observed deleted upstream
- **THEN** the digest after the observation differs from the digest before and from the digest of any other caption

#### Scenario: The preimage bytes are fixed

- **WHEN** the digest of a snapshot with a caption and an available upstream is computed
- **THEN** it equals the SHA-256 of the golden preimage bytes pinned in the test

### Requirement: An updated fact is observed when the post changed

A `social.source.updated.v1` fact SHALL carry `captured_at` equal to the newest of the capture's own time, the current revision's `resolved_at` and the media projection's observation time, and a `social.source.captured.v1` fact SHALL keep the capture's own time.

#### Scenario: A re-resolution that changes the caption is later and different

- **WHEN** a capture resolves, and a later re-resolution changes the caption
- **THEN** the updated fact's digest differs from the captured fact's and its `captured_at` equals the new revision's `resolved_at`, which is later than the capture's own time

#### Scenario: An unchanged re-resolution publishes nothing

- **WHEN** a re-resolution answers with the same media type and caption
- **THEN** no further fact is appended

#### Scenario: A deletion observed upstream is a later, different fact

- **WHEN** a resolved post is observed deleted upstream
- **THEN** an updated fact with `upstream_availability = deleted_upstream`, a third digest and a `captured_at` equal to the observation instant is appended

#### Scenario: The updated fact validates as the contract

- **WHEN** the updated fact is decoded as `SocialSourceUpdated`
- **THEN** `validate` accepts it and its `captured_at` is strictly after the captured fact's

### Requirement: A completion links only the current digest

The service SHALL link a Knowledge analysis completion to a capture only when the completion's digest equals the digest of the capture's current snapshot, and SHALL store that digest.

#### Scenario: A stale completion does not link

- **WHEN** a completion for the first digest arrives after the capture was updated
- **THEN** it is recorded as skipped and no link row exists for that digest

#### Scenario: A completion for the new digest links

- **WHEN** a completion for the updated fact's digest arrives
- **THEN** one link row stores that digest for the capture
