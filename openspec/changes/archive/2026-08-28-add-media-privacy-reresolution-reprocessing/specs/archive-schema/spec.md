## ADDED Requirements

### Requirement: Current schema exposes complete lifecycle policy and operation state

Applying the current first-version schema to a fresh database SHALL create the constrained media-retention, deletion operation/effect, local removal, blob deletion task, re-resolution run/item, and export-reprocessing run/item state required by this change. Operation identities SHALL be owner-scoped and unique, state vocabularies SHALL be closed, and checkpoint/effect constraints SHALL prevent duplicate completion. The repository SHALL contain no migration directory, migration ledger, later schema major, or migration runner.

#### Scenario: Fresh schema contains item-9 lifecycle state

- **WHEN** the embedded schema is applied to an empty disposable PostgreSQL database
- **THEN** the complete constrained lifecycle inventory exists exactly once and invalid states or duplicate owner operation identities are rejected

#### Scenario: Lifecycle schema is an in-place first-version definition

- **WHEN** repository and runtime schema assets are inspected
- **THEN** `schema.sql` is the only schema definition and no database migration file, ledger, runner, negotiation, or compatibility path exists
