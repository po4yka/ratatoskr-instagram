# Instagram connector threat model

## Assets

OAuth credentials, professional account data, captures/notes, private/unavailable references, Data Exports, media artifacts, and user privacy expectations.

## Threats and controls

- **Credential theft/account mix-up:** unpredictable state, exact owner/redirect binding, one-time
  Platform relay claim, optional provider-correct PKCE, and AES-256-GCM envelopes authenticated to
  key version, account/flow identity, and token kind. Secrets are redacted from config and errors.
- **Privacy bypass/hidden scraping:** prohibit passwords/cookies/hidden endpoints; use supported APIs/public resolution and explicit uploads only.
- **Malicious URL/redirect:** strict Instagram URL classification, safe HTTP behavior, no local-network fetch.
- **Archive/zip bomb/path traversal:** authenticate before polling the body; stream/hash under an
  exact body cap into private create-new staging; publish a verified no-overwrite BlobRef; preflight
  central/local headers; reject traversal, absolute/backslash/ambiguous/duplicate names, symlinks,
  encryption and unsupported codecs; enforce entry/count/compressed/decompressed/ratio limits and
  actual emitted-byte counters. Parser reads are bounded and never materialize archive paths.
- **Cross-owner export disclosure:** credentials resolve to one owner, receipt uniqueness is
  owner/digest scoped, and status/report lookup filters by both run and owner with no-store 404s.
- **False completeness/deletion:** reports expose exact overlap/difference sets and a fixed
  disclaimer; absence creates no tombstone, unavailable state, removal fact, or native-Saved claim.
- **Export media execution/expansion:** the raw ZIP is retained, but referenced media is never
  decoded, fetched, executed, or represented as a separately archived media BlobRef.
- **Sensitive media leak:** owner authorization, protected BlobStore, safe events/logs, retention/deletion propagation.
- **False authority:** typed acquisition/saved-authority fields and UI wording.
- **Malicious captions/metadata:** untrusted data, output escaping, no instruction execution.
- **Capability drift:** discovery records account type plus actual permission statuses and replaces a
  total matrix. Authentication failure scrubs credentials and requires reauthorization; transient
  failure preserves the last valid projection rather than inventing a downgrade.
- **Unbounded/racy provider use:** every attempt is committed as `started` before I/O, retries consume
  a new finite-budget ordinal, and only transient discovery calls retry. Raw headers and bodies are
  never stored in the usage ledger or metric labels.
- **Partial or stale own-media authority:** one active staged generation per account, durable cursors,
  completion-only watermarks, and final owner/provider/capability revalidation keep partial or
  downgraded runs from becoming visible. Bounded-prefix absence never infers deletion.
- **Fabricated media backup:** exact raw JSON may carry a content-addressed BlobRef, but expiring CDN
  URLs do not. Events declare partial completeness and empty media attachments until a separate
  reviewed byte-download policy exists. Stories and foreign-owner responses are refused.
- **Incomplete disconnect:** local revoke always deletes credential and live flow material and writes
  a revoked capability generation even when remote revoke is unsupported or fails; startup scrubs
  accounts stranded in `revoking`.

Re-review for messaging, comments/publishing, private media retrieval, automated downloads, or new account types.
