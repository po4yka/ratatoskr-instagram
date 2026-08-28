## Context

See `proposal.md` for motivation. Items 1–8 already provide explicit captures, supported public resolution with inline content-addressed raw revisions, transactional SocialSource outbox facts and Knowledge completion links, encrypted official connections with resumable own-media authority, and immutable Data Export receipts in the protected content-addressed archive root. The current `capture_tombstones` path marks a capture but deliberately retains its content; it is therefore not a privacy-erasure implementation.

The workspace `social-analysis-intake` contract defines `social.source.removed.v1` as the request for Knowledge to stop analysing and erase derived state. The producer can prove only its local SQL erasure and outbox state. The `blob-references` contract leaves physical bytes owned by this service, so PostgreSQL cannot make filesystem deletion part of the same commit.

The binding development rule permits only an in-place edit to the one `schema.sql`; it forbids database migration files/tooling and parallel versions. “Migration” here means operational parser-version reprocessing of retained Data Export evidence, not a database or legacy-monolith migration.

## Goals / Non-Goals

**Goals:**

- Make reference-only the default and require one explicit finite lease before provider media bytes can be fetched.
- Make capture and connection privacy deletion complete by construction against the owned schema and BlobStore inventory.
- Commit SQL erasure, content-free audit, local resurrection guard, removal outbox facts, and exact blob-deletion work atomically; report physical blob convergence separately.
- Refresh only recent eligible captures under durable finite run/provider budgets with a second admission check immediately before I/O.
- Build one pure parser/reconciliation plan shared by dry-run and restartable apply.
- Preserve acquisition, authority, tenant isolation, immutable evidence history, and supported-public-surface boundaries everywhere content remains retained.

**Non-Goals:**

- Provider-side delete/unsave, publishing, account automation, cookie/session use, private scraping, or hidden consumer APIs.
- Automatic archival of every referenced image/video, permanent retention promises, or treating user uploads as provider-fetched content.
- Database migrations, schema ledgers, later majors, negotiation, dual reads/writes, or compatibility shims.
- Legacy monolith import, another database/repository scan, or fleet cutover work.
- Direct writes into Knowledge or a claim that committed/published producer evidence proves consumer deletion.

## Decisions

### D1. A pure policy decision precedes every provider media-byte fetch

Add a media-retention module with a closed decision `MetadataOnly(reason)` or `Archive(lease)`. The input states acquisition/provenance, explicit owner action, rights class, sanitized HTTPS source metadata, expected media type and size, URL lifetime, object limit, owner storage usage/ceiling, and current time. The immutable lease fixes the response-byte maximum and retention deadline. Unknown rights/type/size/lifetime or exhausted budget yields metadata-only before transport or store access.

An admitted fetch uses the existing Reqwest/Rustls posture with no unrestricted redirects, finite deadline, streaming ceiling, final URL/type/length verification, and SHA-256. A private temporary file is published content-addressably only after full verification; then one short transaction attaches the typed `BlobRef`, digest, size, provenance, policy decision, and deadline. Provider URLs remain observations, never BlobRefs. User-uploaded artifacts keep separate provenance and do not pass through the provider-media transition.

Alternative: download first and decide retention later. Rejected because it consumes rights and storage budgets before authorization and creates undeclared retained bytes.

### D2. Blob erasure is a durable convergence phase outside the SQL transaction

Extend the existing Instagram-owned content-addressed store with exact inventory and `delete_if_matches`/absence-verification operations. The deletion transaction detaches live references and inserts one idempotent blob task carrying only the owner-service `BlobRef`, digest, length, media class, and operation id. A worker performs a fresh database-wide live-reference check, refuses any still-referenced object, deletes only the exact regular file beneath the configured private root, verifies absence, and records a bounded content-free outcome.

The privacy operation has separate `local_state` and `blob_state`. SQL erasure is atomic with audit/outbox/task creation; the overall operation becomes complete only when every required blob is verified absent or explicitly classified retained-shared. Filesystem failure leaves retryable durable work and cannot roll SQL content back into visibility.

Alternative: delete files while holding the database transaction. Rejected because filesystem effects cannot roll back and database locks/connections must not be held during external I/O.

### D3. One closed inventory powers both capture and connection planning

Define `OwnedDataClass` as the exact `instagram_archive` relation inventory plus protected BlobStore classes. Define total classification maps for `Capture` and `AccountConnection`. A schema-inventory test compares all three sets one-to-one, including the new lifecycle relations; an unclassified, duplicate, or unknown class fails before mutation.

