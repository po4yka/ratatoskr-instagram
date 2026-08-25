# Add explicit capture intake

## Why

Implementation plan item 3 (`docs/IMPLEMENTATION_PLAN.md`) is the first ingestion lane this service implements. The legacy monolith let the user paste Instagram URLs and captured them explicitly; this change rebuilds that lane on the provenance model from plan item 2, so a share becomes an honest `explicit_user_capture` record instead of an unclassified row. Without it, the schema's capture tables have no writer and every later item (public resolution, events, exports) has no intake to hang from.

## What Changes

- A permalink canonicalization module that turns the Instagram URL forms clients actually deliver (`http`/`https`, `instagram.com`, `www.`/`m.`/`l.`/`instagr.am`, `/p/`, `/reel/`, `/reels/`, `/tv/`, username-prefixed paths, tracking query strings) into one stable canonical permalink, and rejects everything else with a typed reason.
- Capture identity and deterministic idempotency: a unique `(user_ref, canonical_url)` pair identifies a capture, so a duplicate submission — same user, same URL, different timestamp, note, client key or client source — reuses the existing capture record instead of creating a second one.
- Capture intake persistence: `submit_capture` creates a capture with provenance fixed at `explicit_user_capture` and the wire acquisition method implied by the client source; the optional platform `Idempotency-Key` is stored for correlation but never participates in identity.
- Unavailable fallback: `record_capture_unavailable` appends an availability observation against the capture and moves it to status `unavailable`, preserving the canonical URL, captured time and note truthfully and creating no media row and no content.
- An HTTP product plane on the service process: `POST /v1/captures` speaking the documented platform capture grammar (`platform`, `canonical_url`, `captured_at`, `source`, optional `note`, optional `Idempotency-Key` header), with its own loopback listener configuration.
- The capability matrix flips `ExplicitCapture` from `Planned` to `Supported` — the reviewed test change the matrix requires before any code path may claim the lane.
- Schema extension in place (no migration, per development status): `captures` gains a `client_idempotency_key` column and a uniqueness constraint over `(user_ref, canonical_url)`.

Out of scope, unchanged: public resolution logic and raw revision storage (plan item 4), media storage policy (plan item 9), event publication, and caller authentication machinery.

## Capabilities

### New Capabilities

- `capture-intake`: accepting an explicit user capture of a public permalink — URL canonicalization to stable permalinks, deduplicating identity, truthful unavailable fallback records, and the HTTP intake surface.

### Modified Capabilities

- `capability-model`: the `ExplicitCapture` mode reports `Supported`; the scenario asserting that no mode reports `Supported` is narrowed to the modes whose plan items are still open.

## Impact

- `crates/instagram-archive`: new `permalink` and `capture` modules; `capability.rs` support-status flip; new dependency on `time` (RFC 3339 instants, sqlx integration); lib.rs exports.
- `schema.sql`: `captures` gains one nullable column and one uniqueness constraint; edited in place per the binding development status.
- `services/instagram-archive`: product router and handler, second listener wiring, API listen-address configuration key.
- Docs: `README.md`, `DEVELOPMENT.md`, `docs/CAPABILITY_MATRIX.md`, and the OpenSpec project context move item 3 from planned to implemented.
- No provider credentials, no network calls to Instagram, no browser sessions: intake validates and stores; resolution remains a later item.
