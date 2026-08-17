# Instagram connector threat model

## Assets

OAuth credentials, professional account data, captures/notes, private/unavailable references, Data Exports, media artifacts, and user privacy expectations.

## Threats and controls

- **Credential theft/account mix-up:** PKCE/state, encrypted least-privilege tokens, exact binding, refresh/revoke.
- **Privacy bypass/hidden scraping:** prohibit passwords/cookies/hidden endpoints; use supported APIs/public resolution and explicit uploads only.
- **Malicious URL/redirect:** strict Instagram URL classification, safe HTTP behavior, no local-network fetch.
- **Archive/zip bomb/path traversal:** immutable raw blob, file/count/size/decompression/path limits, isolated parsing.
- **Sensitive media leak:** owner authorization, protected BlobStore, safe events/logs, retention/deletion propagation.
- **False authority:** typed acquisition/saved-authority fields and UI wording.
- **Malicious captions/metadata:** untrusted data, output escaping, no instruction execution.
- **Capability drift:** runtime capability checks and graceful degradation.

Re-review for messaging, comments/publishing, private media retrieval, automated downloads, or new account types.
