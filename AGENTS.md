# Ratatoskr Instagram Agent Instructions

## Scope

These instructions apply to the `ratatoskr-instagram` repository.

This repository owns Instagram-specific account integration, explicit user captures, public-content resolution, and versioned Instagram Data Export imports.

## Repository mission

The service has two deliberately separate ingestion lanes:

1. **Official account lane** for the capabilities exposed by the supported Instagram API, primarily professional account data and explicitly authorized account operations.
2. **Explicit capture lane** for posts/reels the user deliberately sends to Ratatoskr through mobile share targets, the browser extension, or another explicit client action.

The service must never blur those lanes or claim authority the provider does not expose.

## Current phase

The repository is in architecture bootstrap. Do not assume Rust crates, OAuth flows, oEmbed resolution, import parsers, a database schema, or CI commands exist unless they are present in the checkout.

When creating initial implementation:

- model acquisition method and saved-state authority first;
- keep official API, public resolution, explicit capture, and Data Export adapters separate;
- preserve raw evidence before normalization;
- do not add browser-session scraping as a shortcut.

### Development status

Ratatoskr is in development. No database holds data that has to survive a schema change. While this
status holds, these rules are binding, and they override anything else in this repository that
plans otherwise, including the rest of this file:

- **One version only.** The API, the database, and the contracts keep their first version. Do not
  add a `v2` or a later major version, and do not add version negotiation, deprecation windows, or
  parallel-major routing.
- **No database migrations.** Do not add a migration file, and do not add migration tooling. A
  schema change edits the current schema definition in place, and a test database is created from
  that definition.
- **The product is `Ratatoskr`.** It is not "Ratatoskr Next". Do not write that name in code,
  documentation, identifiers, comments, or commit messages.

Only the repository owner changes this status. Ask before you write anything these rules forbid.

## How a change starts

Every non-trivial change begins as an OpenSpec change rather than as an edit, and each assistant
starts one in its own syntax. Claude Code has the command: `/opsx:propose <what you want to build>`,
or `/opsx:explore` first when the shape is not clear yet. Codex has no project-level command and
triggers the same skill by name, `$openspec-propose`, or lets its description match it. OpenCode has
its own command, `/opsx-propose`. Whichever starts it, the result is `openspec/changes/<id>/` holding
a proposal, the spec deltas, a design and a task list, and you read that plan before any code is
written. `/opsx:apply`, `$openspec-apply-change` or `/opsx-apply` builds it, and `/opsx:archive`,
`$openspec-archive-change` or `/opsx-archive` folds the deltas into `openspec/specs/`.

`openspec/specs/` holds the behaviour that is true today, and it starts empty on purpose. A spec here
grows from a change that needed it. Do NOT convert `docs/REQUIREMENTS.md`, `docs/INTERFACES.md`,
`docs/DOMAIN.md` or `docs/DATA_MODEL.md` into specs in bulk. Those documents stay where they are, as
material an exploration reads. A spec set produced by bulk conversion is large, stale on the day it
lands, and trusted by nobody.

Behaviour that more than one repository can see — the shape of a contract, the meaning of a field, the
order in which repositories must receive a change — belongs in the `ratatoskr-workspace` store, not
here. `openspec/config.yaml` references it, so `openspec instructions` in this repository lists the
store's specs with the exact command that fetches one. Cite that spec from a local proposal instead
of restating it.

### Tests come first

The task list carries one pair per behaviour. The first task adds a test that fails. The second makes
it pass. Never one task that does both.

- Run the new test before you write the implementation, and confirm it fails for the reason the task
  states — not for a compile error or a typo.
- A refactor task comes after the tests are green. It adds no test and changes no behaviour.
- A task that cannot start from a failing test says why in one line. Configuration, documentation and
  generated files are the usual reasons.
- Do not tick a task whose test has not been run.

Nothing can check the order in which the two were written. What CI does check is
`openspec validate --archived`, which fails when a change was archived with a task left unticked, and
the step in `fleet.yml` that fails when a repository holds a manifest and a `ci.yml` that never runs
a test. `ratatoskr-workspace/docs/QUALITY_GATES.md` states that limit rather than implying it is
covered.

## The Rust skill catalogue

`.agents/skills/` holds eighteen Rust skills, and `.claude/skills/` symlinks to them, so all three
assistants read one copy. Codex reads `.agents/skills/`, Claude Code reads `.claude/skills/`, and
OpenCode scans both, so the existing symlink already covers it and nothing belongs under
`.opencode/skills/`. Each is a reference sheet rather than a tutorial: the commands, flags,
thresholds and triage tables for one Rust concern. Your assistant reads the descriptions and opens a
skill only when the task matches one, so the set costs almost nothing until it is needed.

