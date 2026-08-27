## ADDED Requirements

### Requirement: Own-media jobs use current owner-bound authorization

The system SHALL execute own-media work only with the encrypted credential owned by the targeted account and Ratatoskr user, after verifying that the account is connected and its current complete capability generation makes `own_media_read` available. Finalization MUST revalidate the same ownership, connection, and capability evidence; any downgrade or reauthorization transition during a run SHALL prevent authority and watermark replacement.

#### Scenario: One account credential cannot read another account

- **WHEN** scheduled work names an account and owner that do not match the stored credential binding
- **THEN** the job is refused before provider contact and no other account's token is opened

#### Scenario: Mid-run downgrade blocks completion

- **WHEN** permission reconciliation replaces the run's available capability generation before finalization
- **THEN** the staged run cannot replace the current own-media authority or watermark and records the new capability limit
