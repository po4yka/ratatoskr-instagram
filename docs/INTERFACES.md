# Instagram connector interfaces

## Inbound

OAuth connect/callback/refresh/revoke, capability refresh, own-media sync, explicit capture resolve, Data Export import, re-resolve, delete/privacy, and operation commands.

## Outbound

Account/capability/media/capture/export/social-source/upstream-status events, Knowledge indexing triggers, and safe operation progress/results.

## Rules

Capture commands include owner, canonical URL, captured time, acquisition method, optional note/collections, operation, and idempotency. Provider credentials remain local. Public resolution uses documented supported interfaces and records method/version. Import commands reference immutable blobs and parser versions. Errors distinguish unsupported capability, auth/reauth, private/unavailable, invalid URL/archive, limits, policy, and transient provider failure.

Generic article extraction is delegated only for external article URLs, not used to bypass Instagram access controls.
