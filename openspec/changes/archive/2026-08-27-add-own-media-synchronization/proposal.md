## Why

Plan item 7 is the first operation that exercises the official account lane after OAuth: accounts with an observed `own_media_read` capability still have no scheduled way to preserve their own media, while unsupported accounts have no durable truthful sync outcome. Adding the lane now exceeds the monolith where the supported provider surface allows it without broadening access to other users' content.

## What Changes

- Add scheduled, bounded own-media scans for connected accounts whose current capability generation makes `own_media_read` available.
- Stage paginated metadata and provider media references under a durable run, advance the account watermark only after a successful incremental traversal, and atomically replace the visible own-media authority generation at completion.
- Preserve raw provider evidence and metadata through service-owned `BlobRef` records under the workspace `blob-references` contract; store references and integrity metadata, not downloaded media bytes.
- Record unsupported account types, missing or downgraded permissions, reauthorization state, and other capability limits as explicit no-op run outcomes that perform no provider media request and leave the prior watermark and authority unchanged.
- Publish completed own-media additions and changes through the existing normalized SocialSource outbox with `official_api` acquisition and without any native-Saved assertion.
- Add synthetic business/creator media fixtures and test-first coverage for watermark advancement, failed-run retention, atomic authority swap, truthful capability no-op, strict provider parsing, and metadata-only BlobRef completeness.
- Keep stories and other ephemeral content out of the requested field set, and never read or download another account's media.

## Capabilities

### New Capabilities

- `own-media-sync`: Scheduled, checkpointed, capability-aware synchronization of the connected account's supported own-media metadata and references.

### Modified Capabilities

- `capability-model`: Move `OwnAccountSync` from planned to supported while retaining the per-account `own_media_read` gate and truthful authority ceiling.
- `official-account-connection`: Require own-media work to use the current owner-bound credential and total capability generation, including downgrade and reauthorization no-op behavior.
- `social-source-publishing`: Publish normalized own-media facts from an atomically completed official-account generation without treating them as explicit captures or native Saved records.

## Impact

- Affects the official account lane in `crates/instagram-archive`, provider fixtures, the service scheduler loop, telemetry, the in-place first-version `schema.sql`, schema inventory tests, README, capability, architecture, data-model, interface, testing, and development documentation.
- Reuses the existing encrypted credential lifecycle, durable provider-attempt budget, Reqwest/Rustls adapter, current social-source contracts, and workspace `blob-references` contract; no new production dependency or cross-repository contract shape is expected.
- Adds no migration, later API version, provider write, personal-account support, native Saved access, story/ephemeral scope, browser-session access, cookie handling, or download of other accounts' media.
