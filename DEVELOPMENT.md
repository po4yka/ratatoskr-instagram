# Developing Ratatoskr Instagram

> Status: Active development
> Last reviewed: 2026-08-28

Implementation plan items 1–9 are implemented. The official account lane, own-media scheduler,
authenticated Data Export lane, media-byte retention, blob deletion, re-resolution, and reprocessing
mutation are disabled by default. Data Export stores the exact ZIP before processing, uses parser
`instagram-saved-posts-json-v1`, and reports capture/export gaps without deletion inference. Only
synthetic/redacted export and reprocessing compatibility has been verified.

## Intended toolchain

Rust/Tokio (pinned by `rust-toolchain.toml` at 1.97.0), SQLx/PostgreSQL, axum, tracing, Prometheus, Reqwest/Rustls, AES-256-GCM, and ZIP/Deflate inspection. NATS and PostgreSQL fixtures are required by the full gate.

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

`.github/workflows/ci.yml` runs this list against PostgreSQL 17 and an isolated NATS 2 JetStream
fixture. `compose.yaml` exposes PostgreSQL on `127.0.0.1:5436` (user/password/database `instagram`)
and NATS on `127.0.0.1:14225`. Set `INSTAGRAM_ARCHIVE_TEST_DATABASE_URL` to the PostgreSQL URL and
`INSTAGRAM_ARCHIVE_TEST_NATS_URL=nats://127.0.0.1:14225` before the gate. The suite creates
disposable databases and its own JetStream stream/consumer; missing fixtures fail rather than skip.
CI additionally runs the 850-line file ratchet and a guard asserting this command list is
byte-identical to `.github/workflows/ci.yml`.

## Local run

```bash
docker compose up -d
RATATOSKR__BUS__URL=nats://127.0.0.1:14225 cargo run -p ratatoskr-instagram-archive-service
# operator plane on 127.0.0.1:9082: /health/live /health/ready /metrics /version
# product plane on 127.0.0.1:9083: POST /v1/captures, POST/GET /v1/data-exports
```

`RATATOSKR__STORAGE__DATABASE_URL=postgres://instagram:instagram@127.0.0.1:5436/instagram` is
required to start; `<binary> check-config` validates configuration without binding (exit 78 when
invalid). Both listeners bind loopback only; the product plane trusts its caller to name the acting
`user_ref` because user authentication lives in `ratatoskr-platform`, so opening it beyond loopback
is a deployment decision to make together with that boundary moving.

Official OAuth stays off unless `RATATOSKR__OAUTH__ENABLED=true`. Enabling it requires all of:

- client ID/secret, the exact HTTPS redirect URI, and Graph version `v26.0`;
- the HTTPS Platform relay claim URL and its service bearer token;
- `CURRENT_KEY_VERSION` plus `KEYRING`, encoded as comma-separated `version:base64-32-byte-key` entries;
- bounded connect/request/total timeouts, response size, discovery retries, call budget, and flow TTL.

All use the `RATATOSKR__OAUTH__` prefix shown by the config parser. Effective configuration and errors
redact client secret, relay token, and key material. A separate Platform change must register the
Instagram provider/callback and relay grant before operators enable this flag. Key retirement is an
operator retention decision: keep every version needed to decrypt a live row; this item does not add
a bulk re-encryption tool.

Own-media traffic additionally requires `RATATOSKR__OWN_MEDIA__ENABLED=true`; the strict companion
settings are `CADENCE_SECONDS`, `ACCOUNTS_PER_TICK`, `PAGES_PER_RUN`, and `CALL_BUDGET` under the
same `RATATOSKR__OWN_MEDIA__` prefix. The loop consumes the immediate Tokio interval tick and starts
only after one cadence. Tests call `run_due_once` directly and never sleep. Keep this flag off until
the OAuth product has the reviewed permission and deployment authorization.

Data Export additionally requires `RATATOSKR__DATA_EXPORT__ENABLED=true`, absolute disjoint
`BLOB_ROOT` and `STAGING_ROOT`, and `BEARER_TOKENS=owner-uuid:opaque-token[,..]`. Every archive,
entry, path, ratio, poll, and batch limit is configured under the same prefix; defaults and exact
HTTP/parser behavior are documented in `README.md`. Roots must be private and service-owned. Do not
enable the lane until retention/access policy and owner-bound credentials are provisioned.

Item-9 capabilities use four closed prefixes. `MEDIA_RETENTION` requires object bytes, owner bytes,
and URL-lifetime ceilings; `BLOB_DELETION` requires poll, batch, and attempt ceilings;
`RE_RESOLUTION` requires recency, item, request, response-byte, duration, concurrency, and provider
call budgets; `REPROCESSING` requires a maximum item count per invocation. Each section stays
disabled unless its `ENABLED=true` and every companion value is explicitly finite and nonzero.

Parser reprocessing is an operator process mode, not HTTP:

```bash
ratatoskr-instagram-archive reprocess-export dry-run --owner UUID --run-id UUID --parser instagram-saved-posts-json-v1
ratatoskr-instagram-archive reprocess-export apply --owner UUID --run-id UUID --parser instagram-saved-posts-json-v1 --operation-id UUID
```

Both modes write exactly one newline-terminated JSON report to stdout and diagnostics only to
stderr. Exit codes are `0` success, `1` operational/integrity failure, `2` invalid grammar, and `78`
invalid configuration. Apply requires `RATATOSKR__REPROCESSING__ENABLED=true`; dry-run remains
read-only. Legacy monolith import is intentionally absent and belongs to fleet cutover.

Roll out against a freshly created development database with all four item-9 flags off. Verify
deletion/outbox/blob convergence and a reprocessing dry-run first, then enable one bounded worker or
apply lane at a time while watching the closed lifecycle metrics. The Instagram outbox is the
producer evidence boundary; Knowledge must independently prove consumption of deletion requests.
Rollback disables the workers and CLI apply, leaving committed audit, outbox, checkpoint, and blob
tasks intact for safe replay. There is no down migration: development databases are recreated from
the single prior `schema.sql`, and parser rollback is another explicit supported reprocessing run.

The deterministic hostile/property suite is part of `cargo test`. With local nightly and
`cargo-fuzz`, run the additional bounded smoke as:

```bash
build-gate -- cargo +nightly fuzz run data_export_archive -- -max_total_time=60
```

Set `RATATOSKR__BUS__URL` to a credential-free `nats://` or `tls://` endpoint to enable both the
durable Instagram browser-capture consumer and acknowledged SocialSource publisher. In production also set
`RATATOSKR__BUS__NKEY_SEED_PATH` to the absolute path of the service nkey seed. The NATS role must
subscribe to `cmd.instagram.capture.requested.v1` and acknowledge the preprovisioned durable
consumer `ratatoskr_instagram_browser_capture`; Platform owns creation of the `ratatoskr_commands`
stream and that consumer. It may publish only `evt.social.source.captured.v1`,
`evt.social.source.updated.v1`, and `evt.social.source.removed.v1`; it must not receive broad
`$JS.API.>` permission. Inability to authenticate, connect, or obtain the fixed consumer aborts
configured-bus startup. Omitting the bus is explicit standalone mode: no broker task starts and no
outbox row is attempted.

Before the first deployment that replaces the retired logging transport, stop the old service and
run `ratatoskr-instagram-archive repair-logging-outbox --confirm
logging-transport-never-delivered` with the normal database URL. A successful repeat prints `0`;
deploy and start the acknowledged publisher only after that check.

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
