## 1. Workspace scaffold

- [x] 1.1 Create root `Cargo.toml` (two members: `crates/instagram-archive`, `services/instagram-archive`), `rust-toolchain.toml` pinning 1.97.0, `clippy.toml` (msrv 1.97, test allowances, disallowed env reads, size limits 100/7/5), `deny.toml`, `rustfmt.toml`, member manifests with workspace lints and exact-pinned dependencies, and lib/bin skeletons that compile. Verification: `cargo check --workspace --locked` succeeds. This task cannot start from a failing test: it creates configuration and manifest files only.
- [x] 1.2 Add `compose.yaml` (postgres:17 pinned by digest on 127.0.0.1:5436, ICU initdb args) and start it; verify `pg_isready -h 127.0.0.1 -p 5436 -U instagram` succeeds. This task cannot start from a failing test: local infrastructure provisioning.

## 2. Configuration strictness

- [x] 2.1 Add `crates/instagram-archive/tests/config.rs` with tests named for the spec scenarios: empty environment yields loopback default bind on port 9082 with no database URL; unknown key `RATATOSKR__NOT_A_SECTION__VALUE=1` is refused naming the key without echoing its value; non-loopback admin bind plus zero connection limit are both reported in one error with neither value rendered; a recognized override changes exactly its own field; the operator-facing report of a valid configuration renders no secret material. Confirm they fail against a skeleton loader that accepts everything. Verification: `cargo test -p ratatoskr-instagram-archive --test config --locked` fails on the stated assertions.
- [x] 2.2 Implement the strict closed-key typed loader (`src/config.rs`: defaults, validation collecting every violation, value-free report) until green. Verification: same command green.

## 3. Telemetry bootstrap

- [x] 3.1 Add `crates/instagram-archive/tests/telemetry.rs`: initialization succeeds once and a second call in the same process returns a typed already-installed error; emitted records parse as JSON carrying service name, version, and git SHA fields. Confirm it fails while initialization is unimplemented. Verification: `cargo test -p ratatoskr-instagram-archive --test telemetry --locked` fails to provide the passing behavior.
- [x] 3.2 Implement `src/telemetry.rs` (JSON subscriber via try-init with env filter, Prometheus handle, build-info gauge, typed `TelemetryError`) until green. Verification: same command green.

## 4. Owned schema and disposable-database harness

- [x] 4.1 Add `src/database.rs` (`Database::connect`, `apply_schema` under advisory lock keyed on schema presence, `ping`, `close`, typed `PersistenceError`) and the `test-support` feature exposing `TestDatabase`; add `crates/instagram-archive/tests/schema.rs` asserting: fresh application succeeds into an empty database; applying twice succeeds identically; concurrent applications from two connections both succeed applying once; exactly the declared `instagram_archive` relations exist and nothing outside it; inserting a capture row with acquisition method `carrier_pigeon` fails on the named CHECK while every documented authority value inserts; catalog inspection finds zero cross-schema foreign keys; two harness databases are isolated and both drop on cleanup. Start `schema.sql` at the bare `create schema` statement so the run fails because tables are absent. Verification: `cargo test -p ratatoskr-instagram-archive --test schema --locked` fails on missing tables.
- [x] 4.2 Write the first-version `schema.sql`: `instagram_archive` tables `accounts`, `credentials`, `profiles`, `media`, `media_relations`, `captures`, `capture_notes`, `export_snapshots`, `import_runs`, `raw_records`, `availability_observations`, `outbox_events`, `inbox_events` with UUIDv7 primary keys without database defaults, named CHECK vocabularies for status/acquisition/saved-authority columns, uniqueness on provider identities where declared, `timestamptz` timestamps, and comments per platform SQL conventions. Verification: same command green.

## 5. Operator routes

- [x] 5.1 Add `services/instagram-archive/tests/admin.rs`: `/health/live` answers 200 in starting, ready, and draining states; `/health/ready` answers 503 before startup completion, 200 after, 503 while draining, with name-sorted checks including a failing database check that carries reason `dependency_unavailable` without flipping readiness; `/metrics` returns Prometheus text containing `instagram_build_info`; `/version` returns service name, version, git SHA, Rust version; every response including unknown-path 404 carries `Cache-Control: no-store`. Confirm it fails against a router serving constant success bodies. Verification: `cargo test -p ratatoskr-instagram-archive-service --test admin --locked` fails on the readiness transitions.
- [x] 5.2 Implement the runtime state machine (startup/drain facts plus absent/up/down database probe state) and the admin router until green. Verification: same command green.

## 6. Process boot

- [x] 6.1 Add `services/instagram-archive/tests/boot.rs`: spawn the real binary against a disposable database with environment configuration; assert `check-config` exits 0 binding no port and exits 78 with a value-free report under invalid configuration; assert `/health/ready` reaches 200 after startup; assert `/health/live`, `/metrics`, `/version` return 200 and an unknown path 404 while serving; send SIGTERM and assert exit code 0 within the shutdown bound. Confirm it fails because the process does not serve yet. Verification: `cargo test -p ratatoskr-instagram-archive-service --test boot --locked` fails with readiness not arriving.
- [x] 6.2 Implement `services/instagram-archive/src/main.rs`: configuration load with `check-config` mode, telemetry init, database connect and schema application refusing startup on failure, listener bind, readiness marking, periodic database probe feeding the readiness fact, graceful SIGTERM/SIGINT drain within the configured bound exiting 0. Verification: same command green.

## 7. Gates and documentation

- [x] 7.1 Add `.github/workflows/ci.yml` (postgres service container pinned by digest, pinned actions with `persist-credentials: false`, fetch/deny/fmt/clippy/build/test/doc-test/release/file-ratchet steps plus the ci-versus-DEVELOPMENT.md drift guard) and the matching fenced gate block under "### Rust — the CI gate" in DEVELOPMENT.md; update DEVELOPMENT.md status text from docs-only bootstrap to the running foundation. This task cannot start from a failing test: CI and documentation artifacts; their consistency check is the drift guard step itself.
- [x] 7.2 Update README.md status blockquote to describe the implemented foundation (service runs locally against PostgreSQL with health endpoints and owned `instagram_archive` schema) while keeping account connection, captures, resolution, imports, and events marked planned. Update the repository context paragraph in `openspec/config.yaml` that states "This repository holds no code yet". This task cannot start from a failing test: documentation.

## 8. Full gate

- [x] 8.1 Run the complete gate list from DEVELOPMENT.md against a clean tree — `git diff --check`, `openspec validate --all --strict`, `openspec validate --archived`, and every cargo step of ci.yml — and record the results. Verification: every command exits zero.
