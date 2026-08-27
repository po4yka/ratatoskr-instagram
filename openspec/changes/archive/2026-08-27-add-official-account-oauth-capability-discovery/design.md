## Context

See `proposal.md` for motivation and the delta specs for observable behavior. The repository already has disposable-schema `accounts` and `credentials` tables, a strict optional configuration loader, a loopback product listener, SQLx test-database support, and a static acquisition-mode capability model. It has no OAuth flow state, credential cryptography, official provider client, per-account capability projection, credential audit, or provider-call ledger.

The design is constrained by Platform ADR-0012: Platform may receive the public callback, but the Instagram service generates and validates provider state, claims an audience-bound single-use authorization code, exchanges it, and alone stores the resulting token. Development status forbids migrations and later major versions. The official Meta collection currently distinguishes Instagram Login from Facebook Login; Instagram Login serves professional accounts without a linked Facebook Page and uses the `instagram_business_*` permission family.

## Goals / Non-Goals

**Goals:**

- Make one official professional-account connection executable end to end inside this bounded context, with provider I/O replaceable by deterministic fixtures.
- Make stored credentials cryptographically opaque to a database reader and bind each envelope to its account, token kind, and key version.
- Turn raw account-type and permission observations into a total per-account matrix that later code must consult.
- Count every provider attempt before I/O so retries and crash-interrupted calls cannot disappear from accounting.
- Keep existing no-OAuth deployments bootable while making an enabled but incomplete OAuth configuration fail closed.

**Non-Goals:**

- Facebook Login/Page-linked account support, personal-account access, native Saved access, own-media reads, webhooks, publishing, comments, messages, insights, or any provider write other than a documented best-effort token revoke.
- A new cross-repository callback contract. The service implements ADR-0012's Instagram side; adding `instagram` to Platform's provider registry and deployment routing/grants remains a separate workspace rollout.
- Operational bulk key rotation. The envelope and keyring select by version now; a future operator tool may re-encrypt rows without changing their shape.

## Decisions

### D1. Start with Instagram API with Instagram Login

The first provider profile is Meta's Instagram API with Instagram Login, against `graph.instagram.com`, for business and creator accounts. Authorization requests only `instagram_business_basic`; the adapter does not request content-publish, comments, messages, or insights permissions in this item. The Graph API version is explicit validated configuration and is always present in paths, so a provider default cannot silently change behavior.

The production adapter will use the current documented token exchange/refresh, account identity, and permission-inspection endpoints verified against Meta's official documentation during implementation. Endpoint base hosts are fixed production constants; tests inject a loopback base URL under test support only. Facebook Login was rejected for the first profile because it adds Page linkage, Page tokens, and extra permissions without improving item 6's read-only connection goal. Supporting it later is a second explicit provider profile, not branches hidden inside this one.

### D2. Keep the Platform relay boundary and expose loopback command routes

The product listener gains loopback service routes to begin a connection, complete a relay, refresh capabilities/tokens, and revoke. Begin receives the already-authenticated `user_ref`; complete receives a relay identifier, not an authorization code. A narrow `OAuthCodeRelay` port claims the code exactly once from Platform, while tests use a fake. No code enters request logs, database rows, events, or durable command payloads.

Flow rows store `SHA-256(state)`, owner, exact redirect binding hash, an encrypted PKCE verifier when the provider supports PKCE, expiry, and consumption time. The raw state is returned only in the authorization URL. Completion atomically claims an unexpired owner-bound flow before exchange; provider failure leaves it consumed so a code/state pair is never replayed. Direct public callbacks on the provider service were rejected because they would bypass the accepted relay architecture and require a new exposed trust boundary.

### D3. Use the fleet AES-256-GCM envelope pattern with stronger associated-data binding

Pinned `aes-gcm` and cryptographic randomness produce envelopes formatted as envelope-format byte, key-version `u32`, fresh 96-bit nonce, ciphertext, and 128-bit authentication tag. AES-256-GCM associated data contains a stable domain separator plus account/flow id and token kind, so moving a valid access-token envelope to another row or interpreting it as a refresh token fails authentication. Equal plaintext therefore produces unrelated stored bytes.

