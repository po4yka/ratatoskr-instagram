## ADDED Requirements

### Requirement: Connected-account capabilities reconcile observed account facts

The system SHALL derive each connected account's capability state from the latest provider-observed account type, granted-permission statuses, explicit external-write consent, and the repository's closed capability rules. Reconciliation MUST replace the previous result as one complete matrix, MUST record why every capability is available or unavailable, and MUST never infer a grant from the requested scope set, the account type alone, a legacy connection, or another account. Native Saved-list access SHALL remain not supported, and the acquisition lane `OwnAccountSync` SHALL remain planned until item 7 implements synchronization.

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
