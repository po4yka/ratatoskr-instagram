# Developing Ratatoskr Instagram

> Status: Proposed  
> Last reviewed: 2026-08-17

Architecture bootstrap: account OAuth, capture resolver, Data Export importer, schema, and provider adapters are not implemented.

## Intended toolchain

Rust/Tokio, Reqwest/Rustls, OAuth, SQLx/PostgreSQL, safe archive import, BlobStore, NATS, provider fixtures/WireMock, tracing, and testcontainers.

## Code size limits

There is no code here yet, so no limit is enforced yet. The commit that brings the first manifest brings the configuration that carries the limits with it: `clippy.toml` beside a `Cargo.toml`, `eslint.config.js` beside a `package.json`. `fleet.yml` fails the gate when a manifest arrives without one, so the rule has a check behind it and not only this paragraph.

`ratatoskr-workspace/docs/QUALITY_GATES.md` holds the numbers the repositories with code use today, the command that measured each one, and the limits that were rejected with the reason. Read it before you choose numbers, then measure this tree. Each limit is set at the worst case the tree already has, so that the check fails on a regression and not on work that has not been done yet.

## Workflow

1. Verify the capability exists for the connected account type and current granted scopes.
2. Record acquisition method and saved authority explicitly.
3. Resolve only public content through supported official mechanisms; preserve unavailable/private state.
4. Store raw export/capture evidence before normalization and preserve unknown records.
5. Test privacy, expiry, replay, importer limits, media policy, and no-cookie/no-hidden-API invariants.

The first scaffold PR must define exact commands. Default tests use synthetic exports and no personal account credentials.
