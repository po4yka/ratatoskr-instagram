# Design: add-social-source-publishing

## Context

The service stores explicit captures, their public-resolution revisions, and availability observations in `instagram_archive`, and owns a transactional `outbox_events` table that nothing writes yet. The workspace contracts repository publishes the `ratatoskr-social-contracts` crate (capability `social-source-contracts`) with the two payloads this change emits and their envelope rules. No sibling repository consumes a contracts crate today. Knowledge has no social analysis family yet; its plan defers that to separate changesets, and its architecture document still names an event (`social.source.upserted.v1`) that the published contract does not define.

Gate status: the contracts gate passes and now includes `social.source.removed.v1` plus typed `knowledge.analysis.completed.v1`. The workspace social-analysis intake change defines captured/updated facts as the request flow and the completion payload as the return interface. Knowledge implementation remains outside this repository.

## Goals / Non-Goals

Goals:

- one truthful publication path for preserved sources, built from stored state only;
- first productive use of the published social contracts by a second repository;
- an idempotent Instagram-to-Knowledge linkage and local-deletion interface using those published contracts.

Non-Goals:

- implementing any Knowledge-side behaviour, transport subscription, or analysis family;
- representing unavailable-only captures without a normalized snapshot;
- Data Export provenance events (plan item 8 territory) beyond keeping the snapshot builder open to them;
- any broker topology decision beyond "the outbox hands over durable facts"; NATS subjects are a deployment concern recorded in configuration.

## Decisions

### D1: Publish at first resolution outcome, never at intake

An accepted-but-unresolved capture has unknown upstream availability, no normalized content, and no provider timestamp. Publishing at intake would force a snapshot the archive cannot prove. The captured fact is therefore emitted when the first supported public resolution succeeds; the unavailable fallback emits nothing (spec requirement: unavailable outcome publishes nothing).

Alternative rejected: emitting a bare captured fact at intake with placeholder identity. That would publish fabricated authorship and contradict the authority model.

### D2: Identity is derived, not stored

`social_source_id` is a deterministic UUIDv5 computed over `(owner user_ref, platform token, canonical permalink)`. It needs no schema column, survives re-resolutions, differs across owners of the same URL, and matches the contract's rule that identity is Ratatoskr's own, never derived from the provider id alone. The namespace UUID lives beside the code that computes it and is fixed for v1.

Alternatives rejected: reusing `capture_id` (unstable across a future import merge of the same source) and adding a column (unnecessary state while development status forbids migration churn).

### D3: Consume the contracts crate as a pinned git dependency

Instagram adds `ratatoskr-social-contracts` (which brings `event-envelope` and `identifiers`) as a cargo git dependency on `ratatoskr-workspace/repos/contracts`, locked to a commit recorded in `Cargo.lock`. This sets the workspace's first cross-repo crate precedent: versioned through the lockfile, upgradable deliberately, and honest about the source repository.

Alternatives rejected: path dependencies (break outside the monorepo checkout), vendoring (duplicate contract truth), waiting for a registry (no publishing infrastructure exists).

### D4: Snapshot construction is a pure mapping from stored rows

A single builder reads capture, media, latest revision, and availability state and produces either a snapshot or a typed refusal. Every field maps from storage: acquisition from `client_source`, authority verbatim, text from the newest revision, `raw_blob` pointing at the revision's content-addressed blob, digest over the emitted normalized shape, completeness decided by the media policy with warnings when metadata-only. Author and publication time remain absent until an approved surface actually observes them. Unknown tokens fail construction (D1 rationale applies per field). Notes never enter snapshots.

### D5: The outbox stays the only emission point; delivery order follows aggregate commit order

Facts are appended inside the triggering transaction (resolution success, revision append, observation append). A single publisher loop claims unpublished rows ordered by creation, delivers through a configured transport seam, and marks them published exactly once. At-least-once redelivery plus state-carried payloads makes consumer replay converge; no ordering guarantees beyond per-aggregate sequence are promised.

Alternative rejected: emitting directly from handlers — loses atomicity with the state change and duplicates the durability problem the outbox table was created to own.

### D6: Ratified Knowledge request, completion, and removal boundary

The published contracts and workspace social-analysis intake change define the boundary:

1. **Request flow.** Instagram publishes preservation facts; Knowledge consumes them. There is no command channel from Instagram into Knowledge. An analysis request IS the captured/updated fact itself, correlated by `correlation_id` on the envelope. This keeps producers dumb and matches the article-analysis precedent where runs are created from supplied evidence, not pulled.
2. **Linkage key.** Results link back through `social_source_id` plus the snapshot's `content_digest`: a changed digest means a new analysis run may supersede, an unchanged digest is idempotent. Capture records stay linked by their existing ids; no Knowledge identifier is written into Instagram tables.
3. **Result return path.** Knowledge publishes `knowledge.analysis.completed.v1`; Instagram consumes it through `inbox_events` and persists only `(capture_id, content_digest, completed_at)`. It neither receives an analysis payload nor lets Knowledge mutate capture authority.
4. **Deletion propagation.** A local tombstone atomically writes `social.source.removed.v1`, carrying the stable source identity, owner, removal reason and removal time. This is explicitly distinct from `deleted_upstream`; a late completion is skipped so it cannot re-link or resurrect the tombstoned record.
5. **Unavailable-capture representation.** The existing snapshot permits no truthful complete record for an unavailable-only capture. The service continues to emit nothing rather than manufacture a source.

## Risks / Trade-offs

[Derived identity collides with future import semantics] → uuidv5 input includes owner and exact permalink; if Data Export later proves the same source under a different permalink form, reconciliation is a deliberate change with its own tests, not silent rewriting.

[Contracts crate evolves independently] → Cargo.lock pins the commit; upgrades are explicit commits that rerun fixture conformance tests against the new revision.

[Publisher falls behind or fails repeatedly] → unpublished-row depth and failure counters surface on `/metrics`; redelivery is safe by construction, so lag degrades freshness, never correctness.

[Knowledge result handling is not live proof] → this repository verifies contract conformance, outbox/inbox semantics and local linkage only; Knowledge deployment and indexing are validated in its own gate.

## Migration Plan

The current development schema is applied in place to new test and development databases; no migration is introduced. Rollback disables the publisher/consumer loop while durable outbox and inbox rows remain intact. Downstream removal handling is idempotent by source identity.

## Open Questions

None deferred. Everything the specs need is decided above; everything Knowledge-shaped is explicitly gated behind ratification rather than left open.
