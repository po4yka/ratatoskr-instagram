## Context

See `proposal.md` for motivation. The current service has one loopback product listener, strict `RATATOSKR__` configuration, an editable single-file PostgreSQL schema, content digests for small inline raw responses, normalized media/capture projections, and a transactional SocialSource outbox. It does not have a large-object store, archive upload authentication, a durable import executor, or Data Export parser code. The existing `export_snapshots` and `import_runs` tables are bootstrap placeholders whose global digest uniqueness and coarse outcome field do not satisfy owner isolation or the required state machine.

The workspace `blob-references` specification requires the owning service to retain content-addressed bytes and publish a structured digest/media-type/length reference. The first supported parser has only synthetic/redacted evidence in this checkout: no legacy Instagram export parser and no owner export fixture was found. Consequently the parser must name a narrow accepted shape and the documentation must keep real-export compatibility unverified.

## Goals / Non-Goals

**Goals:**

- Make an authenticated upload produce durable, owner-scoped immutable evidence before ZIP code sees any byte.
- Make each safety, parser, and reconciliation boundary independently durable and replay-safe.
- Support one narrow saved-post export shape, retain everything else recoverably, and calculate exact deterministic gap sets against local captures.
- Keep hostile ZIP memory, disk, CPU, and path effects finite without implementing compression or ZIP framing ourselves.

**Non-Goals:**

- Provider export requests, hidden/private API calls, browser cookies, media-byte expansion, native Saved synchronization, complete account-history claims, parser migration/reprocessing tooling, or a new Platform/Export Agent result contract.
- Filesystem extraction of the archive tree. The parser needs bounded JSON entry bytes, not provider-controlled directory materialization.

## Decisions

### Receipt uses bearer-to-owner authentication and two private storage roots

Add disabled-by-default `DataExportConfig` under `RATATOSKR__DATA_EXPORT__*`: enabled flag, secret bearer-token-to-UUID mappings, Instagram-owned blob root, receipt staging root, body/entry/decompression/ratio/path limits, worker poll interval, and batch size. Enabling the lane requires non-empty tokens, distinct absolute private roots, and finite non-zero limits. Token values are secret typed and absent from debug output.

`POST /v1/data-exports` authenticates before constructing/polling a body stream and accepts only `application/zip`. It writes one create-new staging file while hashing and counting chunks. On success it creates the digest-addressed final object without overwriting a concurrent winner, re-hashes an existing winner before trusting it, fsyncs file and containing directory, then commits the owner/digest receipt and initial transition. A limit or I/O failure deletes only the request's known staging file. The response is `202 Accepted` for a new receipt and `200 OK` for an owner replay, carrying run id, digest, byte length, and current state; `GET /v1/data-exports/{run_id}` uses the same credential and returns owner-scoped state/report with `Cache-Control: no-store`.

This follows the fleet's bearer receipt convention while keeping the existing loopback product-plane trust boundary. Taking `user_ref` from JSON is rejected because it permits tenant confusion. Buffering the request in memory is rejected because the upload limit is a disk-and-network concern, not a heap budget.

### One polling worker advances compare-and-swap stages

The service starts one bounded Data Export worker only when the lane is enabled. It selects a small stable batch and advances each run by its current state:

```text
received   -> verify raw BlobRef, inspect ZIP, record inventory/version -> inspected
inspected  -> read bounded recognized entries, parse/sort/stage records -> parsed
parsed     -> reconcile projections/outbox/report in one transaction   -> reconciled
```

Every update predicates on the expected current state; stage rows use stable run-derived identities and unique constraints. Thus a crash or a second process may repeat pure inspection/parsing work, but only one state transition and one projection transaction can commit. A typed failure transaction changes only the expected non-terminal state to `failed`, appends its bounded transition evidence, and retains the archive. The transition table provides the exact durable history required by tests; `import_runs.state` provides the current work queue.

Holding a database transaction across ZIP I/O is rejected because it would consume a pool connection and hold locks for attacker-controlled work. Adding a new message contract is rejected because this repository can execute its owned durable queue directly and no cross-repository fact is needed to preserve correctness.

### ZIP inspection is metadata-first and extraction is bounded-to-memory

Use the fleet-aligned pinned `zip = 8.6.0` crate with `default-features = false` and only `deflate`, subject to owner approval before apply. The service first walks the central directory and builds a normalized, sorted inventory while rejecting:

- absolute, parent, empty-component, backslash-ambiguous, NUL-bearing, or over-deep paths;
- duplicate normalized names, encrypted entries, symlinks, and non-file/non-directory entry kinds;
- entry-count, compressed-byte, declared-decompressed-byte, and declared-ratio overflow.

After the whole inventory passes, only recognized JSON entries are opened. A counting reader enforces per-entry and cumulative actual emitted bytes before `serde_json` receives a bounded buffer. No archive path is ever materialized on disk; this removes the zip-slip write primitive while still rejecting malicious names as malformed evidence. Unknown entries remain recoverable through `(archive BlobRef, normalized entry name, declared sizes, CRC/digest when safely read)` records and warnings. This is preferred to extracting a directory tree or writing a ZIP decoder: both create substantially larger security surfaces.

