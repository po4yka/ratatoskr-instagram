# Instagram Capability Matrix

Status: authoritative for this repository. Records what `ratatoskr-instagram` can acquire from Instagram, what each acquisition mode is allowed to prove about saved state, and how the model aligns with the published social contracts.

The monolith resolved public embeds and took manual captures without a capability model, so gaps were silent: records existed whose provenance nobody could explain, and absence was indistinguishable from deletion. This matrix exists so that cannot recur. The executable form of everything below lives in `crates/instagram-archive/src/capability.rs`, `crates/instagram-archive/src/resolution.rs`, and the matching `tests/`; the behavior contract lives in `openspec/specs/capability-model/` and `openspec/specs/public-resolution/`.

## The matrix

`ExplicitCapture`, `PublicResolution`, `OwnAccountSync`, and `DataExport` are implemented and report `Supported`. Data Export support means authenticated raw-first import of the one admitted synthetic-fixture grammar; it does not claim compatibility with every provider export or native Saved synchronization. Own-account support is conditional per connected account: only a current available `own_media_read` generation may contact the provider; unsupported types and missing permissions become recorded no-ops.

| Mode | Status | Wire acquisition methods | Authority ceiling |
|---|---|---|---|
| `ExplicitCapture` | Supported | `share_extension`, `browser_extension` | `explicit_user_capture` |
| `PublicResolution` | Supported | `public_resolution` | `explicit_user_capture` |
| `OwnAccountSync` | Supported | `official_api` | `authoritative_platform_state` |
| `DataExport` | Supported | `data_export` | `export_observation` |
| `LegacyImport` | Planned | `legacy_import` | `legacy_observation` |

## Stated non-capabilities

| Capability | Status | Reason |
|---|---|---|
| Native Saved-list synchronization (personal account) | NotSupported | no supported provider surface exposes the personal Saved list |

## Connected-account capability projection

Every discovery generation replaces all six rows for one account. Business and creator accounts gain
`account_identity_read` and `own_media_read` only when `instagram_business_basic` is observed as
granted. Personal and unknown accounts gain nothing. `native_saved_read` is always `not_supported`.
Publishing, comment management, and messaging additionally require their exact granted permission
and separate external-write consent; item 6 requests neither and records them unavailable.

Closed reasons distinguish `granted`, unsupported account type, declined/expired/absent/unknown
permission, missing write permission, missing write consent, provider non-support, revocation, and
reauthorization required. The generation links to preserved raw permission evidence. Callers consume
this projection and must never reinterpret the originally requested scope as a grant.

The scheduler requests only the connected professional account's non-ephemeral media edge. Stories
and native Saved remain outside the matrix. Returned foreign-owner items are refused, and a provider
media URL never becomes a BlobRef unless this service has actually stored and hashed those bytes.

Instagram provides Ratatoskr with no API that reads a personal account's native Saved list. No mode may synthesize that claim: an explicit capture proves the user saved the item to Ratatoskr at `captured_at`, an export proves it was saved at some point in the past, and neither proves current native membership. Deleting a Ratatoskr capture likewise never implies a native unsave.

## Authority rules

The authority ceiling is data on the mode, not caller discipline:

- `ExplicitCapture` and `PublicResolution` may never exceed `explicit_user_capture`. Public resolution observes upstream content; it does not observe the user's saved state, so resolving a capture raises availability knowledge only, never authority.
- `OwnAccountSync` may reach `authoritative_platform_state`, but only for media of the connected professional account that the official API actually exposes. A connected account never widens the authority of captures about other accounts.
- `DataExport` may never exceed `export_observation`: exports show past state without live authority.
- `LegacyImport` may never exceed `legacy_observation`: migrated records are worth exactly what the monolith proved.

Nothing in the module offers a conversion that raises authority above a mode's ceiling. Downstream events preserve these values unchanged (`ratatoskr-knowledge` and clients must be able to trust the label).

## Upstream status versus preservation

Two questions stay two vocabularies because they have different owners:

- **Upstream** — what Instagram last reported. Stored per observation (`availability_observations.availability`: `available`, `unavailable`, `deleted`, `private`, `temporarily_unavailable`, `unsupported`, `resolution_failed`) and collapsed onto the media row (`media.upstream_status`: `available`, `unavailable`, `deleted`, `private`, `unknown`).
- **Local** — what Ratatoskr holds (`PreservationState`: content preserved, metadata only, user artifact only, nothing beyond the capture record).

Collapse mapping, enforced by `AvailabilityObservationKind::collapse_to_media_status`:

| Observation | Media status |
|---|---|
| `available` | `available` |
| `unavailable` | `unavailable` |
| `deleted` | `deleted` |
| `private` | `unavailable` |
| `temporarily_unavailable` | `unavailable` |
| `unsupported` | `unknown` |
| `resolution_failed` | `unknown` |

`private` collapses to `unavailable`, never `deleted`: being denied access is evidence about access, not existence. Unsupported shapes and failed resolutions learned nothing, so they become `unknown` rather than inventing a state.

Preservation is independent of every observation. `retention_after_observation` is identity on purpose: observing deletion upstream keeps whatever was captured before, absence from a later export deletes nothing, and demotion happens only through explicit user action. A metadata-only capture is never reported as a complete backup.

## Alignment review: `ratatoskr-social-contracts`

Reviewed against `po4yka/ratatoskr-contracts` revision `361fe94` (2026-08-25), file `crates/social-contracts/src/vocabulary.rs`. The contract crate is not published to crates.io and no sibling repository consumes it as a dependency yet; this service mirrors its vocabularies as constants whose strings equal both the schema CHECK values and the contract serde representations, pinned value-for-value by `local_method_and_authority_sets_equal_the_recorded_contract_sets`.

| Contract concept | Local counterpart | Verdict |
|---|---|---|
| `AcquisitionMethod` (6 closed variants) | one wire method per variant, owned by exactly one `AcquisitionMode` | aligned, exhaustive |
| `SavedAuthority` (4 closed variants) | `SavedAuthority` mirror; reachable set equals the vocabulary via mode ceilings | aligned, exhaustive |
| `UpstreamAvailability` (`available`, `unavailable`, `deleted_upstream`) | collapse of `UpstreamStatus` at event-publication time: `available`→`available`; `unavailable`, `private`, `unknown`→`unavailable`; `deleted`→`deleted_upstream` | aligned; private and unknown deliberately never publish as deleted |
| `CaptureCompleteness` (`complete`, `partial`) | not modeled locally yet | gap — consumed where captures and imports are recorded (plan items 3 and 6); partial requires warnings there |
| `SocialFolderMembership` | not applicable | Instagram exposes no provider-native saved-folder membership through a supported surface |

Gaps found and their disposition:

1. Schema CHECKs lacked `public_resolution` while the contract grammar carries it — fixed in this change by widening `media_acquisition_method_check` and `captures_acquisition_method_check` in place.
2. Contracts crate consumed as reviewed reference, not build dependency — deliberate until event publication (plan item 8) constructs real payloads; revisit then, ideally once the contracts repo decides how it publishes.
3. Preservation state has no column yet — intentional until the media-handling plan item defines storage policy and budget; the type exists so the distinction precedes storage.
