# Proposal: add-social-source-publishing

## Why

Captures and their public resolutions are persisted inside `instagram_archive` but invisible to the rest of Ratatoskr: nothing publishes a normalized social-source fact, so `ratatoskr-knowledge` can never analyse a preserved item and the archive remains a private silo. This is implementation plan item 5, and it is the first producer-side consumer of the workspace-published social contracts.

## What Changes

- Publish `social.source.captured.v1` when a capture's source is first preserved and `social.source.updated.v1` when its normalized record changes (successful resolution, later re-resolution, observed availability change). Both events travel in the common envelope and carry the whole snapshot, exactly as the published `social-source-contracts` capability of `ratatoskr-workspace/repos/contracts` defines.
- Build snapshots from stored state only: acquisition method and saved authority come verbatim from the capture record (`explicit_user_capture` stays the ceiling for share-style flows), media travels by BlobRef, completeness is declared with warnings, upstream availability comes from availability observations. No field may claim native Saved membership.
- Make the existing transactional `outbox_events` table the publication path so delivery is at-least-once and replay converges on identical payloads.
- Align repository documentation to the published event names; the planned-but-stale `social.source.upserted.v1` name in `README.md`, `AGENTS.md`, and `docs/` is replaced by the real `captured` / `updated` pair.
- Record a draft interface agreement for the Knowledge half of plan item 5 (analysis requests for preserved items, result linkage back to capture records, deletion propagation on tombstoning) in the change design. That agreement is not implemented here: the published contract vocabulary has no local-removal fact, Knowledge has no social analysis family yet, and cross-repo behaviour must be ratified through a `ratatoskr-workspace` changeset plus a Knowledge changeset before any code consumes it.

## Capabilities

### New Capabilities

- `social-source-publishing`: emission of `social.source.captured.v1` / `social.source.updated.v1` events for explicit captures and their public resolutions, snapshot construction from stored provenance, and idempotent publication through the transactional outbox.

### Modified Capabilities

None. Capture intake and public resolution requirements are unchanged; publication is new behaviour owned by the new capability.

## Impact

- `crates/instagram-archive`: new publishing module that builds snapshots from stored rows and appends outbox rows inside the transactions that already write capture, media, revision, and observation state; wired into the capture and resolution flows.
- `services/instagram-archive`: configuration and runtime for the outbox publisher loop; operator metrics for published/duplicate/failed deliveries.
- Dependencies: first cross-repo consumption of `ratatoskr-social-contracts` (and transitively `event-envelope`, `identifiers`) from `ratatoskr-workspace/repos/contracts`; the consumption mechanism is decided in the change design because no sibling repo consumes a contracts crate today.
- Schema: none. `outbox_events` already exists and admits `aggregate_type = 'capture' | 'media'`. Tombstone storage is deliberately out of scope until the deletion-propagation agreement is ratified.
- Downstream: `ratatoskr-knowledge` becomes the intended consumer once its social analysis family lands (Knowledge plan item 9, separate changeset).
