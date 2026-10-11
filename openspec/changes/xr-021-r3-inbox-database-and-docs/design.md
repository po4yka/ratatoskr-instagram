## Context

The contracts are fixed in XR-021 CONTRACTS.md (R3-01, R3-11, R3-19) and are not redefined here. This document records only the decisions internal to this repository.

## Decisions

### One connection function, in the library

The broker connection was built inline in the binary's `BusLane::connect`, in two branches (nkey and anonymous), which an integration test cannot call. The options are now built once in `nats_transport::connect_bus(&BusConfig)` in the service library: `ConnectOptions::with_nkey(seed)` or `ConnectOptions::new()`, then `custom_inbox_prefix(INBOX_PREFIX)` applied to whichever was chosen, then `connect`. `BusLane::connect` calls it and keeps the durable verification, which the test repeats against the same stanza. `INBOX_PREFIX` is the one `const` holding `_INBOX_instagram`, beside the durable names' neighbour module, so the workspace text scan finds the literal inside the `custom_inbox_prefix(` call's argument.

### The test runs the shipped stanza, not a hand-written permission set

`tests/support/stanza.rs` starts a private `nats-server` whose users are an nkey administrator (all permissions, standing in for Edge) and the stanza read from `deploy/nats/identity.conf` with its public-key placeholder replaced by a generated key. It is a separate module that only `tests/inbox_prefix.rs` includes, because `tests/support/mod.rs` serves `nats_transport.rs`, which uses a different broker mode (user and password, a reloadable publish allowlist); sharing one module would leave unused items in one of the two binaries.

### The role file does not create the schema

Like the extractor's `role.sql`, the file creates the role, grants `connect, create` on `ratatoskr` and revokes the `public` schema; the service creates `instagram_archive` itself from `schema.sql`, so the role needs `create` on the database and nothing more.
