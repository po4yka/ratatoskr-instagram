# Ratatoskr Instagram

`ratatoskr-instagram` is the Instagram account and capture bounded context for Ratatoskr. It combines official account access where available with explicit user-initiated captures, public oEmbed resolution, and versioned Data Export imports.

> **Status:** architecture bootstrap. Account connection, capture resolution, import parsers, persistence, and events described below are planned and are not implemented yet.

> [!IMPORTANT]
> **Ratatoskr is in development.** No database holds data that has to survive a schema change.
> While this status holds, these two rules replace what the documents below plan:
>
> - the API and the database keep their first version. There is no `v2` and no later major
>   version.
> - the database has no migrations. No schema exists yet. The first persistence change creates one
>   schema definition, and later schema changes edit it in place.
>
> Only the repository owner changes this status.

## Role in Ratatoskr

Instagram does not provide Ratatoskr with an authoritative API for a personal user's native Saved feed. This service therefore keeps two ingestion lanes separate and records the authority of every item honestly.

### Official account lane

For supported professional accounts, the connector may manage:

- Instagram account identity;
- officially available own-media metadata;
- captions and publication metadata;
- comments, publishing, or other capabilities only when explicitly enabled and supported;
- token lifecycle, scopes, reauthorization, and audit.

### Explicit capture lane

For public posts and reels the primary archival workflow is:

```text
Instagram
  -> Share
  -> Ratatoskr mobile Share Extension
  -> canonical URL
  -> ratatoskr-instagram
  -> official public representation / oEmbed
  -> normalized SocialSource
```

Desktop users can perform the same explicit action through `ratatoskr-browser-extension`.

A captured item represents **"the user saved this to Ratatoskr"**, not proof that the item currently exists in Instagram's native Saved list.

## Core responsibilities

- official Instagram account connection for supported account types;
- encrypted provider credential lifecycle;
- own-media synchronization where officially available;
- explicit URL capture resolution;
- public post/reel/profile oEmbed normalization;
- user notes and capture provenance;
- imported Instagram Data Export snapshots;
- media and attachment blob references;
- upstream availability state;
- normalized social-source events;
- provider-specific raw evidence and parser versioning.

It does not run hidden browser login automation, scrape a user's private Saved feed, store passwords or session cookies, or perform LLM analysis.

## Authority model

Every normalized source records acquisition method and saved-state authority:

```text
acquisition = OfficialApi | ShareExtension | BrowserExtension | DataExport | LegacyImport
saved_authority = AuthoritativePlatformState | ExplicitUserCapture | ExportObservation | LegacyObservation
```

Typical public capture:

```text
platform = Instagram
acquisition = ShareExtension
saved_authority = ExplicitUserCapture
native_saved_state = Unknown
```

This distinction is load-bearing. Absence from a future export or failed public resolution does not prove the user removed the item from Instagram or Ratatoskr.

## Planned data model

The service will own an `instagram_archive.*` PostgreSQL schema when persistence is implemented:

```text
instagram_accounts
instagram_credentials
instagram_profiles
instagram_media
instagram_media_relations
instagram_captures
instagram_capture_notes
instagram_export_snapshots
instagram_import_runs
instagram_raw_records
instagram_availability_observations
outbox_events
inbox_events
```

Large export archives, media files, screenshots, raw API/oEmbed payloads, and unknown provider records are stored in the content-addressed BlobStore.

## Capture API flow

Clients submit captures through `ratatoskr-platform`:

```http
POST /v1/captures
Idempotency-Key: 018f...
```

```json
{
  "platform": "instagram",
  "canonical_url": "https://www.instagram.com/reel/...",
  "captured_at": "2026-08-17T10:30:00+04:00",
  "source": "ios_share_extension",
  "note": "Save for composition analysis",
  "collection_ids": ["..."]
}
```

Platform creates an operation and publishes a typed command. Instagram:

1. validates and canonicalizes the supported URL shape;
2. deduplicates the explicit capture using the idempotency and canonical identity;
3. resolves the public representation through an official supported surface;
4. stores raw evidence and normalized metadata;
5. reports partial or unavailable status honestly;
6. publishes `social.source.upserted.v1`;
7. lets Knowledge analyse the normalized source asynchronously.

## Public media normalization

The normalized representation may contain:

- stable local source ID;
- provider external ID when available;
- canonical URL;
- profile/author identity and display name;
- caption or public text;
- publication timestamp;
- media type and dimensions;
- thumbnail and media references where storage is permitted;
- related post/reel/profile references;
- capture timestamp, source client, user note, and provenance;
- raw provider payload reference;
- upstream availability status.

Unknown fields are preserved in raw records so future parser versions can recover information without re-acquiring the source.

