# Instagram connector threat model

## Assets

OAuth credentials, professional account data, captures/notes, private/unavailable references, Data Exports, media artifacts, and user privacy expectations.

## Threats and controls

- **Credential theft/account mix-up:** unpredictable state, exact owner/redirect binding, one-time
  Platform relay claim, optional provider-correct PKCE, and AES-256-GCM envelopes authenticated to
  key version, account/flow identity, and token kind. Secrets are redacted from config and errors.
- **Privacy bypass/hidden scraping:** prohibit passwords/cookies/hidden endpoints; use supported APIs/public resolution and explicit uploads only.
- **Malicious URL/redirect:** strict Instagram URL classification, safe HTTP behavior, no local-network fetch.
- **Archive/zip bomb/path traversal:** immutable raw blob, file/count/size/decompression/path limits, isolated parsing.
- **Sensitive media leak:** owner authorization, protected BlobStore, safe events/logs, retention/deletion propagation.
- **False authority:** typed acquisition/saved-authority fields and UI wording.
- **Malicious captions/metadata:** untrusted data, output escaping, no instruction execution.
- **Capability drift:** discovery records account type plus actual permission statuses and replaces a
  total matrix. Authentication failure scrubs credentials and requires reauthorization; transient
  failure preserves the last valid projection rather than inventing a downgrade.
- **Unbounded/racy provider use:** every attempt is committed as `started` before I/O, retries consume
  a new finite-budget ordinal, and only transient discovery calls retry. Raw headers and bodies are
  never stored in the usage ledger or metric labels.
- **Incomplete disconnect:** local revoke always deletes credential and live flow material and writes
  a revoked capability generation even when remote revoke is unsupported or fails; startup scrubs
  accounts stranded in `revoking`.

Re-review for messaging, comments/publishing, private media retrieval, automated downloads, or new account types.
