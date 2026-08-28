## ADDED Requirements

### Requirement: Privacy deletion publishes only final local source removal

When owner-authorized deletion removes the final live holding of a published source, the service SHALL append exactly one canonical `social.source.removed.v1` fact in the same database transaction as local content removal, its content-free audit, and the local-removal guard. The fact SHALL carry stable owner/source identity, `reason = user_requested`, and removal time only. A remaining independent holding SHALL suppress the fact, and deletion of one owner SHALL not affect another owner's fact identity.

#### Scenario: Final deletion and removal fact are atomic

- **WHEN** a database failure occurs before the deletion transaction commits
- **THEN** both the local source state and outbox remain unchanged; after a successful retry they commit once together

#### Scenario: Independent holding suppresses removal

- **WHEN** connection deletion removes official-account evidence but the same owner retains an explicit capture or export observation of the source
- **THEN** the remaining source stays analysable under its existing truthful provenance and no removed fact is appended

### Requirement: Producer deletion evidence does not claim Knowledge completion

The service SHALL report separately whether local erasure committed, BlobStore deletion remains pending or completed, and a removal fact remains unpublished or published. It SHALL NOT report Knowledge-derived data deleted until separate consumer evidence establishes that outcome.

#### Scenario: Outbox commit is not downstream deletion proof

- **WHEN** local deletion commits while the removal fact is still pending delivery
- **THEN** the operation reports local deletion complete and downstream cleanup requested, not Knowledge cleanup complete