The pure planner reads an owner-bound snapshot and produces stable-order row actions, source holding decisions, raw/media/blob decisions, source removals, and bounded per-class counts. Preview only renders this plan. Apply takes an owner-scoped stable operation id, locks the target and related source identities in stable order, recomputes the plan, and commits SQL effects in one transaction. Completed-operation replay returns stored counts.

Capture deletion removes notes, availability/resolution history, analysis linkages, capture row, exclusive media revisions/raw records/normalized media, and media-byte references. Shared content is detached or retained after explicit same-owner and cross-owner reference checks. Connection deletion additionally erases credential ciphertext, live OAuth material, capability/permission observations, provider budgets/audit content allowed by policy, own-media run/staging/authority/checkpoints, profiles, and exclusive official media. Same-owner explicit-capture or export observations survive without provenance changes; another owner is never a candidate.

Alternative: rely on foreign-key cascade. Rejected because cascade cannot enumerate completeness, decide shared-lane retention, generate one final-holding removal fact, or produce an auditable preview.

### D4. Replace retention tombstones with content-free resurrection guards

The existing `capture_tombstones` relation retains a foreign key to the content-bearing capture and therefore cannot represent physical privacy deletion. Under the no-compatibility development rule, replace that path with `local_source_removals`, keyed by owner/source plus operation and containing only removal reason/time. Update late-completion and publication checks to consult this guard.

For each source whose final owner holding disappears, the apply transaction calls the existing publishing boundary to append one canonical `SocialSourceRemoved(UserRequested)` fact, then removes content-bearing rows. A remaining holding suppresses the removal. The audit stores typed operation target kind, terminal phase, per-class counts, and safe failure classes only—never IDs that reveal provider content, URLs, notes, bodies, credentials, or private paths beyond the required internal operation/owner/target UUIDs.

Alternative: call Knowledge synchronously. Rejected because it crosses ownership, couples privacy completion to consumer availability, and loses transactional outbox/replay guarantees.

### D5. Re-resolution is select, claim, reserve, execute, finalize

Add `reresolution_runs` and `reresolution_items`, plus `next_resolution_at` policy state associated with live captures. Selection orders due captures within the configured recency window by `(next_resolution_at, capture_id)` and persists admitted/skipped classification. Only resolved, temporarily unavailable, and resolution-failed captures are automatic candidates; private, deleted, unsupported, permanent-unavailable, locally removed, not-due, and old captures are skipped.

Immediately before I/O, a short transaction rechecks owner/live/status/recency/deadline, reserves one item and request from non-zero finite run item/request/byte/duration/concurrency budgets, and consumes the existing supported-public endpoint allowance. No connection is held while the approved surface runs. Streaming uses the reserved response limit. Finalization charges accepted bytes and routes the result through the existing append-only resolution/publishing boundary. Equal normalized content records unchanged without another update fact; changed content publishes the full current snapshot; failures preserve the last good projection.

Next deadlines use deterministic capture-id jitter to avoid synchronized refresh waves. Scheduling stays disabled unless all finite budgets are configured.

Alternative: reuse the official own-media scheduler. Rejected because public captures are a separate authority lane and need different eligibility, resolver, and budget evidence.

### D6. Dry-run and apply share a pure `ReprocessPlan`

Refactor Data Export processing into verified receipt access, safe inventory read, exact registry lookup by `(detected_layout, parser_id)`, pure ordered parsing/classification, owner-current-state snapshot, and pure reconciliation planning. The plan contains stable item keys, classifications, warnings/conflicts, completeness sets/counts, prospective normalized/source digests, and canonical plan/state fingerprints. Fingerprints exclude timestamps and operation ids.

Dry-run stops after report rendering and opens no write transaction or mutable storage/network boundary. Apply creates owner-scoped `export_reprocessing_runs/items` state and commits deterministic bounded chunks; every item checkpoint, projection effect, and outbox append shares one transaction. Resume verifies receipt digest/length, parser identity, plan fingerprint, and owner-state preconditions before continuing. Completed replay returns the stored report. Old import runs, reports, parser ids, raw evidence, and projections remain addressable; parser omission never schedules deletion.

Alternative: mutate `import_runs.parser_id` and restart the initial worker. Rejected because that rewrites evidence, destroys dry-run/apply comparability, and cannot resume independently.

### D7. The existing process gains explicit operator reprocessing modes

Extend the dependency-free command grammar before normal service startup:

