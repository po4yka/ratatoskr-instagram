## Context

See [proposal.md](proposal.md) for motivation and the delta specs for observable behavior. Plan item 6 already supplies owner-bound encrypted credentials, a current total capability generation, finite provider-attempt accounting, a strict Reqwest/Rustls adapter, disposable PostgreSQL integration tests, and the `media` / raw-revision / outbox foundations. It does not list own media, schedule account work, persist scan progress, or distinguish staged from authoritative own-media generations.

The provider collection is newest-first and paginated. A continuation cursor proves only where one run may resume; a stable provider media identifier is the durable incremental watermark. The workspace `blob-references` contract permits a `BlobRef` only for bytes this service actually stores and can verify. Development status forbids migrations, later major versions, and compatibility paths.

## Goals / Non-Goals

**Goals:**

- Exercise only the currently observed `own_media_read` capability for the connected account and record every ineligible job as a truthful no-op.
- Make scheduled work resumable and bounded while changing the visible own-media projection and its watermark only at one successful completion boundary.
- Preserve strict raw evidence, provider identity, metadata provenance, publication time, and verifiable BlobRef semantics through normalized SocialSource facts.
- Keep the default test suite deterministic with synthetic business/creator pages and no personal account.

**Non-Goals:**

- Native Saved synchronization, personal-account fallback, stories or ephemeral media, comments, insights, provider writes, webhooks, foreign-account discovery, or browser/private API access.
- Treating an expiring provider CDN URL as durable stored media, or adding media-byte archival before the explicit policy planned for item 9.
- Adding a Platform scheduler contract or a new external transport; this item owns the service-local cadence and one bounded `run_due_once` composition seam.

## Decisions

### D1. Use a local disabled-by-default scheduler over due account state

An `OwnMediaSyncConfig` adds an enable flag, cadence, per-tick account cap, page cap, and call-attempt cap. The service spawns one delayed Tokio interval only when OAuth and own-media sync are both enabled. Each tick invokes a bounded `run_due_once` seam that claims due connected accounts from PostgreSQL; tests call that seam directly instead of sleeping. A database due timestamp and one-active-run constraint make two service instances converge rather than double-sync.

An HTTP trigger was rejected because scheduled work is not a product command. A new NATS command was rejected because no cross-repository scheduler contract is required for this local cadence. Enabling the loop by default was rejected because a deployment that has only completed OAuth review must not begin new provider traffic silently.

### D2. Gate before credential opening and revalidate at finalization

Claiming a job loads the account row and the current `own_media_read` capability row together. Anything other than connected plus available writes a terminal `capability_noop` run with the observed generation and closed reason, without opening the credential or reserving provider usage. An eligible run records its capability generation. Completion locks the account and rejects a changed generation, connection state, owner, or provider account identity.

Filtering unsupported accounts out of the scheduler was rejected because absence would hide whether a due job was intentionally skipped. Checking only once was rejected because permission refresh may downgrade an account while a multi-page run is in flight.

### D3. Separate provider continuation from the committed observation watermark

The run stores the opaque next-page cursor after every accepted page so retry resumes work already paid for. The account state stores only the stable provider media id of the newest item from the last completed traversal. An initial run must reach end-of-collection; later runs may stop when they observe the old watermark. The candidate watermark is the first item in the new run and becomes current only at completion.

Persisting a provider cursor as the long-term watermark was rejected because cursors can expire and consumers must not interpret them. Advancing after the first page was rejected because a failed later page would create a gap that future scans incorrectly treat as covered.

### D4. Build a complete candidate generation, then swap authority once

The schema gains `own_media_sync_state`, `own_media_sync_runs`, and `own_media_sync_items`, plus one account authority pointer. Starting an incremental run copies membership from the prior authoritative generation into its candidate. Accepted pages upsert staged items by stable provider media id; observed items replace their candidate metadata, while unvisited old identities remain. The completion transaction validates the run, upserts normalized `media` and immutable revisions, writes idempotent outbox facts, marks the run completed, moves the authority pointer, advances the watermark, and schedules the next due time.

This gives atomic Ratatoskr projection authority without claiming that absence from a bounded prefix proves provider deletion. Deleting old members based on prefix absence was rejected as fabricated state. Updating live `media` rows page by page was rejected because observers could see half a run.

