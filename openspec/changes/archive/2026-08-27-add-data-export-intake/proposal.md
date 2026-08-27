## Why

Instagram plan items 1–7 preserve explicit captures and supported own-account observations, but the service cannot yet receive the most complete user-controlled Instagram acquisition artifact: an official Data Export. Accepting that archive without an immutable raw-first boundary, hostile-container limits, parser evidence, and explicit coverage math would risk both unsafe extraction and false native-Saved or deletion claims.

## What Changes

- Add an authenticated, owner-bound ZIP upload on the existing loopback product plane. Authenticate before reading body bytes, stream the body through a finite receipt budget and SHA-256 digest, and publish the exact archive into Instagram-owned content-addressed storage represented by the workspace `BlobRef` shape.
- Persist one owner-and-digest-scoped import with the durable ordered states `received -> inspected -> parsed -> reconciled`, plus a typed terminal failure state. A retry reuses its immutable receipt and resumes or returns durable evidence instead of replacing bytes or duplicating projections.
- Add a hostile-archive inspector/extractor that rejects traversal and absolute paths, backslash ambiguity, duplicate file names, unsupported entry types, excessive nesting, entry-count overflow, compressed/decompressed byte overflow, and excessive compression ratio. Extraction remains confined to a service-created private root and counts bytes actually emitted.
- Add the first explicitly versioned Instagram-export parser over a synthetic/redacted fixture derived from the accepted export shape. It deterministically produces normalized provider records with `data_export` acquisition and `export_observation` authority while retaining unknown sections/records as raw evidence and warnings.
- Reconcile parsed records idempotently by stable provider identity and canonical permalink, preserving existing captures and publication state when an export omits them. Publish normalized SocialSource facts through the existing transactional outbox without claiming native Saved membership.
- Persist and return an owner-scoped completeness report that lists and counts matched identities, export-only gaps, capture-only comparable gaps, non-comparable captures, parsed/unknown categories, and warnings. The report explicitly describes one archive observation and never silently fills a gap or treats absence as unsave, deletion, or complete account-history proof.
- Mark Data Export supported only after receipt, inspection, parsing, reconciliation, reporting, hostile-fixture, determinism, and PostgreSQL-backed tests are green; document exact parser scope and remaining real-export verification limits.
- Keep export requesting, browser/session automation, private API access, and media-byte processing beyond immutable archive/reference storage out of scope.

## Capabilities

### New Capabilities

- `data-export-import`: Authenticated raw-first Instagram Data Export receipt, bounded ZIP inspection/extraction, deterministic versioned parsing, idempotent reconciliation, and truthful completeness reporting.

### Modified Capabilities

- `archive-schema`: Extend the editable first-version schema in place with owner-scoped immutable receipt identity, the ordered import states, parser/raw-record evidence, normalized export records, and gap-report persistence.
- `capability-model`: Move the Data Export acquisition lane from planned to supported while preserving its `export_observation` authority ceiling and the native-Saved non-capability.
- `social-source-publishing`: Publish normalized Data Export observations idempotently through the existing SocialSource/outbox contract with full provenance and without interpreting export absence as removal.

## Impact

- Affected Rust areas: `crates/instagram-archive` receipt/authentication, BlobRef storage, archive safety, parser registry, reconciliation, database, capability, publishing, telemetry, and test support; `services/instagram-archive` product routing/configuration and HTTP integration tests.
- Affected schema: the current `schema.sql` only; no migration file, migration tooling, API major, or parallel parser-major route is introduced.
- Expected new production dependencies require owner approval before apply: one pinned maintained ZIP reader/decompressor with narrowly selected features, plus a temporary-directory library only if the standard library cannot provide the required private lifecycle safely. This avoids a custom container/decompression implementation but adds archive-format attack surface that must remain dependency-audited.
- Shared boundaries: reuse the accepted workspace `blob-references`, `operation-progress`, and SocialSource contracts. A detailed report returned directly to the authenticated owner remains Instagram-owned; publishing a new structured Platform operation-summary payload or changing an Export Agent API would require a separate `ratatoskr-workspace` changeset before integration.
- Fixtures are synthetic and redacted. Because no legacy Instagram-export parser or real owner export fixture was found in the available checkout/history, this change cannot claim broad compatibility with current live Instagram exports until the supported fixture is compared with an owner-supplied redacted archive through a separate evidence-admission step.