```text
ratatoskr-instagram-archive reprocess-export dry-run --owner <UUID> --run-id <UUID> --parser <TOKEN>
ratatoskr-instagram-archive reprocess-export apply --owner <UUID> --run-id <UUID> --parser <TOKEN> --operation-id <UUID>
```

Arguments are closed and duplicate/unknown tokens are usage errors. Database/blob roots come only from validated `RATATOSKR__` configuration. Success writes one newline-terminated canonical JSON report to stdout; diagnostics stay on stderr. Exit `0` means a report or completed idempotent replay, `2` invalid invocation, `78` invalid configuration, and `1` operational/integrity failure. Dry-run never prompts; apply requires its explicit mode and operation id. No new dependency or HTTP mutation endpoint is added.

Alternative: add an unauthenticated admin route. Rejected because the current admin plane is read-only/loopback and does not carry owner mutation authority.

### D8. Lifecycle schema is a first-version replacement, not a migration

Edit `schema.sql` in place to add constrained media policy/reference state; `deletion_operations`, `deletion_effects`, `local_source_removals`, `blob_deletion_tasks`, `reresolution_runs`, `reresolution_items`, `export_reprocessing_runs`, and `export_reprocessing_items`; and due-policy fields. Replace the old tombstone path and update every caller directly. `Database::apply_schema` continues to build fresh disposable databases from the embedded file.

Do not add `migrations/`, SQLx migrate features, schema/data backfill, a ledger, later major, dual path, or compatibility code. The deployment portion of `rust-database` migration guidance is inapplicable; its transaction ownership, finite pool, stable lock order, no-external-I/O-in-transaction, and real PostgreSQL test rules remain binding.

## Risks / Trade-offs

- [Deletion inventory drifts when schema changes] → Compare the exact catalog-derived relation list and BlobStore classes with both target maps in a mandatory test; refuse unknown runtime classes.
- [Shared source/raw/blob evidence is erased prematurely] → Plan owner-scoped holdings explicitly and repeat global live-reference checks before SQL garbage collection and physical blob deletion.
- [SQL deletion commits while a blob remains] → Keep an idempotent pending task, report blob convergence separately, retry to verified absence, and never claim full completion early.
- [Removal outbox is delayed] → Retain the committed fact and local resurrection guard; distinguish committed, published, and Knowledge-consumed evidence.
- [Deletion races re-resolution or import apply] → Stable lock order plus an immediate live-removal/precondition check before I/O and before chunk commit.
- [Run budgets change after selection] → Selection grants no I/O authority; claim transaction reserves every finite budget immediately before request.
- [Large runs monopolize connections] → Bounded chunks and short claim/finalize transactions; no pool connection during HTTP, filesystem, decompression, parsing, or delay.
- [Dry-run goes stale before apply] → Include state/plan fingerprints; apply recomputes and refuses changed preconditions rather than promising stale fidelity.
- [A deterministic parser is consistently wrong] → Retain immutable archive and all prior evidence, require exact parser selection, and allow a later explicit operation using a corrected/prior registered parser.
- [Synthetic fixtures miss real provider variants] → State the verification gap; never claim real-export compatibility until an authorized fixture is proven separately.

## Migration Plan

1. Add RED/GREEN schema and complete-inventory tests, then replace the current first-version definition in place; disposable databases are recreated from it.
2. Add pure media, deletion, eligibility/budget, and reprocessing planners test-first before side-effecting workers.
3. Wire transactional stores/outbox/audit, exact blob task worker, bounded re-resolution runtime, and explicit CLI modes.
4. Keep new schedulers and media-byte archival disabled until validated non-zero finite policy/budget configuration is supplied. Reference-only behavior remains the default.
5. Validate producer behavior against pinned `social.source.removed.v1`, workspace `social-analysis-intake`, and `blob-references`; no coordinated contract rollout is required, but producer evidence never substitutes for consumer proof.
6. Roll back by disabling new workers/CLI apply and reverting code plus the one schema definition for freshly recreated development databases. There is no down migration or compatibility path. Already committed privacy deletion and published removal facts are irreversible; content-free audit remains.
7. Parser rollback is another explicit reprocessing operation with a supported prior/corrected parser; it never rewrites or deletes previous run evidence.

Privacy impact: capture/connection content, credentials, notes, media, raw revisions, import-derived links, and analysis linkage become enumerable and erasable. Retained audit/outbox/guard records are content-free. User-visible authority remains unchanged across explicit capture, official own media, public observations, and Data Export.
