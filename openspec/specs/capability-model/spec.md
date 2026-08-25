# capability-model Specification

## Purpose
Defines the provenance semantics every Instagram acquisition lane inherits: what acquisition modes exist and whether they are supported, the strongest saved-authority claim each mode may make, how local constants map onto the published social-contract grammars, and why upstream availability is never confused with local preservation.

## Requirements

### Requirement: The capability matrix answers for every acquisition mode
The library SHALL expose a total lookup that, for each documented acquisition mode (`ExplicitCapture`, `PublicResolution`, `OwnAccountSync`, `DataExport`, `LegacyImport`), returns an explicit support status (`Supported`, `Planned`, or `NotSupported`), the closed set of wire acquisition-method values the mode produces, and the strongest saved-authority claim the mode is allowed to make. The mode inventory SHALL be exactly these five modes — no hidden sixth lane exists. A mode SHALL report `Planned` until the implementation plan item that builds its lane flips the status with a reviewed test change, and `Supported` from that item onward.

#### Scenario: Each mode resolves to its documented capability
- **WHEN** the capability of each acquisition mode is looked up
- **THEN** every lookup succeeds and reports one explicit support status, exactly the wire method values documented for that mode, and the authority ceiling documented for that mode

#### Scenario: No mode claims support it does not have

- **WHEN** the support statuses of `PublicResolution`, `OwnAccountSync`, `DataExport`, and `LegacyImport` are inspected while their implementing plan items are still open
- **THEN** none of them reports `Supported`

#### Scenario: The implemented explicit-capture lane reports support

- **WHEN** the capability of `ExplicitCapture` is looked up after its implementing plan item landed
- **THEN** it reports `Supported` while keeping its documented wire methods (`share_extension`, `browser_extension`) and its `explicit_user_capture` authority ceiling

### Requirement: The native Saved list is a stated non-capability
Because no supported provider surface exposes a personal account's native Saved list, the capability matrix SHALL report native Saved-list synchronization as `NotSupported` together with that reason, and no acquisition mode's authority path SHALL be able to produce a claim that the user's native Saved membership is known from an explicit capture.

#### Scenario: Native Saved synchronization reports not-supported with its reason
- **WHEN** native Saved-list synchronization is looked up in the capability matrix
- **THEN** the answer is `NotSupported` carrying the reason that no supported provider surface exposes the personal Saved list

### Requirement: Authority ceilings are fixed per mode
Each acquisition mode SHALL carry a fixed authority ceiling: explicit capture and public resolution SHALL never exceed `explicit_user_capture`; own-account sync through the official API MAY reach `authoritative_platform_state`; data export SHALL never exceed `export_observation`; legacy import SHALL never exceed `legacy_observation`. No lookup or conversion SHALL raise a record's authority above its mode's ceiling.

#### Scenario: Only own-account sync reaches authoritative platform state
- **WHEN** the authority ceilings of all five modes are checked
- **THEN** only `OwnAccountSync` carries `authoritative_platform_state`, while `ExplicitCapture` and `PublicResolution` carry `explicit_user_capture`, `DataExport` carries `export_observation`, and `LegacyImport` carries `legacy_observation`

### Requirement: Local vocabularies match the published social-contract grammar value for value
The acquisition-method values this service puts on the wire SHALL equal, value for value, the `AcquisitionMethod` vocabulary of the published `ratatoskr-social-contracts` crate at the revision recorded in the alignment review, and the saved-authority values SHALL equal that crate's `SavedAuthority` vocabulary. Every contract variant SHALL be produced by exactly one local acquisition mode, so nothing the contracts can express is unaccounted for locally.

#### Scenario: The local method and authority sets equal the contract sets
- **WHEN** the local acquisition-method and saved-authority value sets are enumerated and compared against the recorded contract vocabularies
- **THEN** both pairs of sets are equal, and each contract acquisition method belongs to exactly one local mode

### Requirement: Upstream status and preservation state stay separate vocabularies
What Instagram reports about a source (the availability observation) and what Ratatoskr holds locally (the preservation state) SHALL be distinct types with no implicit conversion between them. A seven-value availability observation collapses into the five-value media upstream status by the documented mapping in which `private` never becomes `deleted`, and applying any availability observation to any preservation state SHALL leave the preservation state unchanged: observing deletion upstream never demotes content already preserved.

#### Scenario: Observation collapse stays honest about private sources
- **WHEN** each availability-observation value is collapsed into a media upstream status
- **THEN** `private` yields `unavailable` rather than `deleted`, `temporarily_unavailable` yields `unavailable`, `unsupported` and `resolution_failed` yield `unknown`, and `available`, `unavailable`, and `deleted` yield themselves

#### Scenario: No observation changes what was preserved
- **WHEN** every availability observation is applied to every preservation state
- **THEN** each application leaves the preservation state unchanged, including a deleted-upstream observation against fully preserved content
