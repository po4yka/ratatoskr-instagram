## ADDED Requirements

### Requirement: The service connects under its own inbox prefix

Every NATS connection the service opens SHALL set the custom inbox prefix `_INBOX_instagram` through one connection function, and the repository SHALL ship `deploy/nats/identity.conf`, a byte-equal copy of the `INSTAGRAM` stanza of the deployed ACL, whose only subscribe permission is `_INBOX_instagram.>` (XR-021 CONTRACTS.md R3-01).

#### Scenario: A JetStream request completes under the shipped stanza

- **WHEN** the service's connection function connects to a broker running only the shipped stanza and the administrator has provisioned the command stream, its durable and the events stream
- **THEN** the service verifies and pulls from its durable and publishes `evt.platform.operation.reported.v1` with a JetStream acknowledgement

### Requirement: The operator example names the shared database

`deploy/systemd/instagram.conf.example` SHALL name the database `ratatoskr` on port 5432 with the role `ratatoskr_instagram`, and `deploy/postgres/role.sql` SHALL create that role without a password and without superuser, createdb, createrole, replication or bypassrls rights, grant it `connect, create` on `ratatoskr` and revoke the `public` schema from it (XR-021 CONTRACTS.md R3-11).

#### Scenario: The example and the role file agree

- **WHEN** the example is loaded through the configuration loader and the role file is read
- **THEN** the database URL names `ratatoskr_instagram` and `ratatoskr`, the file creates that role, and the example still loads
