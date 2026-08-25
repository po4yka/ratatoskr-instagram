## MODIFIED Requirements

### Requirement: Provenance vocabularies are enforced closed
The capture and media records SHALL carry an acquisition-method column and a saved-authority column constrained by named CHECK constraints to their documented closed vocabularies (`official_api | share_extension | browser_extension | public_resolution | data_export | legacy_import` for acquisition; `explicit_user_capture | export_observation | authoritative_platform_state | legacy_observation` for authority). Inserting any other value SHALL be refused by the database.

#### Scenario: Unknown acquisition method is refused
- **WHEN** a row is inserted into a provenance-bearing table with an acquisition method outside the closed vocabulary
- **THEN** the insert fails with the named CHECK constraint

#### Scenario: Documented authority values are accepted
- **WHEN** rows are inserted using each documented saved-authority value, including `explicit_user_capture`
- **THEN** all inserts succeed

#### Scenario: Public resolution is accepted on provenance-bearing tables
- **WHEN** rows are inserted into both provenance-bearing tables using acquisition method `public_resolution`
- **THEN** all inserts succeed
