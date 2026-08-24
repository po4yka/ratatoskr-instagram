## Context

The repository holds documents and OpenSpec configuration only; no Rust manifest exists, so `.github/workflows/fleet.yml` has not yet activated its first-manifest checks. The fleet already contains two implemented Rust services that fix most conventions by precedent: `ratatoskr-platform` (multi-crate, three deployables, public API) and `ratatoskr-github` (foundation stage: one library crate, one service crate, operator plane only). Instagram is at the `ratatoskr-github` lifecycle stage and copies its shape, taking three refinements from `ratatoskr-platform` where instagram's spec demands more: readiness check bodies, Prometheus exposition, and a probed-database readiness fact.

Binding constraints: development status forbids a second major version and any migration ledger — `schema.sql` is edited in place forever until the owner changes the status. Cross-repository behaviour (SocialSource events, capture API shapes) stays out of this change.

## Goals / Non-Goals

**Goals:**

- A workspace that builds, lints, and tests under one command list shared by laptop and CI.
- Configuration, telemetry, errors, health endpoints, and schema each covered by at least one failing-test-first pair or an explicit reason why none applies.
- Schema application provable against real PostgreSQL locally (`compose.yaml`) and in CI (service container), never skipped when absent.

**Non-Goals:**

- No public API listener, no NATS/eventing, no OAuth, no capture or import logic (plan items 2-9).
- No OpenTelemetry/OTLP export yet — spans exist only as structured logs until a deployment needs trace export; adding it later is additive telemetry work, not a contract change.
- No Dockerfile/release artifact job yet; the gate runs on the host toolchain like `ratatoskr-github`.

## Decisions

- **Workspace shape: two members** — `crates/instagram-archive` (config, telemetry, database, schema harness) and `services/instagram-archive` (lib + bin `ratatoskr-instagram-archive`). Alternative rejected: platform's twelve-crate split buys seams for a scale instagram does not have; crates can be extracted later without behaviour change, while premature seams are hard to remove.
- **Configuration loader: hand-rolled closed-key parser**, following `ratatoskr-github`, not figment. Rationale: unknown-key refusal and value-free error rendering become direct, testable properties instead of library semantics to pin; zero extra dependencies at bootstrap; the env contract (`RATATOSKR__` prefix, `__` nesting) matches the fleet so a future file provider can slot underneath without renaming keys. Alternative (figment) adopted fleet-wide only if/when profile or file sources arrive.
- **Readiness model from platform**: a three-state runtime fact set (`startup_complete`, `draining`) plus a separately reported database probe state (absent/up/down). Readiness flips only on startup/drain; a down database is visible as a failed check with `dependency_unavailable` but does not flap readiness — the durable half keeps working during a dependency blip. The probe never opens a connection inside a request.
- **Telemetry: tracing-subscriber JSON + env-filter, metrics-exporter-prometheus** rendering through the admin plane via a handle closure, plus `instagram_build_info` and `instagram_readiness` gauges. OTLP deferred (see Non-Goals).
- **Admin paths `/health/live`, `/health/ready`, `/metrics`, `/version`**: the majority convention among running services (platform, extractor); `ratatoskr-github`'s shorter `/live` predates the fleet settling on `/health/*`.
- **Schema: one root `schema.sql`, one owned schema `instagram_archive`**, embedded into the binary with `include_str!` and applied inside one transaction under advisory lock `0x7261_7461_736b_7205` (fleet prefix `7261_7461_736b` = "ratsk"; ordinals 01-04 taken by platform, extractor, github, knowledge-lineage services; instagram takes 05). Presence check keys on `to_regnamespace('instagram_archive')`.
- **Table inventory: README's thirteen-name planned data model** (`accounts`, `credentials`, `profiles`, `media`, `media_relations`, `captures`, `capture_notes`, `export_snapshots`, `import_runs`, `raw_records`, `availability_observations`, `outbox_events`, `inbox_events`), which refines AGENTS.md's shorter conceptual list. Authority vocabulary follows README's four-value model (`authoritative_platform_state | explicit_user_capture | export_observation | legacy_observation`); AGENTS.md's list is labelled representative and omits values README carries. Acquisition vocabulary is identical in both docs. SQL conventions copied from platform: UUIDv7 minted by the application with no DEFAULT, closed vocabularies as `text` + named CHECK, `timestamptz` everywhere, secrets as `*_hash bytea`, no cross-schema foreign keys, `comment on` every table. Columns beyond identity/status/provenance stay minimal: later plan items extend definitions in place, which development status makes free.
- **Ports: admin `127.0.0.1:9082`, local PostgreSQL published on `127.0.0.1:5436`** — first free neighbours (knowledge 9081, github 9095; platform 5432, extractor 5434, github 5435). Database name/user/password `instagram`; test URL env `INSTAGRAM_ARCHIVE_TEST_DATABASE_URL` defaulting to the compose endpoint.
- **CI: single `gate` job** copying `ratatoskr-github/ci.yml`: pinned postgres:17 service container (same digest pin), `cargo fetch --locked`, `cargo deny --locked check`, fmt, clippy `-D warnings --all-targets`, build, test, doc-tests, release build, the 850-line-per-file ratchet, and the ci-versus-DEVELOPMENT.md drift guard. The unchanged OpenSpec workflow keeps running alongside per DEVELOPMENT.md.

## Risks / Trade-offs

- [Schema designed ahead of its consumers will need column changes] → Accepted and cheap by design: development status edits `schema.sql` in place and every database is disposable; tests always rebuild from the current file.
- [Hand-rolled config loader re-implements parsing edge cases] → The key space is small and closed; every failure path is table-tested, and the escape hatch to a provider library remains open without changing the env contract.
- [Boot test depends on a previously built binary] → Gate orders `cargo build --workspace --locked` before `cargo test`, matching platform's documented reason.
- [Placeholder-free but minimal metric set] → Only build info and readiness ship now; capture/oEmbed/import series arrive with their features, avoiding always-zero names.

## Migration Plan

Not applicable: no database holds data, no prior version exists. Rollout is the merge of this branch; rollback is reverting it.

## Open Questions

None blocking. Two recorded deferrals: OTLP export and the release artifact job both wait for a deployment target decision, tracked here rather than in tasks.
