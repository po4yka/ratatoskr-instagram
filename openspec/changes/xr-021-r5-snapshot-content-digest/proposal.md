## Why

Changeset XR-021 round 5 found that the content digest and the observation time of every Instagram fact describe nothing about the post. `content_digest()` returns the SHA-256 of the fixed string `instagram/oembed-metadata-only/v1` for every post, and `build_snapshot` stamps `captures.captured_at`, the browser capture time, on every fact. A later `social.source.updated.v1` (a changed caption, or a post deleted upstream) therefore repeats the first fact's digest and observation time, and Knowledge, which keys the analysis revision by `(tenant, source, digest)` and advances the head by `observed_at`, never analyses an Instagram update. The cross-repository rule is XR-021 CONTRACTS.md R5-09; this change cites it and does not restate it.

## What Changes

- **BREAKING** The digest of every Instagram snapshot changes from a constant to SHA-256 over the canonical JSON of what the snapshot states (domain, permalink, shortcode, caption, upstream availability, media). One function, `publishing::snapshot_content_digest`, builds it from a `SocialSourceSnapshot`; the `content_digest()` constant and its callers are removed. Nothing is deployed, so the break is named, not staged.
- An `updated` fact stamps `captured_at` with the instant its current state was observed (the newest of the capture time, the current revision's `resolved_at` and the media projection's observation time); a `captured` fact keeps the capture time.
- `analysis_linkage` accepts a Knowledge completion only when its digest equals the digest of the current snapshot of the matching capture, computed with the same function, and stores that digest.
- The pinned `ratatoskr-*` contracts revision moves to CONTRACTS_SHA_R5 (`d98967b7b7ee80442c93ef4657140068955c8f61`).

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `social-source-publishing`: the content digest, the observation time of updated facts and the completion linkage.

## Impact

- `crates/instagram-archive`: `src/publishing.rs`, a new `src/publishing/snapshot_digest.rs`, `src/analysis_linkage.rs`, `tests/snapshot_digest.rs`.
- `Cargo.toml`, `Cargo.lock`, `fuzz/Cargo.lock` (contracts pin).
- Out of scope: the data-export lane's own digests (`publishing/data_export.rs` already hashes content) and the official own-media digest (it already hashes content).
