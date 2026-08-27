## Why

Plan item 6 is the missing security boundary between Ratatoskr and Instagram's supported professional-account APIs: the service cannot yet establish an official connection, protect provider credentials, or tell downstream work which permissions the connected account actually has. Implementing that boundary now lets item 7 consume observed capabilities instead of assuming an account type or scope.

## What Changes

- Add an official Instagram/Meta OAuth lifecycle for supported professional accounts, including state and callback binding, least-privilege scopes, encrypted versioned token storage, expiry/refresh handling, and revoke scrubbing.
- Discover the connected provider account and granted permissions after authorization and refresh, then reconcile them against a closed capability matrix so absent, declined, or lost permissions disable the corresponding capability and can require reauthorization.
- Account for every official provider request against a finite call budget and record redacted usage outcomes without tokens, usernames, or full URLs.
- Add synthetic provider fixtures and test-first coverage for authenticated encryption, OAuth state and token lifecycle, capability reconciliation, API-budget accounting, downgrade handling, and complete revocation.
- Keep own-media synchronization and all provider writes out of scope; this change only establishes the connection and reports what later work may exercise.

## Capabilities

### New Capabilities

- `official-account-connection`: Official professional-account OAuth, encrypted credential lifecycle, provider capability discovery, and bounded provider API accounting.

### Modified Capabilities

- `capability-model`: Reconcile per-account observed type and granted permissions into explicit supported, unavailable, and reauthorization-required capability states without changing the still-planned `OwnAccountSync` lane to supported.

## Impact

- Affects the official account lane in `crates/instagram-archive`, the product service routes/configuration, the current `schema.sql`, telemetry, tests, README, security, interface, and data-model documentation.
- Adds pinned authenticated-encryption, randomness/encoding, and Rustls HTTP-client dependencies following the fleet credential pattern; the lockfile and supply-chain policy gate must cover the resulting graph.
- Integrates with Meta's documented professional-account surfaces only. Personal accounts and native Saved-list synchronization remain unsupported, and no browser session, password, cookie, private endpoint, own-media sync, or external provider write is introduced.
- The public callback handoff and service authorization must remain compatible with Platform ADR-0012. Any Platform provider-registry or edge-routing work needed to expose Instagram's callback is a separate cross-repository rollout change; provider tokens remain solely in this service.
