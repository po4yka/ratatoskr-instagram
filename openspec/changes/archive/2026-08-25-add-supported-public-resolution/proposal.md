## Why

Plan item 4 of `docs/IMPLEMENTATION_PLAN.md` is the next unbuilt lane: a capture today stays `accepted` forever or falls back to `unavailable`, because nothing resolves the public source behind the permalink. The capability matrix reserves the `PublicResolution` mode and its `public_resolution` wire method, but reports it `Planned`, so no code path may claim it yet. This change builds the lane honestly: resolve only through the approved official embed/oEmbed-style surface, keep every raw payload as an immutable parser-versioned revision before any normalization, and never let a re-resolution overwrite history.

## What Changes

- Flip `PublicResolution` in the capability matrix from `Planned` to `Supported` via its own reviewed test change, keeping wire method `public_resolution` and the `explicit_user_capture` authority ceiling.
- Add a public resolver that accepts canonical permalinks (`/p/`, `/reel/`, `/tv/`) and fetches their public representation through one approved surface seam; tests drive the seam with recorded fixtures only, never live calls.
- Add immutable revision storage: every successful resolution appends a new revision row referencing the content-hashed raw payload (`raw_records`, kind `oembed_response`) and the parser version that will interpret it; re-resolution appends another revision and never updates or deletes prior ones.
- Add deterministic normalization from fixture payloads into the existing `media` table (acquisition `public_resolution`, authority `explicit_user_capture`, upstream status collapsed from the observation), with the media row linked to its current revision.
- Record failures truthfully per capability matrix vocabulary: unsupported shapes yield `unsupported` observations, transient failures yield `temporarily_unavailable`, private/deleted outcomes keep their kinds; failed resolutions fabricate no media row and no revision.
- Link a resolved capture to its media row (`captures.media_id`, status `resolved`).
- Schema changes edit `schema.sql` in place (development status: no migrations): a `media_revisions` table and the linkage columns it needs.

## Capabilities

### New Capabilities

- `public-resolution`: resolving supported public permalinks through the approved official embed surface into immutable parser-versioned revisions and deterministic normalized media records, with truthful unsupported/unavailable outcomes.

### Modified Capabilities

- `capability-model`: the `PublicResolution` mode reports `Supported` now that its implementing plan item lands; the scenario asserting that unimplemented lanes claim no support drops `PublicResolution` from its list.

## Impact

- `crates/instagram-archive`: new `resolution` module (surface seam, outcome types, normalization, persistence); `capability.rs` status flip; `lib.rs` export.
- `crates/instagram-archive/tests`: new `resolution.rs` integration suite plus fixtures under `tests/fixtures/oembed/`; updated `capability.rs` test for the flipped row.
- `schema.sql`: new `instagram_archive.media_revisions` table; linkage column on `media`; comments kept consistent.
- `docs/CAPABILITY_MATRIX.md`: matrix row and alignment notes updated to match the executable flip.
- No service HTTP surface change: auto-resolving captures at intake stays out of scope until eventing lands (plan item 5); the resolver is exercised as library behavior against disposable databases.
- No new external dependencies: the network client behind the surface seam arrives with provider credentials (item 6); recorded fixtures drive all current tests.
