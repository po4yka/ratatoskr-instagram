# Instagram connector data model

## Planned owned schema: `instagram_archive.*`

- `accounts`, encrypted credentials, scopes, capabilities, expiry/status.
- own-media records/revisions and safe raw blob references.
- `captures`, canonical URLs, acquisition, saved authority, captured time, notes/collection references.
- resolved posts/reels, authors, media, relations, upstream status, resolution attempts.
- `exports`, schema/parser version, archive hash/blob, import runs, warnings, unknown records.
- write audits if supported, outbox/inbox.

## Constraints

Owner scope is mandatory. Provider IDs/canonical URLs are unique within relevant account/context. Raw blobs are immutable/content-addressed. Acquisition and authority are required and cannot be silently upgraded. Missing records do not become deletion without explicit evidence. Credentials/private media are excluded from events/logs. Cross-schema writes/foreign keys are forbidden.