Configuration carries a redacted keyring (`version -> base64 32-byte key`) and one current write version. Decryption accepts only a version present in the keyring; encryption always uses the current version. When OAuth is disabled the keyring may be absent. Enabling OAuth requires a valid non-empty keyring, current version, client id/secret, redirect URI, Platform relay configuration, Graph version, timeouts, and finite budgets. Invalid secret configuration names rules but never values.

`pgcrypto` was rejected because it places decryption authority in the database. Rolling a custom cipher was rejected in favor of the fleet's maintained AEAD dependency. Reusing the fleet pattern without associated data was rejected because row substitution is cheap to prevent here.

### D4. Edit the current schema in place and normalize observations

`schema.sql` changes, with no migration files:

- `credentials`: one active row per account; access and optional refresh envelopes, encryption key version, granted permission set, expiry, and rotation timestamps.
- `oauth_flows`: flow id, owner, state hash, redirect hash, optional encrypted PKCE verifier, key version, expiry, and consumed time. Authorization codes are never stored.
- `account_permission_observations`: one complete discovery generation per account, provider permission name/status, observation time, and protected raw-record reference.
- `account_capabilities`: one row for every closed capability in the latest generation, with `available`/`unavailable`/`not_supported`, a closed reason, and observation time.
- `account_credential_audit`: append-only redacted lifecycle entries for authorized, refreshed, reauthorization-required, and revoked transitions.
- `provider_api_usage`: operation id/kind, optional account, request class, attempt ordinal, `started`/terminal outcome, optional HTTP status and bounded provider-usage percentages, timestamps, and no URL or payload.

Existing `accounts.scopes` becomes the provider-observed permission set rather than the requested set. Account identity/type discovery responses are preserved through the existing `raw_records(record_kind = 'api_response')` path under the same protected access rules; normalized projections reference that evidence. Capability reconciliation and account/credential writes share one transaction after provider I/O.

Keeping permissions only as a serialized string was rejected because it cannot represent granted, declined, expired, and absent observations independently. Keeping only available capabilities was rejected because absence would again be ambiguous.

### D5. Reconcile one total closed per-account matrix

The pure reconciliation function takes one `AccountObservation`: provider account id, observed account type, complete permission-status map, explicit write-consent flags, and timestamp. It produces every closed `AccountCapability` exactly once:

- account identity read and own-media read require a professional account plus granted `instagram_business_basic`;
- content publish, comment management, and message management additionally require their exact granted permission and explicit external-write consent, which is false in item 6;
- native Saved-list read is always `not_supported`;
- any personal/unknown account type or absent/declined/expired/unknown permission produces an explicit unavailable reason.

Persistence replaces the entire prior generation for that account in one transaction. Callers load the projection rather than independently interpreting scopes. `AcquisitionMode::OwnAccountSync` remains `Planned`; an available `own_media_read` account capability only says the provider authorizes a future item 7 operation.

### D6. Put all provider HTTP behind a narrow, bounded adapter

An `InstagramProvider` port exposes code exchange, token refresh, account discovery, permission discovery, and optional revoke. The Reqwest/Rustls adapter has fixed HTTPS production hosts, bearer headers rather than token query strings where the endpoint permits, bounded connect/read/total timeouts, response byte caps, strict JSON models, and typed failure classes. Synthetic JSON fixtures and a loopback scripted server test request paths, headers, response classification, limits, and redaction; no normal test uses a real Instagram account.

Token exchange is never automatically retried because the authorization code is single-use. Idempotent discovery GETs may retry only transient network, 429, and 5xx responses within both the small retry count and shared call budget, respecting a bounded `Retry-After`. Authentication and validation failures never retry. Refresh follows the provider's documented token strategy and also re-runs both discovery calls before committing.

### D7. Reserve durable usage before every network attempt

