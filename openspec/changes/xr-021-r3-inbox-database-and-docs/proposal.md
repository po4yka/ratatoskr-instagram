## Why

Changeset XR-021 round 3 found three gaps in Instagram. First, every non-Edge identity could subscribe to the shared `_INBOX.>` namespace, so a compromised service that parses hostile content could read the JetStream deliveries of every other service; the deployed ACL now lets Instagram subscribe only to `_INBOX_instagram.>`, and a connection that does not set that inbox prefix never receives a JetStream reply. Second, the operator example names a database, `ratatoskr_instagram`, that no deployment creates, and nothing creates the role. Third, the README says no workspace pin or integration profile exists, which is no longer true. Cross-repository behaviour is defined in XR-021 CONTRACTS.md R3-01 (per-service inbox prefixes), R3-11 (one database, one role file per service) and R3-19 (stale documentation); this change cites them and does not restate them.

## What Changes

- **BREAKING** Every NATS connection the service opens sets `custom_inbox_prefix("_INBOX_instagram")` through one function, `nats_transport::connect_bus`, that `BusLane::connect` calls. A service built before this change cannot complete JetStream requests under the new ACL; nothing is deployed, so the break is named, not staged (R3-01).
- Ship `deploy/nats/identity.conf`, a byte-equal copy of the `INSTAGRAM` stanza of Platform's `ratatoskr.conf`, and a test that runs a private broker with that stanza and completes a JetStream request through the real connection function.
- **BREAKING** `deploy/systemd/instagram.conf.example` names the shared database `ratatoskr` instead of `ratatoskr_instagram`; `deploy/postgres/role.sql` creates the `ratatoskr_instagram` role with `connect, create` on it, and the README states the provisioning order (R3-11).
- `README.md` states the workspace integration as it is (R3-19).

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `service-runtime`: the per-service inbox prefix and the shared-database operator example.

## Impact

- `services/instagram-archive`: `src/nats_transport.rs`, `src/bus.rs`, `tests/inbox_prefix.rs`, `tests/support/stanza.rs`, `tests/boot.rs`, `Cargo.toml` (`nkeys` as a dev-dependency, already in `Cargo.lock` through `async-nats`).
- New `deploy/nats/identity.conf`, `deploy/postgres/role.sql`; `deploy/systemd/instagram.conf.example`, `README.md`.
- Out of scope: calibrating `classify_response` against live Meta responses, a trust-anchor key for the Meta endpoint (allow-listed on purpose), the Instagram OAuth and publishing paths, and Platform's ACL.
