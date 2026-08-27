# Instagram connector interfaces

## Inbound

The loopback product listener implements:

- `POST /v1/accounts/instagram/oauth/begin` with the authenticated `user_ref`;
- `POST /v1/accounts/instagram/oauth/complete` with `user_ref` and a one-time Platform `relay_id`;
- `POST /v1/accounts/instagram/{account_id}/refresh` and `/revoke` with `user_ref`;
- `GET /v1/accounts/instagram/{account_id}/capabilities?user_ref=...`;
- the existing `POST /v1/captures` explicit-capture intake.
- `POST /v1/data-exports` with exact `application/zip` and an owner-bound bearer credential;
- `GET /v1/data-exports/{run_id}` with the same owner credential.

These are internal loopback commands, not a public callback surface. Authorization codes, OAuth
state, and tokens are never accepted in command bodies. Own-media sync is a service-local scheduled
operation with no HTTP command. Data Export receipt returns `202` for a new immutable run and `200`
for an exact same-owner replay; status/report responses are `Cache-Control: no-store`, and unknown
or other-owner run ids both return the same typed `404`.

## Outbound

The official-account adapter calls Meta's fixed Instagram Login HTTPS endpoints and claims the
audience-bound callback relay from Platform. Provider credentials remain local. The own-media
adapter calls only the connected account's versioned `/media` edge with a bearer header, fixed
non-ephemeral fields, and an optional opaque continuation. Accepted completions append normalized
`social.source.captured.v1` / `social.source.updated.v1` facts through the transactional outbox.

## Rules

Capture commands include owner, canonical URL, captured time, acquisition method, optional note/collections, operation, and idempotency. Provider credentials remain local. Public resolution uses documented supported interfaces and records method/version. Import commands reference immutable blobs and parser versions. Errors distinguish unsupported capability, auth/reauth, private/unavailable, invalid URL/archive, limits, policy, and transient provider failure.

Generic article extraction is delegated only for external article URLs, not used to bypass Instagram access controls.

Data Export bearer values never enter bodies, logs, metrics, status responses, or stored config.
The status report exposes sorted gap identities only to its owner and explicitly disclaims complete
account history, native Saved membership, unsave, and deletion authority.
