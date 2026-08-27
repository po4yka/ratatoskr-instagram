## MODIFIED Requirements

### Requirement: A fresh database receives the complete first-version schema
Applying the schema definition to a fresh database SHALL create the `instagram_archive` schema containing, at minimum, the account, credential, profile, media, media-relation, capture, capture-note, export-snapshot, import-run, import-transition, export-record, completeness-report, raw-record, availability-observation, outbox-event, and inbox-event tables declared in `schema.sql`. Export receipt identity SHALL be unique per owner and archive digest, import state SHALL be constrained to the documented first-version state machine, and normalized export records and reports SHALL remain owner-scoped through their run. The set of created relations SHALL exactly match the file — no relation outside it is created, none of them is missing.

#### Scenario: Fresh apply creates every declared table and nothing else
- **WHEN** the embedded schema definition is applied to a newly created empty database
- **THEN** querying the catalog lists exactly the tables declared in `schema.sql`, all within the `instagram_archive` schema, including the complete Data Export receipt, transition, record, and report inventory

#### Scenario: Database refuses an invalid import transition state
- **WHEN** a row is inserted with an import state outside `received | inspected | parsed | reconciled | failed`
- **THEN** the insert fails with the named import-state constraint