The hostile suite builds ZIPs in memory for traversal/absolute/backslash paths, duplicate names, excessive entries/depth, declared and actual decompressed overflow, high ratios, encrypted/unsupported kinds, malformed central directories, and random byte input. Property tests assert that bounded inspector/parser entry points never panic; a short local fuzz smoke is attempted if the pinned nightly/cargo-fuzz tooling is present, while the deterministic suite remains the merge gate.

### The first parser supports one explicit saved-post JSON shape

The first registry entry is keyed by detected export shape plus parser id; it recognizes exactly one redacted fixture layout rooted at `your_instagram_activity/saved/saved_posts.json`. The grammar reads only the saved-post collection, stable Instagram permalink/shortcode, exported observation timestamp, and bounded display metadata. It does not interpret the observation timestamp as provider publication or Ratatoskr capture time. Other categories, fields, and records become raw references plus warnings rather than heuristic alternate layouts.

Parser output is a pure value: records keyed by stable shortcode/canonical permalink, raw entry evidence, categories, conflicts, and warnings, each sorted by a closed stable key. JSON object and ZIP order cannot affect output. The parser id is recorded on the run and every staged record. Changing this grammar later adds another parser implementation and an explicit reprocessing change; it does not add an API/database major or silently reinterpret old output.

Supporting several guessed provider layouts now is rejected because there is no admitted real fixture to prove them. The synthetic fixture proves behavior and determinism only; owner-fixture admission remains named evidence debt rather than a fabricated live-compatibility claim.

### Reconciliation and completeness commit as one owner-scoped transaction

Parsed staging rows never become visible social authority. The reconciliation transaction:

1. loads the run and owner, requiring state `parsed`;
2. resolves stable shortcodes first and canonical permalinks second, retaining ambiguity as conflicts;
3. upserts immutable revisions/current media with `data_export` and `export_observation` without overwriting stronger official or explicit-capture provenance;
4. appends idempotent captured/updated SocialSource facts using the current accepted contract and the archive `BlobRef`;
5. compares distinct export identities with owner capture identities as sets;
6. stores sorted `matched`, `export_only`, `capture_only`, and `non_comparable_capture` lists plus counts, categories, warnings, and the authority disclaimer;
7. commits the report and `reconciled` transition atomically.

Set cardinality invariants are validated before persistence and by tests after JSON round-trip. Missing export identities never alter captures, tombstones, availability, or prior source facts. A detailed report is returned only from the authenticated Instagram endpoint; publishing that structure through Platform is deferred until a workspace changeset defines a social-archive summary contract.

### The schema is rewritten in place around the actual state machine

Edit `schema.sql` rather than adding a migration. Replace global archive-digest uniqueness with owner/digest uniqueness; keep archive object identity content-addressed. Extend snapshots/runs and add transition, staged export-record, and completeness-report tables with explicit owner/run foreign keys, named closed CHECK constraints, deterministic uniqueness keys, bounded error/warning JSON shapes, and no raw archive body. `raw_records` continues to hold small resolver/API evidence; large archive bytes live only under the configured Instagram blob root.

This avoids treating bootstrap placeholder columns as a compatibility contract. A fresh disposable database is the only supported database shape during development.

## Risks / Trade-offs

- [The ZIP/decompression dependency parses attacker-controlled bytes] → pin the fleet-used 8.6.0 release with only Deflate, run `cargo deny`/advisory gates, keep explicit service-owned limits, property tests, and hostile regressions; do not enable crypto or broad codec features.
- [Disk fills before ZIP inspection] → enforce a streaming compressed-byte cap, require private staging/blob roots, preflight configured free-space policy where supported, fsync before acknowledging, and expose receipt failures without logging paths or digests.
- [A concurrent writer leaves corrupt content under a digest] → create final objects without overwrite and verify an existing winner's length and SHA-256 before recording its BlobRef.
- [Declared ZIP sizes lie] → enforce both metadata totals and actual emitted-byte counters; parsing begins only after full inventory validation.
- [Worker crashes between stages] → make inspection/parsing replayable and stage/projection commits compare-and-swap idempotent; keep the archive and exact current state.
- [Synthetic parser drifts from real Instagram exports] → expose the exact detected/parser identifiers, preserve unknown material, mark compatibility unverified, and admit a redacted owner fixture before claiming current real-export support.
- [Gap identifiers expose private activity] → authenticate every read, scope every query by owner, return not-found across owner boundaries, apply no-store headers, and exclude URLs/identifiers from logs and metric labels.
- [Publishing export observations overwrites stronger evidence] → keep revisions/provenance separate and choose authority through the existing fixed ceiling; reconciliation cannot demote or relabel an official/capture record.

## Migration Plan

1. Obtain repository-owner approval for the pinned `zip 8.6.0` production dependency and its Deflate transitive surface.
2. Add tests and implementation under the disabled lane, edit the current `schema.sql` in place, and recreate disposable development/test databases from that definition; add no migration file or ledger.
3. Configure private roots, bearer mappings, and finite limits, then enable the lane only after the full hostile suite and PostgreSQL-backed gate pass.
4. Rollback disables the upload/worker before reverting code. Immutable owner archives are retained rather than silently deleted; because development databases are disposable, rollback recreates the database from the reverted first-version schema.
5. A future Platform/Export Agent report projection or real-owner fixture admission starts its own workspace/local change and does not alter evidence already recorded by parser id.
