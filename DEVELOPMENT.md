# Developing Ratatoskr Instagram

> Status: Active development
> Last reviewed: 2026-08-25

Implementation plan items 1–4 are implemented: a Rust/Tokio service with typed strict configuration, structured telemetry, operator health routes, typed errors, and the first-version `instagram_archive` schema applied at startup; plus the explicit capture intake — permalink canonicalization, idempotent capture identity on `(user_ref, canonical_url)`, the unavailable fallback record, and `POST /v1/captures` on a second loopback listener; plus supported public resolution — an approved embed/oEmbed-style surface seam (recorded fixtures in tests), immutable parser-versioned revisions over content-addressed raw payloads, deterministic normalization into `media`, and verbatim failure observations. The production network client behind the seam arrives with provider credentials; account OAuth, Data Export import, and eventing are not implemented yet.

## Intended toolchain

Rust/Tokio (pinned by `rust-toolchain.toml` at 1.97.0), SQLx/PostgreSQL, axum, tracing, Prometheus. Planned for later items: Reqwest/Rustls, OAuth, safe archive import, BlobStore, NATS, provider fixtures/WireMock, testcontainers.

## Code size limits

`clippy.toml` beside the root `Cargo.toml` carries the limits: functions at most 100 lines of code, signatures at most 7 arguments, block nesting at most 5 deep, plus `allow-unwrap-in-tests` and the disallowed direct environment reads outside the config module. The numbers are the fresh-tree baseline, not an ambition; an exception is a site-level `#[expect]` with a reason. The gate also enforces the one limit clippy cannot express: no tracked `.rs` file may exceed 850 lines.

## Current validation

The repository has two gates. The docs-only/OpenSpec gate stays unchanged:

```bash
git diff --check
openspec validate --all --strict
openspec validate --archived
```

`.github/workflows/openspec.yml` runs the two OpenSpec commands in CI; `.github/workflows/fleet.yml`
keeps checking its invariants now that a manifest exists.

### Rust — also the CI gate

```bash
cargo fetch --locked
cargo deny --locked check
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --workspace --locked
cargo test --workspace --locked
cargo test --workspace --locked --doc
cargo build --workspace --locked --release
```

`.github/workflows/ci.yml` runs this list against PostgreSQL 17 (service container in CI,
`compose.yaml` on a laptop: user/password/database `instagram`, published on `127.0.0.1:5436`). The
suite creates disposable databases from the embedded schema per test; without the server the suite
fails rather than skips. CI additionally runs the 850-line file ratchet and a guard asserting this
command list is byte-identical to `.github/workflows/ci.yml`.

## Local run

```bash
docker compose up -d
cargo run -p ratatoskr-instagram-archive-service
# operator plane on 127.0.0.1:9082: /health/live /health/ready /metrics /version
# product plane on 127.0.0.1:9083: POST /v1/captures
```

`RATATOSKR__STORAGE__DATABASE_URL=postgres://instagram:instagram@127.0.0.1:5436/instagram` is
required to start; `<binary> check-config` validates configuration without binding (exit 78 when
invalid). Both listeners bind loopback only; the product plane trusts its caller to name the acting
`user_ref` because user authentication lives in `ratatoskr-platform`, so opening it beyond loopback
is a deployment decision to make together with that boundary moving.

Set mandatory `RATATOSKR__BUS__URL` to a credential-free `nats://` or `tls://` endpoint for the
durable Instagram browser-capture consumer. In production also set
`RATATOSKR__BUS__NKEY_SEED_PATH` to the absolute path of the service nkey seed. The NATS role must
subscribe to `cmd.instagram.capture.requested.v1` and acknowledge the preprovisioned durable
consumer `ratatoskr_instagram_browser_capture`; Platform owns creation of the `ratatoskr_commands`
stream and that consumer. The Instagram identity must not receive broad `$JS.API.>` permission.
Inability to authenticate, connect, or obtain the fixed consumer aborts startup.

## Workflow

1. Verify the capability exists for the connected account type and current granted scopes.
2. Record acquisition method and saved authority explicitly.
3. Resolve only public content through supported official mechanisms; preserve unavailable/private state.
4. Store raw export/capture evidence before normalization and preserve unknown records.
5. Test privacy, expiry, replay, importer limits, media policy, and no-cookie/no-hidden-API invariants.

The first scaffold PR must define exact commands. Default tests use synthetic exports and no personal account credentials.

## What a clone needs before you plan a change

A change is planned with OpenSpec, which is a CLI a clone installs for itself. Use the version
`.github/workflows/openspec.yml` pins, so your terminal and the gate answer the same:

```bash
npm install --global @fission-ai/openspec@1.10.0
```

Cross-repository behaviour lives in a store, and registering one is per-machine state that no
repository can turn on for you — the same kind of step as `git config core.hooksPath .githooks`:

```bash
git clone git@github.com:po4yka/ratatoskr-workspace.git <path>
openspec store register <path> --id ratatoskr-workspace
```

`openspec doctor` reports whether both are in place.

## The Rust skills in this repository

`.agents/skills/` holds eighteen Rust skills vendored from `po4yka/rust-skills`, and
`.claude/skills/` symlinks to them. Unlike the steps above this needs nothing from your machine: the
files are in the tree, so a fresh clone already has them.

Update them with the catalogue and never by hand:

```bash
npx skills update
```

That rewrites `.agents/skills/` and `skills-lock.json` from the catalogue. Run it in one repository,
read the diff, then apply the same change to every Ratatoskr repository whose stack is Rust.
`ratatoskr-workspace/.github/workflows/drift.yml` fails when one copy differs from the others.
