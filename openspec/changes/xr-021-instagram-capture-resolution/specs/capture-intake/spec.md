## ADDED Requirements

### Requirement: A browser capture command records its Platform operation and queues a report

The service SHALL record the Platform operation named by an accepted browser capture command in `capture_operations`, one row per operation id, bound to the capture it reuses or creates, in the same transaction that holds the broker inbox claim (XR-021 CONTRACTS.md S10 CD7). The same transaction SHALL append one `platform.operation.reported.v1` outbox row carrying a complete `EventEnvelope` with status `queued` and stage `capture_queued` (S10 CD1, CD2), and SHALL make the capture due for resolution. A redelivery SHALL add nothing. A later explicit capture of a permalink whose capture is `unavailable` SHALL reset it to `accepted` with zero resolution attempts.

#### Scenario: Intake queues a report

- **WHEN** a valid browser capture command is ingested
- **THEN** one `capture_operations` row holds the operation id, command id, capture id and owner with `reported_at` unset, the capture has a due `next_resolution_at`, and one outbox row carries a queued report enveloped for tenant `user:<owner>` with correlation `operation:<operation id>`

#### Scenario: A redelivery adds nothing

- **WHEN** the same command is delivered twice
- **THEN** exactly one operation row and one queued report exist

#### Scenario: A second operation reuses the capture

- **WHEN** a command with another operation id names a permalink the owner already captured
- **THEN** a second operation row binds to the same capture, and an `unavailable` capture returns to `accepted` with zero attempts