`rust-tdd` is the Rust form of the task pair above. `rust-lints` owns `clippy.toml`, which is where
this repository's size limits live. `rust-security` answers a `RUSTSEC` advisory.
`rust-async-internals` covers `tokio::select!` cancel safety and shutdown. `rust-database` covers
pool budgets and transaction ownership. `rust-compiler-errors` is the entry point when the build
fails and the cause is not obvious.

`rust-database` also carries a section on deploying migrations in compatible phases. The Development
status above overrides it: while that status holds, this product has no migrations at all. Read the
rest of that skill and skip that section.

The eighteen are identical in every Ratatoskr repository whose stack is Rust, and
`ratatoskr-workspace/.github/workflows/drift.yml` fails when one copy stops matching the others. Do
not edit a file under `.agents/skills/`. A correction belongs upstream in `po4yka/rust-skills` and
reaches this repository through `npx skills update`.

The catalogue holds forty-four skills and eighteen are vendored here.
`ratatoskr-workspace/docs/QUALITY_GATES.md` records which were left out and why. They are vendored
under BSD-3-Clause, (c) 2026 Nikita Pochaev, who also owns this repository; each `SKILL.md` keeps its
`license` field, and the full text is in that repository's `LICENSE`.

## Sources of truth

Use this order:

1. active task/changeset and accepted ADRs;
2. `README.md`;
3. social/event contracts from `ratatoskr-contracts`;
4. explicit user capture records or complete import evidence;
5. official provider responses and safe redacted fixtures;
6. implementation details.

When provider capability is absent or uncertain, store `unknown`/partial state. Do not invent a native Saved API or background synchronization guarantee.

## Hard bounded-context rules

### Instagram service owns

- Instagram account linkage and encrypted credentials for supported official flows;
- provider account/media identity and normalized own-account content;
- explicit Instagram capture intake and provider-specific resolution;
- acquisition method and saved-authority classification;
- public oEmbed/provider metadata observations;
- Instagram Data Export import runs, parser versions, raw archive references, and projections;
- upstream availability state for Instagram sources;
- Instagram-specific outbox/inbox records;
- references to Knowledge analysis and client collections.

### Instagram service does not own

- Ratatoskr user sessions or device credentials;
- local collections/tags;
- LLM analysis, embeddings, or search ranking;
- generic article extraction;
- native Saved-list authority that the official API does not provide;
- user passwords, browser cookies, or hidden consumer-session tokens;
- Telegram/mobile/browser-extension interaction state;
- unrelated provider credentials.

## Acquisition and authority semantics

Every stored source must record how it was obtained and what that proves.

Representative acquisition methods:

```text
OfficialApi
ShareExtension
BrowserExtension
DataExport
LegacyImport
```

Representative saved authority:

```text
ExplicitUserCapture
ExportObservation
Unknown
```

Rules:

- An explicit Ratatoskr capture proves that the user saved the item **to Ratatoskr** at `captured_at`.
- It does not prove membership in Instagram's native Saved list.
- Do not expose `native_saved=true` unless an authoritative supported source actually establishes it.
- Do not infer native unsave when a Ratatoskr capture is deleted.
- Preserve provider publication time separately from user capture time.
- Preserve raw/normalized provenance through downstream events.

Names and UI contracts must remain honest about these semantics.

## Official account lane

The official account lane may implement only capabilities available to the supported account/API configuration, such as:

- professional account identity;
- own media catalog and metadata;
- captions/comments where authorized;
- publishing or messaging only through separate explicit permissions and reviewed product scope.

Rules:

- request minimum scopes;
- record granted scopes and account capability state;
- keep read and external-write consent separate;
- do not assume a personal account has professional API capabilities;
- do not expand the service into a general Instagram automation product;
- reconcile provider objects by stable external IDs;
- treat usernames, captions, URLs, and display attributes as mutable;
- publish normalized social-source events rather than leaking provider SDK types.

## OAuth and credential handling

- Validate OAuth `state`, callback-user binding, redirect URI, and any PKCE/nonce requirements of the selected flow.
- Store access/refresh tokens encrypted and versioned.
- Record provider account ID, granted scopes, expiry/refresh, reauthorization, and revocation state.
- Never send provider tokens to Platform, Knowledge, clients, Telegram, events, or logs.
- Audit permission changes and external provider writes.
- Detect capability/scope downgrade and transition the connection to an explicit degraded/reauth state.

