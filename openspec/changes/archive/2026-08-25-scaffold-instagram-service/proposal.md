# Scaffold the ratatoskr-instagram service

## Why

The repository is architecture bootstrap: it describes an Instagram bounded context in documents only and holds no code, so nothing about the planned service can run, be tested, or be deployed. Implementation plan item 1 creates the executable foundation every later item builds on, and the first-manifest rule in `.github/workflows/fleet.yml` requires that commit to arrive with its own product gate (`ci.yml`) and its size-limit configuration (`clippy.toml`).

## What Changes

- Add a Rust/Tokio workspace following the fleet's established layout: one library crate under `crates/instagram-archive` and one deployable binary under `services/instagram-archive`, with root `Cargo.toml`, `rust-toolchain.toml` (1.97.0), `rustfmt.toml`, `clippy.toml` (measured size limits), and `deny.toml` matching sibling repositories.
- Add finite, typed configuration loaded from `RATATOSKR__`-prefixed environment variables with strict validation; unknown keys and invalid values are startup refusals that never echo supplied values.
- Add structured telemetry: a JSON `tracing` subscriber with env-filter, Prometheus metrics exposition on the admin plane, and build-identity gauges.
- Add typed errors (`ConfigError`, `PersistenceError`, `TelemetryError`, service-level `ProcessError`) whose operator-facing renderings are value-free by construction.
- Add an operator-only admin plane on loopback serving `/health/live`, `/health/ready`, `/metrics`, `/version`, all with `Cache-Control: no-store`; readiness reports drain/startup/database checks and never opens a connection during a probe.
- Add graceful shutdown: SIGTERM/SIGINT drain, bounded shutdown timeout, pool close, exit code 0 on clean stop; `<binary> check-config` validates configuration without binding anything (exit 78 on failure).
- Add the first-version `schema.sql`: one service-owned `instagram_archive` PostgreSQL schema carrying acquisition-method and saved-authority vocabularies per the authority model in `README.md`, applied at startup inside one advisory-locked transaction, idempotently, with no migration ledger.
- Add a disposable-database test harness (`test-support` feature) that creates a uniquely named database from `schema.sql` per test.
- Add `.github/workflows/ci.yml` (fetch, deny, fmt, clippy `-D warnings`, build, test, release build, 850-line file ratchet) plus a `compose.yaml` providing local PostgreSQL; keep the unchanged docs-only OpenSpec gate alongside.
- Update `README.md` and `DEVELOPMENT.md` so stated status matches reality: scaffold present, lanes not implemented.

Out of scope: capture intake, URL canonicalization, OAuth, public resolution, Data Export import, event publishing (implementation plan items 2-9).

## Capabilities

### New Capabilities

- `service-runtime`: The deployable process contract — typed configuration loading and refusal rules, telemetry initialization, the four admin endpoints, readiness semantics, and clean shutdown behaviour. Every requirement is executable as a unit, router, or boot test without external services except PostgreSQL for the database-backed paths.
- `archive-schema`: The first-version `instagram_archive` schema contract — what objects exist after a fresh apply, that re-applying is a no-op, that provenance columns enforce their closed vocabularies, and that test databases can be created and dropped deterministically.

### Modified Capabilities

None. `openspec/specs/` is empty by design.

## Impact

- New code: workspace manifest, two crates, `schema.sql`, CI workflow, compose file. No existing code changes; documentation status lines updated.
- Fleet gates: adding `Cargo.toml` activates the `fleet.yml` first-manifest checks — satisfied by shipping `ci.yml` (with a test invocation) and `clippy.toml` together with the manifest.
- Dependencies: pinned Rust toolchain 1.97.0 and exact-pinned crates.io dependencies (axum, sqlx/postgres, tokio, tracing, metrics-exporter-prometheus); no provider credentials, no network egress at runtime beyond the configured database.
- Cross-repository contracts are untouched: SocialSource events, capture API shapes, and export completeness semantics belong to later plan items and the `ratatoskr-workspace` store.
