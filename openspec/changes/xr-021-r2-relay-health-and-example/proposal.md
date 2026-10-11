## Why

Changeset XR-021 round 2 found two operational gaps in Instagram. First, the JetStream relay keeps running and keeps the process ready while the broker refuses every publish (an ACL drift, a refused subject, an unreachable broker), so an operator sees a healthy service whose captures never finish; and an envelope larger than the broker's `max_payload` is refused on every attempt and retried forever. Second, Instagram ships no operator example, so the keys that enable the capture flow (the bus URL, the nkey seed path and the Meta access token path) are discoverable only from `DEVELOPMENT.md` prose. Cross-repository behaviour is defined in XR-021 CONTRACTS.md R2-03 (publisher health reaches readiness), R2-04 (outbox failure classes and the broker payload limit) and R2-10 (operator examples are shipped, loadable and name the right variables); this change cites them and does not restate them.

## What Changes

- The outbox pass classes a delivery failure as transient or undeliverable. A transient failure keeps the row with a backoff as today. An envelope larger than the connected server's `max_payload` minus 1 KiB is undeliverable: the row gets `undeliverable_at` and the safe `last_error` class `payload_too_large` at once, `instagram_outbox_undeliverable_total{class}` is incremented, and the pass continues with the next rows (R2-04).
- The outbox pass reports a verdict (healthy, failing, waiting) and a `PublisherHealth` flag fed by the relay loop becomes a named `bus_publish` readiness check: failing when the last pass attempted a publish, at least one failed, and the oldest publishable unpublished row has `attempt_count >= 3` or is older than 300 seconds. The relay is never stopped by it (R2-03).
- **BREAKING** `TransportError` becomes an enum (`Transient`, `Undeliverable`) instead of a tuple struct, and every call site is updated. `schema.sql` gains `outbox_events.undeliverable_at` in place (no migration); the unpublished index and every selection exclude undeliverable rows.
- Ship `deploy/systemd/instagram.conf.example`, loaded by a test through the real configuration loader, and point `DEVELOPMENT.md` at it, stating that a Meta app token with oEmbed access is an external prerequisite (R2-10).

## Capabilities

### New Capabilities

None; every requirement lands in an existing capability.

### Modified Capabilities

- `social-source-publishing`: failure classes and the undeliverable terminal state.
- `service-runtime`: the `bus_publish` readiness check and the shipped operator example.

## Impact

- `crates/instagram-archive`: `publishing.rs` (the publisher pass moves to `publishing/pump.rs`, the health type to `publishing/health.rs`), `lib.rs` re-exports, `schema.sql`, `tests/outbox_backoff.rs`.
- `services/instagram-archive`: `nats_transport.rs`, `lib.rs` (`RuntimeState` and `CheckName`), a new `relay.rs` extracted from `bus.rs`, `tests/nats_transport.rs`, `tests/boot.rs`.
- New `deploy/systemd/instagram.conf.example`, `DEVELOPMENT.md`, `.github/workflows/ci.yml` (the authorization-enabled broker test needs a `nats-server` binary).
- Out of scope: calibrating `classify_response` against live Meta responses, the capture lifecycle, OAuth and data-export lanes, removal events for provider-side deletion.