Do not store passwords or MFA secrets.

## Explicit capture lane

A capture is accepted only from an authenticated Ratatoskr client action with:

- internal user/device identity;
- canonical or original Instagram URL;
- capture timestamp;
- client source, such as iOS Share Extension, Android Share Target, browser extension, or Telegram forwarding;
- idempotency key;
- optional note and local collection references carried as separate platform/client data;
- operation/correlation metadata.

Rules:

- normalize and classify the URL without executing page scripts;
- deduplicate repeated client delivery idempotently;
- preserve original URL and capture metadata;
- resolve only through supported public/provider mechanisms;
- do not fetch the user's browser cookies or authenticated page state;
- return partial/unavailable status when content cannot be resolved;
- keep the capture record even when provider content later becomes unavailable, subject to policy.

## Public resolution and oEmbed

For eligible public posts/reels, a provider-supported public resolver/oEmbed path may supply:

- canonical URL;
- provider post/media identity when available;
- author/display metadata;
- caption/text exposed by the endpoint;
- embed representation/thumbnail metadata;
- observed availability.

Rules:

- treat oEmbed/public metadata as an observation, not a native Saved record;
- validate response size, type, schema, redirects, and URLs;
- cache according to documented staleness/revalidation policy;
- preserve raw response/blob reference when policy permits;
- sanitize embed HTML before any rendering and never use it as executable server content;
- do not expose provider access tokens embedded in requests/logs;
- do not parse undocumented private page data when the official resolver is insufficient.

## Private and unavailable content

- Do not bypass privacy, authentication, age, region, or access controls.
- Do not ask clients/extensions to exfiltrate cookies or hidden API responses.
- Preserve canonical URL, explicit capture timestamp, optional user note, and `content_unavailable`/equivalent state.
- A user may explicitly attach a screenshot or file they possess; store it as a separate user-uploaded artifact with its own provenance and access policy.
- Do not represent a screenshot as provider-fetched canonical content.
- Distinguish deleted, private/inaccessible, temporarily unavailable, malformed, unsupported, and resolution-failed states when evidence allows.

## Media handling

Media metadata and media-byte archival are separate capabilities.

- Do not automatically download all media merely because a capture exists.
- Define an explicit policy for URLs, expiry, rights, storage budget, and completeness.
- Validate MIME, size, dimensions/duration, redirects, and content hashes.
- Route generic file storage through approved BlobStore interfaces.
- Preserve provider metadata and user-uploaded media as different sources.
- Never execute media payloads or trust filenames.

A metadata-only capture must not be reported as a complete media backup.

## Data Export imports

Instagram Data Export archives are untrusted, versioned inputs.

Import pipeline requirements:

1. compute archive hash and store the raw archive immutably;
2. enforce archive path, file count, decompressed size, nesting, and zip-bomb limits;
3. detect provider/schema/export version;
4. parse into staging state;
5. preserve unknown sections as raw blobs/references when safe;
6. normalize only known structures;
7. produce counts, warnings, conflicts, and completeness report;
8. reconcile idempotently without deleting records because a category is absent;
9. retain parser version and import-run evidence.

Do not promise that every export contains native Saved items. Inspect the actual archive and report category availability honestly.

Do not execute HTML, scripts, media helpers, or archive contents.

## Identity and deduplication

Deduplicate with explicit precedence:

- stable provider media/post ID when available;
- canonical URL observation;
- content hash as supporting evidence, not necessarily identity;
- capture/import provenance.

Rules:

- multiple captures may reference one provider source while retaining individual user intent/timestamps if the product requires it;
- do not collapse distinct carousel/reel/post objects solely by caption similarity;
- provider URL changes do not create a new source when stable identity proves continuity;
- ambiguous imports remain separate with a recorded conflict rather than destructive merging.

## Downstream integration

Publish normalized `SocialSource`-compatible events with:

- platform and external ID;
- canonical URL;
- acquisition method;
- saved authority;
- author/publication/capture timestamps;
- text/media metadata;
- raw blob/reference and content hash where applicable;
- upstream availability;
- operation/correlation IDs.

`ratatoskr-knowledge` owns analysis and embeddings. Platform/clients own local collections and presentation. Generic linked articles are delegated to Extractor.

## Persistence and schema evolution

Instagram writes only its owned schema.

Conceptual data includes:

```text
instagram_accounts
instagram_credentials
instagram_media
instagram_captures
instagram_capture_resolutions
instagram_export_runs
instagram_export_records
instagram_raw_objects
instagram_tombstones
instagram_outbox
instagram_inbox
```

Rules:

