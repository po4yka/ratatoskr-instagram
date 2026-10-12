## Context

The contract is XR-021 CONTRACTS.md R5-09 and is not redefined here. This document records the decisions internal to this repository.

## Decisions

### The digest preimage is a typed struct, rendered by the contracts' canonical JSON

`snapshot_content_digest(&SocialSourceSnapshot)` serializes a private struct whose fields are, in this order, `domain` (`instagram.snapshot-content`), `permalink`, `external_post_id`, `caption`, `upstream` and `media`, with `ratatoskr_identifiers::canonical_json` (pretty-printed, two-space indent, one trailing newline, declaration order). The test pins the exact bytes of one golden case, so a change to the contracts' rendering fails loudly instead of silently re-keying every analysis. `caption` is the snapshot's `text`, so an empty stored caption and a missing one both render `null`: the digest describes what the snapshot states, not how the row spells it. `upstream` collapses the three `UpstreamAvailability` values to `available`, `deleted` and `unavailable`. `media` lists the snapshot's media items; the lane archives none, so it is the empty array.

### The observation time of an updated fact

A deletion observed upstream does not append a `media_revisions` row (`record_failed_resolution` updates the projection and appends an availability observation), so `media_revisions.resolved_at` alone would leave a deletion fact with the time of the revision before it, and Knowledge would break the tie by event id only. Both write paths set `media.updated_at` to the observation instant, so an updated fact takes the newest of `captures.captured_at`, the current revision's `resolved_at` and `media.updated_at`. When the current revision was just resolved, the three agree with R5-09 rule 2 (the new revision's `resolved_at`); when a deletion was just observed, the deletion instant wins. A captured fact keeps `captures.captured_at`.

### The completion linkage recomputes the current digest

`link_matching_capture` finds the capture that matches the source identity first, builds that capture's current snapshot from stored rows (the same `build_snapshot` the publisher uses) and compares digests. A completion for an older digest therefore skips, which is the honest answer: the analysis it reports describes content that is no longer the capture's current state. The link row stores the digest that matched.

### Unchanged re-resolution keeps today's behaviour

`resolve_capture_permalink` publishes a fact only when the media type or caption changed or no fact exists yet. This change keeps that: an unchanged re-resolution publishes nothing.
