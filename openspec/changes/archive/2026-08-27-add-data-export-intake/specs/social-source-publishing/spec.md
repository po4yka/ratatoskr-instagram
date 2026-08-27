## ADDED Requirements

### Requirement: Reconciled Data Export observations publish normalized source facts
When a reconciled Data Export run first preserves a stable provider media identity, the service SHALL append one `social.source.captured.v1` fact; when it adds a new normalized revision to an already published identity, it SHALL append one `social.source.updated.v1` fact. Each fact SHALL commit in the same transaction as its projection, carry the complete current snapshot, use `data_export` acquisition and `export_observation` saved authority, reference the immutable raw evidence through a valid `BlobRef`, and make no native-Saved or account-history-completeness claim.

#### Scenario: First export observation publishes once
- **WHEN** reconciliation first preserves one stable provider media identity from a supported Data Export
- **THEN** exactly one captured fact commits with `data_export` acquisition, `export_observation` authority, and the run's verifiable raw BlobRef

#### Scenario: Replayed import does not duplicate a source fact
- **WHEN** a completed Data Export run is reconciled again
- **THEN** no additional captured or updated fact is appended for unchanged normalized content

### Requirement: Export absence never publishes removal or upstream deletion
The service SHALL NOT publish `social.source.removed.v1`, change upstream availability, or remove preserved content merely because an identity, capture, relation, or export category is absent from a Data Export. Only separate evidence accepted by the existing removal or availability contracts can cause those facts.

#### Scenario: Missing export identity leaves the outbox unchanged
- **WHEN** a later Data Export omits an identity previously preserved by capture, official observation, or another export
- **THEN** no removal or upstream-availability fact for that identity is appended and its preserved snapshot is unchanged
