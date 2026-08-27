# Instagram connector data model

## Owned schema: `instagram_archive.*`

- `accounts` stores owner, stable provider account ID, mutable username, observed account type,
  actual granted scopes, and connected/degraded/revoking/revoked state.
- `credentials` has at most one row per account and stores versioned authenticated access/refresh
  token envelopes, actual permissions, expiry, and rotation time—never plaintext.
- `oauth_flows` stores owner, hashes of state and redirect binding, optional encrypted PKCE verifier,
  expiry, and one-time consumption; no authorization code is stored.
- `account_permission_observations` and `account_capabilities` hold one raw-linked, account-scoped,
  total latest generation.
- `account_credential_audit` stores closed lifecycle outcomes and bounded JSON detail.
- `provider_api_usage` reserves every attempt before I/O and stores ordinal, closed request/outcome,
  optional status, bounded Meta usage percentages, and timestamps with no payload columns.
- own-media records/revisions and safe raw blob references.
- `captures`, canonical URLs, acquisition, saved authority, captured time, notes/collection references.
- resolved posts/reels, authors, media, relations, upstream status, resolution attempts.
- `exports`, schema/parser version, archive hash/blob, import runs, warnings, unknown records.
- write audits if supported, outbox/inbox.

## Constraints

Owner scope is mandatory. Provider IDs/canonical URLs are unique within relevant account/context. Raw blobs are immutable/content-addressed. Acquisition and authority are required and cannot be silently upgraded. Missing records do not become deletion without explicit evidence. Credentials/private media are excluded from events/logs. Cross-schema writes/foreign keys are forbidden.

Capability and credential replacement share one transaction after provider I/O. Revocation deletes
credentials and live owner flows while retaining a redacted audit and a total revoked projection.
