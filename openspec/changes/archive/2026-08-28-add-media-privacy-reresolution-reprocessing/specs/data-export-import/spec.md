## ADDED Requirements

### Requirement: Retained immutable exports support explicit parser reprocessing

An owner SHALL be able to dry-run or apply an exact supported parser version against a retained reconciled Data Export receipt without reacquiring or rewriting the original archive. Reprocessing SHALL preserve the initial import run, parser evidence, transition history, report, raw records, and prior projections as addressable evidence; it SHALL use separate owner-scoped operation state and SHALL never silently change an existing run's parser identifier.

#### Scenario: Reprocessing leaves initial import evidence intact

- **WHEN** a retained reconciled archive is successfully reprocessed under another supported parser
- **THEN** its original receipt, archive reference, import transitions, parser identifier, report, and prior revisions remain unchanged and retrievable beside the reprocessing report

### Requirement: Parser-version reprocessing is not legacy or database migration

Reprocessing SHALL operate only on immutable Data Export receipts owned by the current service. It SHALL NOT import a legacy monolith, scan another repository/database, run schema/data migrations, or create compatibility paths for old APIs or schemas.

#### Scenario: Non-export source is refused

- **WHEN** reprocessing is requested for a legacy source or an identifier without an owner-matching immutable Data Export receipt
- **THEN** it is refused before planning and no external system, schema migration, or compatibility path is invoked
