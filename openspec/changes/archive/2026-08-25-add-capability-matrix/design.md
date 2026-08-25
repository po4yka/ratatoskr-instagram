# Design: capability matrix and provenance semantics

## Context

Plan item 1 left a service skeleton whose `schema.sql` already enforces closed provenance vocabularies, and the published `ratatoskr-social-contracts` crate (repo `po4yka/ratatoskr-contracts`, revision `361fe94`, 2026-08-25) defines the wire grammars this context must eventually publish: `AcquisitionMethod` (six variants including `PublicResolution`), `SavedAuthority` (four variants), `CaptureCompleteness`, and `UpstreamAvailability` (three values). The schema CHECKs were written before that review and accept only five acquisition methods — `public_resolution` is missing, so an honestly-provenanced public-resolution record cannot be stored today. The crate is not published to crates.io and no sibling repository consumes it yet; see proposal.md for motivation.

## Goals / Non-Goals

**Goals:**

- One typed home per concept in `crates/instagram-archive/src/capability.rs`, each serializing to exactly the snake_case string shared by the schema CHECKs and the contract serde representations.
- Lookups that make dishonesty unrepresentable: authority ceilings fixed per mode, support statuses explicit, native Saved list stated as `NotSupported` with its reason.
- Executable alignment: tests pin the local value sets against the recorded contract vocabularies and against the live database catalog, so drift in any of the three places (code, contracts, schema) fails CI.
- A written alignment review (`docs/CAPABILITY_MATRIX.md`) recording the mapping, the found-and-fixed gap, and remaining gaps with dispositions.

**Non-Goals:**

- Implementing any acquisition mode (plan items 3+); flipping any mode's status to `Supported` is their job.
- Adding `ratatoskr-social-contracts` as a build dependency (see Decisions).
- Schema changes beyond the vocabulary widening; preservation state gets no column until media handling (plan item 6) defines storage policy.

## Decisions

### 1. Mirror the contract vocabularies as local constants; defer the dependency

Local enums carry `as_str()`/parse pairs whose strings equal both the schema CHECK values and the contract serde values. The alignment test hardcodes the contract sets copied from `crates/social-contracts/src/vocabulary.rs` at the recorded revision, cited in the test doc comment.

Alternatives: consuming the crate via a git dependency would give compile-checked exhaustiveness, but couples this gate to another repository's HEAD, has no sibling precedent, and buys nothing until event publishing (plan item 8) actually constructs contract payloads — at which point the dependency arrives deliberately, likely after a publishing decision for the contracts repo. Copying without a recorded revision was rejected; the review records `361fe94`.

### 2. Five modes, not four, so the mapping is total

The task names four forward-looking modes; `LegacyImport` is added because the contract grammar carries it and the monolith migration (README milestones) must land somewhere honest. Every contract `AcquisitionMethod` variant belongs to exactly one mode: `ExplicitCapture` produces `share_extension` + `browser_extension`; `PublicResolution` produces `public_resolution`; `OwnAccountSync` produces `official_api`; `DataExport` produces `data_export`; `LegacyImport` produces `legacy_import`.

### 3. Support status is a three-valued explicit answer, not absence of code

`SupportStatus::{Supported, Planned, NotSupported}`. All five modes report `Planned` today; `NATIVE_SAVED_LIST_SYNC` reports `NotSupported` ("no supported provider surface exposes the personal Saved list"). Flipping a status is a deliberate test-plus-spec edit in the implementing change, which turns "we silently started claiming things" into a reviewed diff.

Alternative considered: deriving status from feature flags — rejected; capability truth is compile-time policy here, not runtime configuration.

### 4. Authority ceiling is data on the mode, enforced by construction

`authority_ceiling()` returns the strongest claim the lane proves. Nothing offers a conversion that raises authority; later items that record provenance must go through these constants. The collapse rules for observations exist because two vocabularies genuinely differ; there is deliberately no analogous function from upstream status or observations into `PreservationState` — instead `retention_after_observation(current, _observed)` returns `current` unchanged, making "an observation never demotes preservation" an executable, documented rule rather than a convention.

### 5. Two upstream-facing vocabularies stay two

`UpstreamStatus` (media column: available/unavailable/deleted/private/unknown) and observation kinds (observations table: adds temporarily_unavailable/unsupported/resolution_failed) mirror their schema CHECKs exactly. Collapse mapping: private→unavailable, temporarily_unavailable→unavailable, unsupported→unknown, resolution_failed→unknown, others identity. Private never collapses to deleted because Instagram refusing an embed is evidence of access, not of absence.

### 6. TDD pairs start from assertion-level failures

Task 1.1 lands the test files together with a skeleton `capability.rs` whose lookups return placeholder values, so every failure is a failed assertion about behavior, not a compile error. The schema test fails on the CHECK violation itself, which needs no skeleton.

## Risks / Trade-offs

- [Mirrored constants drift from contracts] → alignment test pins both directions value-for-value and cites the revision; plan item 8 replaces mirroring with the real crate.
- [`retention_after_observation` looks like a no-op] → it encodes AGENTS.md's "absence never causes deletion" as a checked invariant; docs state why it exists.
- [Schema vocabulary widened while no resolver writes it yet] → harmless under dev-status (fresh databases only), and required before plan item 3 can store honest provenance.
- [Status table rots when lanes ship] → spec scenario "No mode claims support it does not have" fails until the implementing change updates both test and spec together.

## Migration Plan

None needed: development status means fresh databases only; `schema.sql` is edited in place and no deployment holds data across the change. Rollback is reverting one commit.

## Open Questions

None.
