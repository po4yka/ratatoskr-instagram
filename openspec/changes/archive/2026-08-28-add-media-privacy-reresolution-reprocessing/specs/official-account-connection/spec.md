## ADDED Requirements

### Requirement: Owner-authorized connection deletion removes all connection-derived personal data

Deleting an official account connection SHALL erase its encrypted credentials, live OAuth flow secrets, permission/capability observations, provider usage records, sync checkpoints/runs/staging, connection-derived normalized revisions and raw evidence, media-byte references, analysis linkages, and other account-owned personal data as classified by the complete privacy-deletion inventory. It SHALL retain only content-free audit/removal/outbox evidence and shared records still required by another live holding.

#### Scenario: Deleted connection leaves no recoverable credential or exclusive source

- **WHEN** an owner deletes a connection whose sources have no other live holding
- **THEN** no credential or connection-exclusive source content remains, each final published source has one removal fact, and retained audit rows contain no secret or source content

### Requirement: Connection deletion preserves independent lanes

Connection deletion SHALL NOT delete or weaken a same-owner explicit capture or Data Export observation merely because it refers to the same provider identity. It SHALL preserve that holding's acquisition, saved authority, capture time, raw evidence, and source identity. It SHALL never affect another owner.

#### Scenario: Explicit capture survives connection deletion

- **WHEN** official-account and explicit-capture evidence refer to the same provider media and the owner deletes the connection
- **THEN** the connection-derived evidence is removed, the capture remains unchanged with `explicit_user_capture` authority, and no removal fact is appended for the still-held source
