# Design: add-social-source-publishing

## Context

The service stores explicit captures, their public-resolution revisions, and availability observations in `instagram_archive`, and owns a transactional `outbox_events` table that nothing writes yet. The workspace contracts repository publishes the `ratatoskr-social-contracts` crate (capability `social-source-contracts`) with the two payloads this change emits and their envelope rules. No sibling repository consumes a contracts crate today. Knowledge has no social analysis family yet; its plan defers that to separate changesets, and its architecture document still names an event (`social.source.upserted.v1`) that the published contract does not define.

Gate status at definition time: the contracts gate passes. The Knowledge gate fails on both branches — no social analysis family exists and no request/linkage interface is agreed anywhere — so the Knowledge-dependent half of plan item 5 is defined here as a draft agreement and is not implemented by this change.

## Goals / Non-Goals

Goals:

- one truthful publication path for preserved sources, built from stored state only;
- first productive use of the published social contracts by a second repository;
- a concrete, reviewable draft of the Instagram-to-Knowledge interface so the missing cross-repo agreements can be ratified.

Non-Goals:

- implementing any Knowledge-side behaviour, transport subscription, or analysis family;
- representing unavailable-only captures or local removals in events under the current contract vocabulary;
- Data Export provenance events (plan item 8 territory) beyond keeping the snapshot builder open to them;
- any broker topology decision beyond "the outbox hands over durable facts"; NATS subjects are a deployment concern recorded in configuration.

## Decisions

### D1: Publish at first resolution outcome, never at intake

An accepted-but-unresolved capture has unknown upstream availability, no author, and no provider timestamp. The published snapshot requires an author and a closed-vocabulary availability value with no `unknown` state, so publishing at intake would force fabrication. The captured fact is therefore emitted when the first supported public resolution succeeds; the unavailable fallback emits nothing (spec requirement: unavailable outcome publishes nothing).

Alternative rejected: emitting a bare captured fact at intake with placeholder identity. That would publish fabricated authorship and contradict the authority model.

### D2: Identity is derived, not stored

`social_source_id` is a deterministic UUIDv5 computed over `(owner user_ref, platform token, canonical permalink)`. It needs no schema column, survives re-resolutions, differs across owners of the same URL, and matches the contract's rule that identity is Ratatoskr's own, never derived from the provider id alone. The namespace UUID lives beside the code that computes it and is fixed for v1.

Alternatives rejected: reusing `capture_id` (unstable across a future import merge of the same source) and adding a column (unnecessary state while development status forbids migration churn).

### D3: Consume the contracts crate as a pinned git dependency

Instagram adds `ratatoskr-social-contracts` (which brings `event-envelope` and `identifiers`) as a cargo git dependency on `ratatoskr-workspace/repos/contracts`, locked to a commit recorded in `Cargo.lock`. This sets the workspace's first cross-repo crate precedent: versioned through the lockfile, upgradable deliberately, and honest about the source repository.

Alternatives rejected: path dependencies (break outside the monorepo checkout), vendoring (duplicate contract truth), waiting for a registry (no publishing infrastructure exists).

### D4: Snapshot construction is a pure mapping from stored rows

A single builder reads capture, media, latest revision, latest observation, and note-presence rows and produces either a snapshot or a typed refusal. Every field maps from storage: acquisition from `client_source`, authority verbatim, text/media/author from the newest revision, `raw_blob` pointing at the revision's content-addressed blob, digest recomputed from normalized content, completeness decided by the media policy with warnings when metadata-only. Unknown tokens fail construction (D1 rationale applies per field). Notes are read only to confirm existence for product data elsewhere and never enter snapshots.

### D5: The outbox stays the only emission point; delivery order follows aggregate commit order

Facts are appended inside the triggering transaction (resolution success, revision append, observation append). A single publisher loop claims unpublished rows ordered by creation, delivers through a configured transport seam, and marks them published exactly once. At-least-once redelivery plus state-carried payloads makes consumer replay converge; no ordering guarantees beyond per-aggregate sequence are promised.

Alternative rejected: emitting directly from handlers — loses atomicity with the state change and duplicates the durability problem the outbox table was created to own.

### D6: Draft agreement for the Knowledge half (not implemented here)

This is the deliverable the failed gate demands. It defines what must be ratified before plan item 5's Knowledge tasks can start:

1. **Request flow.** Instagram publishes preservation facts; Knowledge consumes them. There is no command channel from Instagram into Knowledge. An analysis request IS the captured/updated fact itself, correlated by `correlation_id` on the envelope. This keeps producers dumb and matches the article-analysis precedent where runs are created from supplied evidence, not pulled.
2. **Linkage key.** Results link back through `social_source_id` plus the snapshot's `content_digest`: a changed digest means a new analysis run may supersede, an unchanged digest is idempotent. Capture records stay linked by their existing ids; no Knowledge identifier is written into Instagram tables.
3. **Result return path.** Knowledge publishes its completion facts (`knowledge.analysis.*`); Instagram consumes them through its inbox purely as observational linkage, never as authority over captures. Until that event contract is published, no result-linkage task can be implemented.
4. **Deletion propagation.** The published vocabulary has no fact for "the user removed this from Ratatoskr" — `deleted_upstream` means the provider removed it, which is a different fact. Privacy deletion therefore requires a new contract element (proposed name `social.source.removed.v1`, payload carrying `social_source_id`, owner, and reason class) plus a tombstone table in this schema. Both belong to a `ratatoskr-workspace` contracts changeset and a schema-bearing change here; neither starts until ratified.
5. **Unavailable-capture representation.** Closing the gap recorded in the spec requires either an optional author or an author-unknown marker in the snapshot contract — again a contracts-repo changeset, not a local workaround.

Ratification path, in order: a contracts-repository changeset covering items 4–5 (and any vocabulary consequences), a Knowledge changeset creating the social analysis family against those contracts, then a workspace-store spec citing both. Only after all three exist do the gated tasks below become implementable.

## Risks / Trade-offs

[Derived identity collides with future import semantics] → uuidv5 input includes owner and exact permalink; if Data Export later proves the same source under a different permalink form, reconciliation is a deliberate change with its own tests, not silent rewriting.

[Contracts crate evolves independently] → Cargo.lock pins the commit; upgrades are explicit commits that rerun fixture conformance tests against the new revision.

[Publisher falls behind or fails repeatedly] → unpublished-row depth and failure counters surface on `/metrics`; redelivery is safe by construction, so lag degrades freshness, never correctness.

[Draft agreement drifts from what Knowledge eventually ratifies] → the draft is marked non-normative; the gated tasks cite the future store spec, not this document, once it exists.

## Migration Plan

No schema change and no rollout coordination: the capability activates with the service. Rollback is disabling the publisher via configuration; outbox rows accumulate harmlessly and drain on re-enable. Because consumers do not exist yet, first activation cannot break anyone.

## Open Questions

None deferred. Everything the specs need is decided above; everything Knowledge-shaped is explicitly gated behind ratification rather than left open.
