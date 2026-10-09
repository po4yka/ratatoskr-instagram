## Why

Changeset XR-021 found that an explicit Instagram browser capture never finishes from Platform's point of view. `ingest_browser_capture_command` drops the Platform operation id, nothing implements `PublicSurface` outside test fakes, `services/instagram-archive/src/main.rs` publishes through a logging-only transport that returns `Ok` and marks every outbox row published, and the NATS ACL gave the identity no `evt.*` publish right. A user captures a reel and the operation stays `accepted` until Platform's stale reaper fails it. Cross-repository behaviour is defined in XR-021 CONTRACTS.md sections S10 (CD1, CD2, CD5, CD7 and the social fact envelope paragraph), S02 and S03; this change cites them and does not restate them.

## What Changes

- Track the Platform operation at intake: a new `capture_operations` table (one capture can carry several operations because captures dedupe on `(user_ref, canonical_url)`), `captures.resolution_attempts`, and a queued `platform.operation.reported.v1` report committed in the same transaction as the capture and the inbox claim (S10 CD1, CD2, CD7).
- Add a real `PublicSurface` over Meta's `instagram_oembed` Graph endpoint with a pure `classify_response`, redirects off, a body cap, a host allowlist and a secret-safe access token, plus fail-closed `public_resolution` configuration that is required whenever the bus is configured (S10 CD5, S02 rule 5).
- Add a capture resolution worker (`CaptureResolver::run_due_once`) with bounded retry (30 s, 2 min, 8 min, 30 min, terminal on the fifth transient failure) and exactly one terminal report per operation, guarded by a nullable `reported_at` set in the same statement that inserts the report (S10 CD2).
- **BREAKING** Replace `LoggingTransport` with a JetStream `NatsEventTransport` (closed subject allowlist, `Nats-Msg-Id`, ack-before-published). Without a configured bus no publisher starts and rows accumulate unpublished instead of being marked published. The outbox pass now honours `next_attempt_at` so a failing head row cannot starve later rows (S02 rules 2 to 5).
- **BREAKING** The process refuses to start (exit 78) when a bus is configured and `public_resolution` is not, and exits non-zero when a bus task stops (S02 rule 5).
- **BREAKING** `schema.sql` changes in place (no migration): `capture_operations`, `captures.resolution_attempts`, an `event_type` CHECK on `outbox_events`. Development databases are recreated.

## Capabilities

### New Capabilities

None; every requirement lands in an existing capability.

### Modified Capabilities

- `capture-intake`: the operation is recorded and a queued report is emitted at intake.
- `public-resolution`: the HTTP oEmbed surface, the classifier, and the retrying resolution worker with exactly-once terminal reports.
- `social-source-publishing`: the JetStream relay replaces the logging transport and honours backoff.
- `service-runtime`: fail-closed `public_resolution` configuration and bus-task lifecycle.

## Impact

- `crates/instagram-archive`: `command_capture.rs`, new `publishing/operation_report.rs`, new `public_surface.rs`, new `capture_resolution.rs`, `resolution.rs` split, `config.rs`, `publishing.rs`, `schema.sql`, new tests.
- `services/instagram-archive`: new `nats_transport.rs`, `main.rs` wiring, `command_consumer.rs`.
- Dependencies: `ratatoskr-operation-contracts` becomes a direct dependency, `wiremock` a dev-dependency, all `ratatoskr-*` pins move to contracts commit `ad16855c4e7f3d52cd118274faa3b8f3ab4da576`.
- Out of scope: Meta OAuth callback relay, the Data Export lane, Knowledge ingestion, erasure for `capture_operations`, and any change to the authorized lanes' shared rows.
