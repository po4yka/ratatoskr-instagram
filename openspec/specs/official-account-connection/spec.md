# official-account-connection Specification

## Purpose

Establishes a truthful, encrypted official Instagram professional-account connection whose current permissions and provider usage are safe inputs to later account-owned features.

## Requirements

### Requirement: OAuth authorization is bound, least-privilege, and single-use

The system SHALL begin official account authorization with unpredictable state bound to the Ratatoskr owner and configured redirect target, SHALL use PKCE when the selected Meta flow supports it, SHALL request only the read permissions required for account discovery, and SHALL complete authorization only for the matching unexpired owner-bound state exactly once. It MUST reject unknown, expired, mismatched, and replayed state without storing a credential.

#### Scenario: Authorization begin is owner-bound and least-privilege
- **WHEN** an authenticated owner begins an official Instagram account connection
- **THEN** the returned provider authorization URL contains fresh state, the exact configured redirect target, PKCE parameters when supported, and only the documented account-discovery read permissions

#### Scenario: Invalid callback state persists nothing
- **WHEN** authorization completion presents unknown, expired, owner-mismatched, redirect-mismatched, or already-consumed state
- **THEN** completion is refused with a typed error and no account, credential, capability, or audit record is added or changed

#### Scenario: Successful callback consumes state once
- **WHEN** the provider returns an authorization code for matching unexpired owner-bound state and the exchange and discovery calls succeed
- **THEN** the state becomes unusable, one connected account is recorded with the provider-observed identity and type, and repeating the callback cannot exchange the code or change stored state

### Requirement: Provider credentials are authenticated-encrypted at rest

The system SHALL seal every access or refresh token with authenticated encryption before persistence, SHALL record the key version needed to select rotation material, and SHALL never persist, serialize, emit, or log credential plaintext. Decryption MUST reject unknown key versions, malformed envelopes, wrong keys, and tampering without returning plaintext.

#### Scenario: Token encryption round-trip recovers the original
- **WHEN** synthetic token material is sealed under one configured key version and opened under the same version and key
- **THEN** the recovered bytes equal the original token while the sealed bytes do not

#### Scenario: Equal tokens do not reveal equality at rest
- **WHEN** identical token material is sealed twice under the same configured key version
- **THEN** the two stored envelopes differ

#### Scenario: Tampered token envelope is refused
- **WHEN** any authenticated part of a stored token envelope is changed before opening
- **THEN** opening fails with a typed redacted error and returns no token bytes

#### Scenario: Raw credential storage contains no plaintext
- **WHEN** a completed connection's owned database rows are inspected directly
- **THEN** no column contains the access token, refresh token, authorization code, raw OAuth state, or a diagnostic rendering of any of them

### Requirement: Refresh revalidates permissions and degrades safely

The system SHALL refresh a connection only through the documented strategy supported by the selected provider flow, SHALL atomically replace encrypted token material on success, and SHALL repeat account-type and granted-permission discovery before reporting refreshed capabilities. When refresh or provider authentication establishes that authorization is no longer usable, the connection SHALL become `reauthorization_required` and no stale capability SHALL remain available.

#### Scenario: Successful refresh replaces tokens and capabilities together
- **WHEN** a connected account refreshes successfully and the provider reports a changed expiry or permission set
- **THEN** the new encrypted credential, expiry, observed permissions, reconciled capabilities, usage entries, and refresh audit record commit together

#### Scenario: Permission downgrade removes stale capability
- **WHEN** refresh discovery no longer reports a permission that previously made a capability available
- **THEN** reconciliation records that capability as unavailable for the missing permission and no caller can exercise its former available state

#### Scenario: Refresh authentication failure requires reauthorization
- **WHEN** the provider rejects the current refresh strategy as unauthorized or revoked
- **THEN** the connection becomes `reauthorization_required`, its capabilities are unavailable, and the error, telemetry, and audit data contain no token material

### Requirement: Revocation scrubs every recoverable credential

The system SHALL mark the account revoked, delete all access and refresh token envelopes and live OAuth flow secrets owned by that account, make every reconciled capability unavailable, and append a redacted revocation audit record in one local transaction. It SHALL attempt provider-side revocation only when the selected official flow documents that operation, but local scrubbing MUST complete even when that attempt is unsupported or unavailable.

#### Scenario: Revocation leaves no recoverable token
- **WHEN** a connected account is revoked and every owned table is inspected after the transaction commits
- **THEN** the account is `revoked`, no credential or live flow secret remains decryptable or recoverable, every account capability is unavailable, and one revocation audit record remains without secret material

#### Scenario: Provider revoke failure cannot retain local secrets
- **WHEN** a documented provider-side revoke attempt fails or the selected flow exposes no revoke operation
- **THEN** local revocation still succeeds completely and records only the redacted provider-revoke outcome for operators

### Requirement: Official provider calls consume a finite accounted budget

The system SHALL enforce a finite call-attempt budget for each connection, refresh, capability-discovery, and revoke operation before issuing official provider requests. Every attempted provider call, including failed and retry attempts, MUST consume one unit and produce a durable redacted usage entry naming the operation class and outcome; an exhausted budget MUST prevent another network request.

#### Scenario: Successful connection accounts for all provider calls
- **WHEN** an OAuth completion exchanges a code and performs account and permission discovery
- **THEN** the operation's usage entries and consumed-unit total equal the provider requests actually attempted

#### Scenario: Failed attempts still consume budget
- **WHEN** a provider request fails after it is issued and policy permits a retry
- **THEN** both the failed attempt and retry are recorded and counted against the same finite operation budget

#### Scenario: Exhausted budget performs no request
- **WHEN** an operation has consumed its configured provider-call budget and asks to issue another call
- **THEN** it fails with a typed budget-exhausted result, no network request occurs, and no unconsumed usage unit is recorded

### Requirement: Credential and provider diagnostics remain redacted

The system SHALL exclude access tokens, refresh tokens, authorization codes, OAuth state, PKCE verifiers, client secrets, usernames, captions, and full provider URLs from ordinary logs, metrics, public errors, audit details, and usage records.

#### Scenario: Failure diagnostics contain no supplied secrets
- **WHEN** authorization, refresh, discovery, or revocation fails after receiving synthetic secret values
- **THEN** captured logs, metrics labels, public errors, audit details, and usage rows contain none of those values or their URL-encoded forms

### Requirement: Own-media jobs use current owner-bound authorization

The system SHALL execute own-media work only with the encrypted credential owned by the targeted account and Ratatoskr user, after verifying that the account is connected and its current complete capability generation makes `own_media_read` available. Finalization MUST revalidate the same ownership, connection, and capability evidence; any downgrade or reauthorization transition during a run SHALL prevent authority and watermark replacement.

#### Scenario: One account credential cannot read another account

- **WHEN** scheduled work names an account and owner that do not match the stored credential binding
- **THEN** the job is refused before provider contact and no other account's token is opened

#### Scenario: Mid-run downgrade blocks completion

- **WHEN** permission reconciliation replaces the run's available capability generation before finalization
- **THEN** the staged run cannot replace the current own-media authority or watermark and records the new capability limit