### D5. Keep one active run resumable and every provider request pre-accounted

A partial unique constraint permits one `running` or `retryable` run per account. Each page request reserves `RequestClass::OwnMediaPage` through the existing durable `ProviderBudget` before I/O, under the run id as operation id. A retryable network/rate/server failure keeps staged pages and the cursor; response refusal, foreign ownership, and capability drift terminate the run without authority. Page and request caps stop before further contact and retain the old watermark.

Restarting a new run after every fault was rejected because it discards accepted evidence and repeats provider cost. Automatic unbounded retries were rejected because the durable budget is a correctness boundary.

### D6. Extend the provider port with strict own-media pages

`InstagramProvider` gains a paginated own-media operation taking the connected provider account id, secret token, and optional opaque continuation. The production adapter targets only that account's documented media edge, sends the token in the authorization header, requests a fixed reviewed field set for stable id, owner identity, type, permalink, caption, publication time, child metadata where supported, and media URL observations, and omits story/ephemeral fields. Responses are byte-capped, deny unknown top-level/page-control fields, reject duplicate ids and foreign owners, and preserve each accepted raw page before normalization.

Generic Graph traversal and tolerant unknown shapes were rejected because they expand authority and can normalize unreviewed provider data. Downloading returned URLs was rejected until item 9 defines rights, expiry, size, MIME, and retention policy.

### D7. Treat raw JSON and media bytes as different BlobRef claims

Each accepted raw page uses the existing content-addressed `raw_records` path and can become the snapshot's service-owned `raw_blob`: the exact bytes, digest, JSON media type, and length are known. Provider CDN URLs remain protected observations inside raw evidence and staged metadata. Because this item archives no media bytes, it emits no `SocialMediaItem`; SocialSource completeness is partial with a stable missing-media warning.

Minting a BlobRef from a URL or expected content was rejected because the workspace contract requires stored bytes with a verifiable digest and length. Adding a general blob service was rejected because the workspace explicitly assigns blob ownership to each producer.

### D8. Publish only from the completion transaction

The existing publisher is generalized from capture-only identity to an owned-source snapshot builder. Stable own-media identity derives from owner plus provider media id, not permalink. First completed visibility appends `social.source.captured.v1`; a later changed content digest appends `social.source.updated.v1`; unchanged rows append nothing. The snapshot carries official acquisition, observed platform-state authority for the account's own media, provider publication time, raw evidence BlobRef, sync checkpoint, and partial completeness. Outbox uniqueness keys the source, fact kind, and content digest so uncertain finalization replay cannot duplicate facts.

Publishing each staged page was rejected because downstream consumers would observe a generation the service had not accepted as current. Reusing capture identity was rejected because own-account media is not an explicit user capture.

### D9. Keep telemetry closed and content-free

Metrics count due, resumed, completed, capability-noop, retryable, refused, and failed runs; page attempts remain in the existing usage ledger. Logs and labels carry only non-sensitive account/run ids, capability reason, terminal class, counts, and bounded durations. They exclude username, caption, provider URL, raw body, and token data.

## Risks / Trade-offs

- [Older provider metadata changes outside the newest-first prefix] → incremental scans do not observe them; item 9's explicit re-resolution tooling or a later reviewed periodic full traversal can refresh them without weakening the current absence semantics.
- [Provider continuation expires between retries] → terminate the candidate without authority or watermark change and let the next scheduled run start from the committed watermark.
- [Copying prior membership grows with a large account] → use one set-based insert and a finite per-account catalogue; retain the atomic model until measurement justifies a deeper representation.
- [Capability changes during provider I/O] → finalization revalidates the generation and refuses stale authority even though already-accounted provider calls remain truthful usage.
- [Raw JSON remains inline until the local blob path exists] → keep the existing content-addressed digest/body representation and emit only BlobRefs the service can resolve; moving bytes later does not change the contract.

## Migration Plan

Development status forbids migrations. Edit `schema.sql` in place and extend the disposable-database inventory. Deploy with own-media scheduling disabled, apply a fresh development database, verify OAuth/capability state and synthetic fixtures, then enable the cadence only in an authorized environment. Rollback disables the scheduler and runs the prior binary against a freshly initialized development database; no compatibility layer or preservation promise exists in this phase.
