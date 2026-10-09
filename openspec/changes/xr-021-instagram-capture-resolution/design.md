## Context

The contracts are fixed in XR-021 CONTRACTS.md and are not redefined here. This document records only the decisions internal to this repository.

## Decisions

### One operation row per Platform operation

Captures dedupe on `(user_ref, canonical_url)`, so a second explicit capture of one permalink reuses the capture. `capture_operations(operation_id pk, command_id unique, capture_id, user_ref, reported_at)` keeps one row per operation. The terminal report of an operation is guarded by `UPDATE capture_operations SET reported_at = now() WHERE operation_id = $1 AND reported_at IS NULL RETURNING`, and the outbox insert happens only when a row comes back, so a redelivery or a crash between resolution and reporting only reports.

### Resolution state machine

A capture becomes due when it has at least one unreported operation and `next_resolution_at <= now`. The resolver claims due captures with `FOR UPDATE SKIP LOCKED` and moves `next_resolution_at` forward by a 120 s lease inside the claim transaction, so a crashed worker's capture becomes due again without a second worker taking it meanwhile.

`resolve_capture_permalink` is split into fetch plus `apply_surface_outcome(capture_id, permalink, outcome, resolved_at)`; behaviour for the existing callers (re-resolution jobs and tests) is unchanged. The worker uses `apply_surface_outcome` only for outcomes that conclude the capture. A transient outcome before `max_attempts` bumps `resolution_attempts`, sets `next_resolution_at = now + min(30 s * 4^(n-1), 30 min)` and records nothing else: no availability observation, no status change. The fifth transient outcome is applied, which records one `temporarily_unavailable` observation and concludes the capture as `unavailable`.

The report for a concluded capture is derived from stored state, never from the in-memory outcome, so a crash simulation (capture already concluded, operation unreported) reports without a second fetch: `resolved` maps to preserved with `source_identity(user_ref, canonical_url)`, and `unavailable` maps by the latest observation kind (deleted to Deleted; private, unsupported, resolution_failed to Inaccessible; temporarily_unavailable to Transient).

A new explicit capture on an `unavailable` capture resets it to `accepted` with zero attempts and `next_resolution_at = now()`, because a deleted or private post needs a fresh acquisition. On a `resolved` capture the new operation is reported preserved without refetching.

### Public surface

`HttpPublicSurface` calls `GET {endpoint}?url=<canonical permalink>&access_token=<token>` through reqwest with redirects disabled, a 3 s connect and 10 s total timeout and a 256 KiB bounded body read. `classify_response(status, body)` is a pure function holding the status table so that a correction against live evidence is a one-line change. The token is a `SecretString`, redacted in `Debug`, and errors never embed the request URL. The endpoint must be `https` on `graph.facebook.com` or `graph.instagram.com`.

The access token travels as the documented `access_token` query parameter. Meta's current documentation for the `instagram_oembed` endpoint specifies an app access token or client token in that parameter; the endpoint version is configurable through `ENDPOINT`.

### Transport and lifecycle

`NatsEventTransport` publishes to `evt.<event_type>` for a closed list of four types, with `Nats-Msg-Id` equal to the event id, and returns `Ok` only after the JetStream acknowledgement. An unlisted type is a `TransportError`, and the schema CHECK makes it impossible to insert one. The outbox pass selects `published_at is null and (next_attempt_at is null or next_attempt_at <= now())` ordered by `occurred_at, event_id`. The service shares one NATS client between the command consumer and the transport. The publisher starts only when a bus is configured. Any bus task that returns before an orderly shutdown flips readiness and ends the process with a non-zero code.

## Risks

- The classifier status table is a starting point from documentation, not recorded live fixtures. It is isolated in one function.
- `outbox_events.event_type` gets a CHECK, which makes every future event type a schema edit; this is deliberate (S02 rule 2).
