## MODIFIED Requirements

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
