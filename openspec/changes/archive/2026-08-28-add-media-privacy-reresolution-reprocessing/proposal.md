## Why

Plan item 9 is the remaining lifecycle gap between accepted Instagram evidence and a privacy-safe, maintainable archive: media references have no executable retention policy, owners cannot comprehensively erase a capture or account connection, stale public observations have no bounded refresh job, and retained exports cannot be previewed and reprocessed when parser support improves.

## What Changes

- Add an explicit media-retention policy that keeps provider media reference-only by default and admits stored bytes only within validated rights, type, size, lifetime, and owner-storage budgets.
- Add owner-authorized deletion by capture and by official account connection, backed by a closed enumeration of every affected owned relation and blob class. Deletion atomically removes local records, appends content-free audit evidence, schedules reference-safe blob erasure, and emits one canonical source-removal fact for each final owner holding so Knowledge can delete derived state.
- Add scheduled public re-resolution for recent eligible captures, with durable run state and finite item, request, response-byte, deadline, concurrency, and provider-budget guards checked immediately before I/O.
- Add restartable parser-version reprocessing over retained immutable Data Export archives. Dry-run and apply share one deterministic plan so their classifications, counts, warnings, conflicts, completeness evidence, and prospective/applied digests match when source state is unchanged.
- Edit the current first-version `schema.sql` in place. Parser-version migration means archive reprocessing only; this change adds no database migration file, migration ledger/tooling, later major version, negotiation, or compatibility path.
- Update runtime wiring, bounded telemetry, documentation, and synthetic/redacted tests for the new lifecycle guarantees. Legacy monolith import remains outside this change.

## Capabilities

### New Capabilities

- `media-retention`: Reference-only defaults, bounded media-byte admission, truthful completeness, expiry, and reference-safe BlobStore deletion.
- `privacy-deletion`: Owner-scoped capture and account-connection deletion, complete owned-data/blob enumeration, content-free audit evidence, replay safety, and downstream removal propagation.
- `re-resolution-jobs`: Eligible recent-capture selection and finite per-run/provider budgets for supported public re-resolution.
- `data-export-reprocessing`: Exact parser-version dry-run fidelity, immutable receipt reuse, checkpointed apply/resume, and replay-safe reporting.

### Modified Capabilities

- `archive-schema`: The current first-version schema gains constrained lifecycle policy, deletion/audit, re-resolution, and reprocessing state in place.
- `public-resolution`: Later resolution becomes an explicitly budgeted scheduled operation while preserving append-only evidence and truthful unavailable semantics.
- `data-export-import`: An immutable export receipt can be explicitly previewed and reprocessed by a selected supported parser version without reacquisition or silent reinterpretation.
- `social-source-publishing`: Final local privacy deletion appends the canonical removal fact atomically and does not claim upstream deletion or downstream consumption.
- `official-account-connection`: Owner-authorized connection deletion extends credential revocation into complete removal of connection-derived holdings while preserving independent capture/export lanes.

## Impact

- Affected Rust areas: `crates/instagram-archive` media, deletion, resolution, Data Export, account, publishing, BlobStore, persistence, and telemetry boundaries; `services/instagram-archive` scheduled runtime and an explicit operator reprocessing command.
- Affected storage: the single in-place `schema.sql`, Instagram-owned content-addressed blobs, transactional outbox, and content-free lifecycle audit/checkpoint records. No new production dependency is planned.
- Cross-repository behavior uses the already pinned `social.source.removed.v1`, workspace `social-analysis-intake`, and `blob-references` contracts. The service writes no Knowledge-owned state and makes no new contract claim.
- Official account, explicit capture, public resolution, and Data Export authority remain separate. Another owner or a same-owner independent holding prevents shared source/blob erasure. No provider write, native unsave, private-session access, cookie use, or hidden API is introduced.
