# Instagram connector interfaces

## Inbound

The loopback product listener implements:

- `POST /v1/accounts/instagram/oauth/begin` with the authenticated `user_ref`;
- `POST /v1/accounts/instagram/oauth/complete` with `user_ref` and a one-time Platform `relay_id`;
- `POST /v1/accounts/instagram/{account_id}/refresh` and `/revoke` with `user_ref`;
- `GET /v1/accounts/instagram/{account_id}/capabilities?user_ref=...`;
- the existing `POST /v1/captures` explicit-capture intake.

These are internal loopback commands, not a public callback surface. Authorization codes, OAuth
state, and tokens are never accepted in command bodies. Own-media sync, Data Export, and event
commands remain planned.

## Outbound

The official-account adapter calls Meta's fixed Instagram Login HTTPS endpoints and claims the
audience-bound callback relay from Platform. Provider credentials remain local. Account/capability
events and own-media/export flows remain planned.

## Rules

Capture commands include owner, canonical URL, captured time, acquisition method, optional note/collections, operation, and idempotency. Provider credentials remain local. Public resolution uses documented supported interfaces and records method/version. Import commands reference immutable blobs and parser versions. Errors distinguish unsupported capability, auth/reauth, private/unavailable, invalid URL/archive, limits, policy, and transient provider failure.

Generic article extraction is delegated only for external article URLs, not used to bypass Instagram access controls.
