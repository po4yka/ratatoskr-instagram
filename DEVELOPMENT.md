# Developing Ratatoskr Instagram

> Status: Proposed  
> Last reviewed: 2026-08-17

Architecture bootstrap: account OAuth, capture resolver, Data Export importer, schema, and provider adapters are not implemented.

## Intended toolchain

Rust/Tokio, Reqwest/Rustls, OAuth, SQLx/PostgreSQL, safe archive import, BlobStore, NATS, provider fixtures/WireMock, tracing, and testcontainers.

## Workflow

1. Verify the capability exists for the connected account type and current granted scopes.
2. Record acquisition method and saved authority explicitly.
3. Resolve only public content through supported official mechanisms; preserve unavailable/private state.
4. Store raw export/capture evidence before normalization and preserve unknown records.
5. Test privacy, expiry, replay, importer limits, media policy, and no-cookie/no-hidden-API invariants.

The first scaffold PR must define exact commands. Default tests use synthetic exports and no personal account credentials.
