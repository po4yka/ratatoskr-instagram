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
- `own_media_sync_state` keeps the committed stable media-id watermark and next due time;
  `own_media_sync_runs` keeps one active resumable traversal and its closed outcome;
  `own_media_sync_items` stages complete candidate generations linked to raw JSON evidence; and
  `own_media_authority` points at the one visible completed generation.
- normalized own-media `media` / immutable `media_revisions` rows use `official_api` and
  `authoritative_platform_state`; media rows distinguish reference-only, stored, expired, and
  deletion-pending state with content hash, byte length, and retention deadline where applicable.
- `captures`, canonical URLs, acquisition, saved authority, captured time, notes/collection references.
- resolved posts/reels, authors, media, relations, upstream status, resolution attempts.
- `export_snapshots` owns immutable owner/digest receipts and the archive BlobRef;
  `import_runs` plus `import_run_transitions` enforce `received -> inspected -> parsed -> reconciled`
  or terminal `failed`; `export_records` retains deterministic normalized, unknown, warning, and
  conflict staging evidence; `export_completeness_reports` stores sorted exact gap sets/counts and
  the non-authority disclaimer.
- `deletion_operations` / `deletion_effects` retain content-free owner audit; local source removals
  prevent resurrection; `blob_deletion_tasks` converge exact digest/length deletion after all live
  references disappear.
- `reresolution_runs` / `reresolution_items` retain bounded job state and skip/outcome classes;
  `export_reprocessing_runs` / `export_reprocessing_items` retain owner-scoped parser plan/state
  fingerprints, checkpoints, omissions, and replay state.
- write audits if supported, outbox/inbox.

## Constraints

Owner scope is mandatory. Provider IDs/canonical URLs are unique within relevant account/context. Raw blobs are immutable/content-addressed. Acquisition and authority are required and cannot be silently upgraded. Missing records do not become deletion without explicit evidence. Credentials/private media are excluded from events/logs. Cross-schema writes/foreign keys are forbidden.

Capability and credential replacement share one transaction after provider I/O. Revocation deletes
credentials and live owner flows while retaining a redacted audit and a total revoked projection.
Own-media pages commit staging/cursor progress separately, but normalized revisions, outbox facts,
authority pointer, and watermark change in one completion transaction after capability revalidation.
Data Export reconciliation likewise commits revisions, idempotent outbox facts, the completeness
report, and its terminal transition together. A missing export identity never updates captures,
local removals, upstream availability, or stronger official/explicit provenance. Privacy deletion
commits owned SQL erasure, local removal guards, content-free audit/effects, blob work, and canonical
Knowledge deletion requests atomically; BlobStore I/O follows after commit. Archive bytes live only
under the configured protected blob root; referenced media remains metadata inside that ZIP.
