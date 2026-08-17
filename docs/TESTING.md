# Instagram connector testing strategy

Required tests:

- OAuth/account binding, encrypted credentials, refresh/revoke, scopes, and capability drift.
- URL classification/canonicalization and malicious URL cases.
- Explicit Share/Browser capture idempotency and provenance.
- Public post/reel resolution, deleted/private/unsupported/expired responses, unknown fields.
- Media size/type/authorization/retention behavior.
- Safe Data Export import: schema detection, parser versions, zip bomb/path traversal, unknown records, duplicate archives, partial assets.
- Missing-data versus deletion semantics.
- SQL migrations, outbox/inbox redelivery, privacy deletion, no-secret/content logging.
- Workspace mobile/extension capture -> Instagram -> Knowledge flow.

Fixtures are synthetic or user-authorized and scrubbed; no personal account is required in CI.
