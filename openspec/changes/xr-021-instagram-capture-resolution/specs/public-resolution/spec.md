## ADDED Requirements

### Requirement: The HTTP public surface classifies provider answers in one pure function

The service SHALL resolve permalinks through Meta's `instagram_oembed` Graph endpoint with a request carrying the canonical permalink and an app access token, without following redirects, with a bounded connect and total timeout, and with a response body capped at 256 KiB (XR-021 CONTRACTS.md S10 CD5). One pure function SHALL map status and body to a surface outcome: a 200 JSON object is a payload, 404 is deleted, 403 is private, 400 is unsupported, and 429 and 5xx are temporarily unavailable. A transport failure or an oversized body SHALL be a transport failure. Deleted SHALL never be produced for a 5xx or a timeout. The access token SHALL never appear in `Debug` output or any error text.

#### Scenario: The status table classifies every documented answer

- **WHEN** the classifier receives 200 with an object, 200 with a non-object, 404, 403, 400, 429, 500 and 503
- **THEN** the outcomes are payload, unavailable-as-malformed, deleted, private, unsupported, temporarily unavailable, temporarily unavailable and temporarily unavailable

#### Scenario: The request is bounded and secret-safe

- **WHEN** the surface fetches a permalink from a mock server that redirects or returns an oversized body
- **THEN** no redirect is followed, the oversized body is a transport failure, and the token is absent from every rendered error and `Debug` string

### Requirement: Public resolution configuration fails closed

When the bus is configured the service SHALL also require `RATATOSKR__PUBLIC_RESOLUTION__ACCESS_TOKEN_PATH`, read once at startup into a secret, and SHALL refuse to start with exit code 78 otherwise (XR-021 CONTRACTS.md S02 rule 5). The endpoint SHALL be `https` on `graph.facebook.com` or `graph.instagram.com`.

#### Scenario: A bus without a resolution surface is refused

- **WHEN** configuration sets the bus URL and no access token path
- **THEN** loading fails with a violation naming `RATATOSKR__PUBLIC_RESOLUTION__ACCESS_TOKEN_PATH`

#### Scenario: A non-Graph endpoint is refused

- **WHEN** the endpoint names another host or uses `http://`
- **THEN** loading fails with a violation naming the endpoint key and rendering no supplied value

### Requirement: Captures resolve with bounded retry and one terminal report per operation

A resolution worker SHALL claim due captures that have an unreported operation, resolve each once per attempt through the public surface, and report every pending operation of a concluded capture exactly once (XR-021 CONTRACTS.md S10 CD2). A transient outcome before the configured maximum number of attempts SHALL only schedule the next attempt 30 seconds, 2 minutes, 8 minutes and 30 minutes after the first four failures, recording no observation and no terminal state. The fifth transient outcome SHALL conclude the capture as unavailable and report a retryable failure. A deleted outcome SHALL report a non-retryable `social.source.deleted` failure; private and unsupported outcomes SHALL report a non-retryable `social.source.unavailable` failure. A resolved capture SHALL report success naming the source identity derived from the owner and permalink. Reporting SHALL set `reported_at` in the statement that guards the outbox insert, so a rerun or a crash between resolution and reporting reports without a second fetch.

#### Scenario: A payload preserves and reports once

- **WHEN** the worker runs against a due capture and the surface answers with a payload
- **THEN** the capture is `resolved`, one captured fact and one succeeded report naming `social_source:<identity>` exist, `reported_at` is set, and a second run adds nothing

#### Scenario: Permanent failures stop immediately

- **WHEN** the surface answers deleted, then private or unsupported for other captures
- **THEN** each operation receives one non-retryable failed report with the matching code, the capture is `unavailable`, and no captured fact exists

#### Scenario: Transient failures back off then terminate

- **WHEN** the surface is temporarily unavailable four times and then answers with a payload
- **THEN** no report exists after the first four runs, `next_resolution_at` advances 30 seconds, 2 minutes, 8 minutes and 30 minutes, and the fifth run reports success

#### Scenario: Five transient failures end retryably

- **WHEN** the surface is temporarily unavailable five times
- **THEN** one retryable `social.source.unavailable` failure is reported and one `temporarily_unavailable` observation exists

#### Scenario: Two operations share one resolution

- **WHEN** two operations name one permalink
- **THEN** one fetch happens and each operation receives its own succeeded report

#### Scenario: A crash after resolution only reports

- **WHEN** a capture is already resolved but its operation is unreported
- **THEN** the next run reports success and performs no fetch