## Private and unavailable content

Ratatoskr never attempts to bypass Instagram privacy or access controls.

For private, removed, region-limited, login-only, or otherwise unavailable content the service may retain:

- canonical URL;
- explicit capture timestamp;
- user note and local collections;
- client-supplied selected text when explicitly provided;
- user-uploaded screenshot or file as a separate artifact;
- an `unavailable` reason and observation time.

A screenshot supplied by the user is not labeled as provider-authoritative media. Its provenance remains `user_upload`.

## Data Export imports

Instagram Data Export is imported as a versioned snapshot, not assumed to have one permanent schema.

Import flow:

1. receive the user's archive through the local export agent or upload flow;
2. compute and preserve the archive hash;
3. store the original archive immutably;
4. detect archive structure and parser version;
5. safely extract into an isolated temporary directory;
6. parse known sections into staging tables;
7. preserve unknown sections as raw blobs;
8. reconcile known records without treating absence as deletion;
9. produce a completeness and warning report;
10. publish normalized source events where possible.

The importer never claims that a particular export contains a complete Saved list unless the detected provider schema explicitly supports and validates that conclusion.

## Official account connection

Provider credentials are owned only by this service. Requirements:

- OAuth state and callback binding;
- minimum necessary scopes;
- explicit account-type and capability detection;
- encrypted token storage and key rotation;
- expiry, refresh, and reauthorization status;
- separate consent for publishing or other mutations;
- no tokens in events, logs, traces, or client responses.

A connected professional account does not expand the authority of captures from unrelated public or private accounts.

## Linked content and analysis

Instagram publishes normalized source data; it does not interpret it.

- Public external links are sent to `ratatoskr-extractor`.
- Captions, media metadata, user notes, and resolved linked documents are analysed by `ratatoskr-knowledge`.
- Local collections remain owned by the product context and are not written back to Instagram.

## Commands and events

Expected contracts include:

```text
instagram.account.connected.v1
instagram.account.reauth_required.v1
instagram.account.sync_requested.v1
instagram.media.upserted.v1
instagram.capture.requested.v1
instagram.capture.resolved.v1
instagram.capture.unavailable.v1
instagram.export.ingest_requested.v1
instagram.export.ingested.v1
social.source.upserted.v1
social.source.unavailable.v1
```

All handlers are idempotent under at-least-once delivery. Replaying a capture or import converges on the same source and snapshot records.

## Security invariants

1. No Instagram password or browser session cookie is stored or requested.
2. The service uses official OAuth and officially supported public resolution surfaces.
3. A capture is always an explicit user action or a user-provided export observation.
4. Private-content access controls are never bypassed.
5. User-uploaded artifacts are labeled separately from provider-authoritative content.
6. Provider write scopes require separate consent.
7. Unknown export records are preserved rather than discarded.
8. Absence from an import never silently deletes an archived capture.
9. Knowledge and clients never receive provider credentials.

## Observability

Core metrics include:

```text
instagram_capture_duration
instagram_capture_resolved
instagram_capture_unavailable
instagram_oembed_failures
instagram_account_sync_duration
instagram_rate_limit_waits
instagram_export_import_duration
instagram_export_unknown_records
instagram_export_completeness
instagram_reauth_required
```

Every capture and import records acquisition method, parser or resolver version, result authority, warnings, and operation correlation.

## Non-goals

- Automatic mirroring of the native personal Saved feed without an official API.
- Server-side headless login or stealth browser scraping.
- Bypassing private account or media restrictions.
- LLM analysis, embeddings, or search ownership.
- Treating an explicit Ratatoskr capture as authoritative Instagram Saved state.
- Writing local tags and collections back to Instagram.
- Claiming complete export coverage without validated evidence.

## Initial milestones

1. Define account, capture, media, import, and provenance schemas.
2. Implement canonical Instagram URL recognition.
3. Add the explicit capture command and public resolver/oEmbed adapter.
4. Publish normalized social-source events.
5. Integrate Android/iOS Share Extensions and the browser extension.
6. Add safe versioned Data Export import.
7. Add supported professional-account OAuth and own-media synchronization.
8. Integrate linked documents with Extractor and analysis with Knowledge.
9. Add availability revalidation, completeness reports, and provider diagnostics.

## Workspace integration

Planned: `ratatoskr-workspace` will pin Instagram with compatible social contracts, Platform, Mobile, Browser Extension, Extractor, Knowledge, and client commits. No workspace pin or integration profile exists for this service today. The connector will remain independently testable with recorded public-resolution fixtures, synthetic exports, and mock OAuth/API servers.

## Project status

This README defines the intended Instagram connector and explicit-capture architecture. No provider connection, capture resolver, import parser, or persistence implementation exists yet.
