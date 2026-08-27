## MODIFIED Requirements

### Requirement: The capability matrix answers for every acquisition mode
The library SHALL expose a total lookup that, for each documented acquisition mode (`ExplicitCapture`, `PublicResolution`, `OwnAccountSync`, `DataExport`, `LegacyImport`), returns an explicit support status (`Supported`, `Planned`, or `NotSupported`), the closed set of wire acquisition-method values the mode produces, and the strongest saved-authority claim the mode is allowed to make. The mode inventory SHALL be exactly these five modes — no hidden sixth lane exists. A mode SHALL report `Planned` until the implementation plan item that builds its lane flips the status with a reviewed test change, and `Supported` from that item onward.

#### Scenario: Each mode resolves to its documented capability
- **WHEN** the capability of each acquisition mode is looked up
- **THEN** every lookup succeeds and reports one explicit support status, exactly the wire method values documented for that mode, and the authority ceiling documented for that mode

#### Scenario: No mode claims support it does not have

- **WHEN** the support statuses of `DataExport` and `LegacyImport` are inspected while their implementing plan items are still open
- **THEN** neither reports `Supported`

#### Scenario: The implemented explicit-capture lane reports support

- **WHEN** the capability of `ExplicitCapture` is looked up after its implementing plan item landed
- **THEN** it reports `Supported` while keeping its documented wire methods (`share_extension`, `browser_extension`) and its `explicit_user_capture` authority ceiling

#### Scenario: The implemented public-resolution lane reports support

- **WHEN** the capability of `PublicResolution` is looked up after its implementing plan item lands
- **THEN** it reports `Supported` while keeping its documented wire method (`public_resolution`) and its `explicit_user_capture` authority ceiling

#### Scenario: The implemented own-account lane reports support

- **WHEN** the capability of `OwnAccountSync` is looked up after own-media synchronization lands
- **THEN** it reports `Supported` while keeping its documented wire method (`official_api`) and its `authoritative_platform_state` ceiling for observed own-media state

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