Each top-level operation gets an `operation_id` and an in-memory remaining-attempt counter initialized from finite configuration. Admission inserts a unique `provider_api_usage` row in `started` state and commits it before the HTTP request begins; only then may the adapter send bytes. Completion updates that row with a closed outcome and redacted response metadata. A crash can therefore leave `started`, which truthfully means an attempt may have reached the provider, but cannot erase its budget cost.

Retries request another admission and another ordinal. Exhaustion returns `ProviderBudgetExhausted` before transport invocation. This local operation budget complements Meta response usage headers; bounded numeric percentages from documented headers may be stored, while raw headers are not.

An after-the-fact counter was rejected because process death between I/O and insert undercounts provider usage. A global rate limiter alone was rejected because spacing does not bound the number of requests one operation can generate.

### D8. Refresh and revoke prioritize truthful local state

Successful refresh seals new material first in memory, performs discovery under the same operation budget, then transactionally replaces credentials, permission generation, capability generation, account status, and audit data. An authentication/revocation response marks `reauthorization_required` and replaces every capability with unavailable state. Transient failures preserve the prior connection and return a retryable typed error rather than inventing downgrade evidence.

Revocation decrypts the current access token into a secret wrapper, marks the account `revoking`, makes one documented provider revoke attempt if supported, and then always commits local scrub: delete credential and live owner flow rows, set `revoked`, replace capabilities with unavailable/revoked reasons, and append the redacted audit result. A startup/retry sweep treats any stranded `revoking` account as scrub-required, so a crash cannot leave it indefinitely usable. Local scrub is authoritative even when provider revoke is unavailable; this trades remote certainty for the user's explicit demand that Ratatoskr retain no recoverable token.

### D9. Keep module and test seams narrow

The library adds separate `account`, `oauth`, `credentials`, `provider`, `capability_reconciliation`, and `provider_budget` modules. Time, randomness, relay claiming, and provider transport are injected at seams used by tests, while production constructors supply OS randomness, UTC time, Platform relay access, and Reqwest. Public errors carry stable classes only; secret wrappers implement redacted `Debug` and no `Display` exposing bytes.

Every behavior task follows a red/green pair. Database tests use fresh schemas and directly inspect raw rows for plaintext and revoke completeness. Provider tests use checked-in synthetic fixtures. The final gate is the exact command list in `DEVELOPMENT.md`, through `build-gate` for compiler-backed work, followed by strict OpenSpec validation and archived validation after sync/archive.

## Risks / Trade-offs

- [Meta changes a Graph version, scope, or token endpoint] → Pin the Graph version in validated configuration, verify current official documentation during implementation, classify drift explicitly, and require an intentional version update rather than using an unversioned default.
- [Platform does not yet accept an Instagram callback relay] → Land this service disabled by default; a separate workspace change adds the provider registry, `oauth.claim.instagram` grant, edge/deployment routing, and compatible rollout before enabling it.
- [Best-effort provider revoke fails after the user asks to disconnect] → Always scrub local material and record the redacted upstream outcome; never retain the token merely to retry remote cleanup.
- [A process crashes after reserving provider usage but before receiving a response] → Keep the durable `started` attempt as consumed and expose it as an indeterminate outcome rather than undercounting.
- [Keyring configuration increases operator complexity] → Validate the whole ring at startup when OAuth is enabled, redact it from effective config, and include deterministic rotation-selection tests.
- [New cryptography and HTTP crates widen supply-chain surface] → Pin exact versions, inspect the candidate lockfile and upstream metadata/code, and require cargo-deny advisories, bans, licenses, and sources gates before acceptance.

## Migration Plan

There is no database migration: development status requires editing `schema.sql` in place and creating fresh test/development databases from it. The feature ships disabled by default. Deployment order is: merge the Instagram implementation; land the separate Platform callback/provider/grant routing change; provision the Meta app, redirect, keyring, service credential, Graph version, and finite budgets; validate configuration; then enable the routes. Rollback disables OAuth routing/configuration and reverts the feature while the development database may be recreated from the prior schema.