- no cross-schema writes or foreign keys;
- raw archives/responses are separated from normalized projections;
- uniqueness and idempotency constraints reflect provider and capture identities;
- schema changes preserve acquisition/authority provenance;
- absence in one export or failed resolution never causes unproven deletion;
- secrets and large blobs use protected storage/reference mechanisms.

## Commands and events

Representative messages include:

```text
instagram.capture.requested.v1
instagram.capture.resolved.v1
instagram.account.sync_requested.v1
instagram.account.media_updated.v1
instagram.export.ingested.v1
social.source.upserted.v1
social.source.unavailable.v1
social.connection.reauth_required.v1
```

Use canonical contracts, transactional outbox, inbox deduplication, correlation/causation IDs, and at-least-once-safe handlers.

Do not publish authoritative native-saved events from explicit captures.

## Prohibited implementation approaches

Do not add:

- server-side Playwright/Chromium login to Instagram;
- storage or replay of user passwords/MFA secrets;
- browser cookie/session exfiltration;
- hidden/private consumer API reverse engineering as the supported path;
- stealth/anti-bot bypass;
- background crawling of the user's account without an official authorized API;
- misleading UI/data fields that call an explicit capture a native Saved mirror.

An experimental local connector, if ever approved by ADR, is a separate security/product scope and must not weaken these default rules.

## Security and privacy

- Provider credentials remain encrypted inside this service.
- Apply internal-user ownership to account and capture access.
- Treat captions, URLs, embed HTML, archives, media, and filenames as malicious input.
- Sanitize rendered output and prohibit script execution.
- Do not log private content, raw archives, access tokens, or user notes by default.
- Limit raw export/blob access and retention.
- Redact provider diagnostics before user display.
- Record explicit consent for provider writes and user uploads.
- Use least-privilege database, network, and BlobStore access.

## Observability

Required telemetry should cover:

- account connection/reauth state without token values;
- captures accepted, deduplicated, resolved, unavailable, and failed;
- acquisition method and resolution strategy;
- public resolver latency/failure classes;
- Data Export archive/import counts, warnings, unknown sections, and completeness;
- media metadata/archive status;
- outbox/inbox lag and duplicates;
- correlation, account, source, capture, and import-run IDs in non-sensitive form.

Avoid usernames, captions, and full URLs as ordinary metric labels.

## Testing expectations

When implementation exists, include applicable tests for:

- OAuth callback/state/scope/refresh/revoke behavior;
- token encryption and redaction;
- URL normalization and provider-ID extraction;
- capture idempotency and provenance;
- explicit-capture versus native-saved authority semantics;
- public resolver/oEmbed schema, sanitization, cache, and unavailable responses;
- private content refusal paths;
- user-uploaded artifact separation;
- hostile Data Export archives, version detection, unknown sections, and restartable import;
- absence-in-export never causing deletion;
- deduplication conflicts;
- events/outbox/inbox replay;
- schema initialization preserving provenance.

Use synthetic/redacted fixtures. Never depend on a live personal Instagram account in normal tests.

## Cross-repository change rules

Use a workspace changeset when changing:

- social/event contracts;
- capture API used by Platform, mobile, browser extension, web, or Telegram;
- analysis inputs used by Knowledge;
- linked-article extraction requests;
- OAuth/callback/scopes;
- BlobStore/media contracts;
- Data Export completeness semantics;
- deployment secrets or schema/import cutover behavior.

List producer/consumer compatibility, rollout, rollback, privacy, reprocessing/reindexing, and user-visible authority impact.

## Git and PR workflow

- State which lane is affected: official account, explicit capture, public resolution, media, or Data Export.
- Keep authority/provenance changes separate from unrelated refactors.
- Include safe fixtures and import/resolution tests.
- Document new scopes, external writes, raw data, and retention impact.
- Do not add server-side login/cookie scraping.
- Do not commit credentials, personal exports, private media, or real user notes.
- Do not claim native Saved synchronization without an authoritative supported contract.
- Update README/ADRs when provider capability or product semantics change.

## Completion criteria

A task is complete only when:

- responsibility belongs to the Instagram bounded context;
- official account and explicit capture lanes remain separate;
- acquisition method and saved authority are explicit and truthful;
- no browser-session/password/cookie automation is introduced;
- private/unavailable content is handled without bypass;
- Data Export processing is raw-first, safe, versioned, idempotent, and completeness-aware;
- normalized events preserve provenance;
- media completeness is reported honestly;
- relevant security/import/resolution tests pass;
- contracts, schema, telemetry, and cross-repository rollout are documented.
