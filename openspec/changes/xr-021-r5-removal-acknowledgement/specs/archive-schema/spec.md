## ADDED Requirements

### Requirement: The outbox records the acknowledgement of a removal

`instagram_archive.outbox_events` SHALL carry `acknowledged_at`, `ack_outcome` (`applied` or `refused`, present exactly when `acknowledged_at` is), `resend_count` (non-negative, default 0) and `next_resend_at`, and only `social.source.removed.v1` rows SHALL use them (XR-021 CONTRACTS.md R5-11 rule 4).

#### Scenario: The database refuses an acknowledgement on another event type

- **WHEN** a row of another event type is inserted with `acknowledged_at` set
- **THEN** the insert fails with the named constraint

#### Scenario: The database refuses an outcome without an acknowledgement

- **WHEN** a removal row is inserted with `ack_outcome` set and `acknowledged_at` null, or with an outcome outside `applied | refused`
- **THEN** the insert fails with the named constraint
