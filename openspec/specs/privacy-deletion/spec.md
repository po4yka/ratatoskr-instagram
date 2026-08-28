## Purpose

Defines owner-authorized, inventory-complete privacy deletion for captures and official account connections without erasing independent holdings or claiming downstream completion prematurely.

## Requirements

### Requirement: Every owned data and blob class is explicitly classified for deletion

The service SHALL maintain a closed deletion inventory equal to the current `instagram_archive` relation set plus every Instagram-owned BlobStore class. Capture and account-connection deletion SHALL each classify every inventory member as remove, detach, retain, audit, outbox, or not applicable. An unknown or duplicate class MUST fail planning before mutation.

#### Scenario: Schema growth cannot silently escape deletion policy

- **WHEN** the current schema or BlobStore inventory contains a class absent from either target classification
- **THEN** deletion planning fails naming the missing class and no row, blob reference, outbox event, or audit record changes

### Requirement: Deletion is owner-bound, deterministic, and replay-safe

Capture and account-connection deletion SHALL require a stable operation identity and an owner matching the target. Preview SHALL return deterministic bounded per-class effects without mutation. Apply SHALL re-read and lock the target, recompute the plan, refuse a changed or cross-owner target, and commit all database removals, reference detachments, content-free audit effects, local-removal guards, BlobStore tasks, and required outbox facts in one transaction. Replaying a completed operation SHALL return its stored result without another effect.

#### Scenario: Cross-owner deletion changes nothing

- **WHEN** an owner requests deletion of another owner's capture or connection
- **THEN** the request is refused and database contents, blob inventory, outbox, and audit remain byte-for-byte unchanged

#### Scenario: Preview and apply enumerate the same effects

- **WHEN** preview and apply run against unchanged target state
- **THEN** their ordered per-class counts and retained-shared classifications match, while only apply commits the effects and audit

### Requirement: Final local holdings emit removal facts atomically

Apply SHALL emit exactly one `social.source.removed.v1` fact with `reason = user_requested` for every source whose final live holding for that owner disappears. The database removal, content-free local-removal guard, and outbox append MUST commit atomically. A remaining explicit capture, official-account observation, or export observation for the same owner SHALL suppress removal and preserve its provenance. Another owner's holding SHALL never enter the operation. Producer completion proves the committed removal fact, not that Knowledge has consumed it.

#### Scenario: Deleting one duplicate capture preserves the source

- **WHEN** an owner deletes one of two live captures referring to the same source
- **THEN** only capture-specific data disappears, shared source/revision/media evidence remains, and no removal fact is appended

#### Scenario: Deleting the final holding requests Knowledge cleanup

- **WHEN** deletion removes an owner's final holding of a published source
- **THEN** one content-free removal fact commits with the local erasure and audit, and its payload contains no URL, caption, note, raw body, credential, or private path

### Requirement: Late completion cannot resurrect locally removed state

After a local-removal guard commits, a matching or replayed Knowledge completion SHALL NOT recreate analysis linkage or make the source live. It SHALL be handled idempotently without source content.

#### Scenario: Completion arrives after privacy deletion

- **WHEN** a matching Knowledge completion is received after the source's final local removal
- **THEN** no analysis linkage or source projection is created and replaying the completion adds nothing
