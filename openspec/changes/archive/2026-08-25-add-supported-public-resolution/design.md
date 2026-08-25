## Context

Plan items 1–3 shipped the service foundation and the explicit capture lane. The pieces item 4 builds on exist: canonical permalinks (`permalink.rs`), the closed provenance vocabularies (`capability.rs`), the unavailable fallback (`capture.rs::record_capture_unavailable`), and the first-version schema (`schema.sql`) whose `media`, `raw_records`, `availability_observations`, and `captures` tables were designed for this lane but are unused by it. The capability matrix reports `PublicResolution` as `Planned`; flipping it is part of this change. There is no BlobStore, no HTTP client dependency, and no provider credential story yet; development status forbids migrations, so `schema.sql` is edited in place.

## Goals / Non-Goals

- Goals: the resolution lane end to end at the library level — one approved-surface seam, immutable parser-versioned raw revisions, deterministic normalization into `media`, truthful unsupported/failure observations, capture linkage — plus the capability-matrix flip and documentation consistency.
- Non-Goals: the network client and OAuth credentials for the real endpoint (item 6); automatic resolution wired into `POST /v1/captures` and event publication (item 5); BlobStore infrastructure and media-byte archival (later items); Data Export intake (item 8).

## Decisions

### D1: The approved surface sits behind one seam; recorded fixtures drive it in tests

`PublicSurface` is a trait with one async method taking a `CanonicalPermalink` and returning a typed outcome: a payload (`body` bytes) or a classified failure (`Deleted`, `Private`, `TemporarilyUnavailable`, `Unsupported`, `TransportFailure`). Tests use a hand-written `FakePublicSurface` replaying committed fixture files; no test makes a live call.

Why not ship the HTTP client now: the official oEmbed-style endpoint requires an app access token, so an anonymous fetch today would be exactly the scraping this repository prohibits; `DEVELOPMENT.md` defers Reqwest/Rustls to the credential-bearing item. The seam keeps endpoint construction, auth, retries, and timeouts in item 6 without touching this lane's contract.

### D2: Revisions are their own table referencing content-addressed raw evidence

New `instagram_archive.media_revisions`: `revision_id` (UUIDv7, time-sortable), `media_id`, `raw_record_id`, `parser_version`, `resolved_at`. One row per successful resolution attempt; the attempt identity stays separate from the bytes identity in `raw_records`. `media` gains `current_revision_id` pointing at the newest revision. Prior revisions are never updated or deleted — the storage API simply has no such path.

Each resolution writes its own `raw_records` row even if the payload repeats. Deduplicating identical payloads would be premature before BlobStore semantics exist; `content_hash` makes later consolidation possible.

### D3: Raw bytes live inline until BlobStore lands

`raw_records` gains a `body bytea` column; `blob_ref` carries the lowercase hex SHA-256 of the body — the future BlobStore key, stable across the move. oEmbed documents are small JSON, so inline storage is proportionate now; when BlobStore arrives (export/media items), bodies move out and references remain meaningful. This edits the schema comment honestly instead of pretending a store exists.

### D4: Normalize only what evidence supports; uncertain media type is stored as `unknown`

The parser reads the documented oEmbed-style grammar: author name/URL, title text, dimensions, thumbnail references, embed markup — all optional. What it projects into the normalized row is exactly what the `media` table models: the media type implied by the permalink kind, which is direct evidence (`reel` → `reel`, `tv` → `video`), and the title text as caption when present. A plain `/p/` post does not reveal its type through this grammar, so guessing `image` would fabricate; the CHECK vocabulary widens with `'unknown'` and plain posts store that. Publication time and provider media identifiers are absent from the grammar and stay `NULL`. Author identity would belong in `profiles`, whose identity key this grammar cannot fill reliably — deferred to the lane that observes profiles properly. Everything unrecognized survives solely in the raw body. The parser version constant (`instagram.oembed.v1`) is stamped on every revision.

Determinism is structural: normalization is a pure function of payload bytes, so equal inputs produce equal normalized values by construction; the tests pin it anyway.

### D5: Every resolution appends an availability observation; failures never invent deletion

Success appends `available` bound to both the media row and the capture, then links `captures.media_id` and sets status `resolved` inside the same transaction as the revision insert. Failures append their outcome verbatim — `unsupported`, `deleted`, `private`, `temporarily_unavailable`, `unavailable` for unclassified surface answers, `resolution_failed` when the attempt fails before classification — bound to the capture alone, moving the capture to `unavailable` via the same rules as the existing fallback. No failed outcome creates media or a revision; none rewrites another's kind.

On permalink conflict the media upsert updates only this lane's projection columns (`provider_media_id`, `caption`, dimension/thumbnail columns, `current_revision_id`, `updated_at`) and never touches provenance columns: acquisition and authority are immutable once written.

### D6: The flip is a reviewed test change, matching the matrix discipline

`capability.rs` flips `PublicResolution` to `Supported` in the same commit as the test that demands it; `docs/CAPABILITY_MATRIX.md` updates its row and status prose in the same change, keeping the document and the executable truth identical.

## Risks / Trade-offs

- [Inline bodies duplicate bytes across identical re-resolutions] → accepted pre-BlobStore; `content_hash` supports consolidation when storage policy lands.
- [`'unknown'` joins the media-type vocabulary] → required by the honest-uncertainty rule; item 5's event publication must carry it forward unchanged.
- [Permalink-keyed upsert meets rows from other lanes someday] → provenance columns are never rewritten by this lane; the official-account item inherits that guard.
- [Fixtures drift from the live surface] → fixtures are synthetic redacted samples defining the grammar this parser trusts; the client item validates them against recorded provider responses.

## Migration Plan

None under development status: fresh databases apply the edited `schema.sql`; disposable test databases are created per test. Existing local databases are throwaway and simply recreated.

## Open Questions

None blocking. The production endpoint URL and token handling are item 6 decisions that do not affect this lane's contract.
