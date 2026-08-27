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

- **WHEN** the support status of `LegacyImport` is inspected while its implementing plan item is still open
- **THEN** it does not report `Supported`

#### Scenario: The implemented explicit-capture lane reports support

- **WHEN** the capability of `ExplicitCapture` is looked up after its implementing plan item landed
- **THEN** it reports `Supported` while keeping its documented wire methods (`share_extension`, `browser_extension`) and its `explicit_user_capture` authority ceiling

#### Scenario: The implemented public-resolution lane reports support

- **WHEN** the capability of `PublicResolution` is looked up after its implementing plan item lands
- **THEN** it reports `Supported` while keeping its documented wire method (`public_resolution`) and its `explicit_user_capture` authority ceiling

#### Scenario: The implemented own-account lane reports support

- **WHEN** the capability of `OwnAccountSync` is looked up after own-media synchronization lands
- **THEN** it reports `Supported` while keeping its documented wire method (`official_api`) and its `authoritative_platform_state` ceiling for observed own-media state

#### Scenario: The implemented Data Export lane reports support

- **WHEN** the capability of `DataExport` is looked up after immutable receipt, hostile inspection, versioned parsing, reconciliation, and completeness reporting land
- **THEN** it reports `Supported` while keeping its documented wire method (`data_export`) and its `export_observation` authority ceiling

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

### Requirement: Connected-account capabilities reconcile observed account facts

The system SHALL derive each connected account's capability state from the latest provider-observed account type, granted-permission statuses, explicit external-write consent, and the repository's closed capability rules. Reconciliation MUST replace the previous result as one complete matrix, MUST record why every capability is available or unavailable, and MUST never infer a grant from the requested scope set, the account type alone, a legacy connection, or another account. Native Saved-list access SHALL remain not supported. A supported `OwnAccountSync` lane SHALL NOT authorize work for a specific account unless that account's current `own_media_read` capability is available.

#### Scenario: Professional account with granted basic permission exposes only read capabilities
- **WHEN** discovery reports a business or creator account with the documented basic read permission granted and no external-write consent
- **THEN** the account identity and own-media-read capabilities are available, write capabilities are unavailable with their missing-permission or consent reason, and native Saved-list access is not supported

#### Scenario: Requested but declined permission is unavailable
- **WHEN** discovery reports a requested permission as declined, expired, or absent
- **THEN** every capability requiring that permission is unavailable with that observed reason rather than assumed from the requested scopes

#### Scenario: Reconciliation replaces prior matrix state
- **WHEN** a later discovery changes the observed account type or permission statuses
- **THEN** one reconciliation replaces every prior capability row for that account so no formerly available capability survives without current evidence

#### Scenario: Unsupported account type grants no professional capability
- **WHEN** discovery reports a personal or unknown account type
- **THEN** every professional-account capability is unavailable with an account-type reason and no official lane claims support from that observation

#### Scenario: One account observation cannot authorize another
- **WHEN** two connected accounts have different observed permissions and their matrices are reconciled
- **THEN** each account's results depend only on its own latest observations and neither receives a capability granted only to the other
