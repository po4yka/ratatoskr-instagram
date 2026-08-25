# Define the capability matrix and provenance semantics

## Why

The retired monolith resolved public embeds and took manual captures without ever stating what it could or could not acquire, so gaps were silent: records existed whose provenance nobody could explain, and absence was indistinguishable from deletion. Instagram's API surface is deliberately limited, so this bounded context must be explicit about which acquisition modes exist, what each mode is allowed to prove about saved state, and how upstream availability differs from local preservation — before any lane is implemented (plan items 3-9 inherit these semantics).

## What Changes

- Add a `capability` module to `crates/instagram-archive`: typed acquisition modes (`ExplicitCapture`, `PublicResolution`, `OwnAccountSync`, `DataExport`, `LegacyImport`), each with an honest support status, the closed acquisition-method values it produces on the wire, and the strongest saved-authority claim it may make.
- State the native Saved-list as an explicit non-capability (`NotSupported` with the documented reason): no supported provider surface exposes it, so no mode may claim it.
- Separate upstream status from preservation state as distinct vocabularies: the seven-value availability-observation taxonomy collapses into the five-value media upstream status honestly (private never reads as deleted), and no upstream observation ever demotes what was preserved locally.
- Close the first contract-alignment gap found against the published `ratatoskr-social-contracts` grammar: widen the `media` and `captures` acquisition-method CHECK vocabularies in `schema.sql` (edited in place per development status) to accept `public_resolution`.
- Document the capability matrix, the authority rules per mode, the upstream-versus-preservation boundary, and a value-for-value alignment review against `ratatoskr-social-contracts` at a recorded revision in `docs/CAPABILITY_MATRIX.md`; align the README authority summary.

Out of scope: implementing any acquisition mode (capture intake, resolution adapters, export import, OAuth sync are plan items 3+); consuming the contracts crate as a build dependency; event publishing.

## Capabilities

### New Capabilities

- `capability-model`: The provenance semantics layer — capability-matrix lookups per acquisition mode (support status, wire vocabulary, authority ceiling), the native-Saved non-capability, exhaustive mapping between local constants and the published social-contract grammars, and the rule set keeping upstream availability separate from local preservation state. Every requirement is executable against the library or the applied schema.

### Modified Capabilities

- `archive-schema`: The acquisition-method vocabulary enforced by the named CHECK constraints gains `public_resolution`, so a media record resolved through the supported public surface can carry its true provenance instead of being misfiled under a share method.

## Impact

- Code: new `capability.rs` module plus integration tests in `crates/instagram-archive`; `schema.sql` vocabulary widened in place; no service binary behaviour changes.
- Dependencies: none added. `ratatoskr-social-contracts` is consumed as a reviewed reference at a recorded revision, not a build dependency; the gap list records when that decision must be revisited (event publishing, plan item 8).
- Cross-repository contracts untouched: the social contracts are read-only here, and no store-visible behaviour changes.
- Fleet gates: unchanged; the existing CI gate covers the new module and schema text.
