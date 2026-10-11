## Context

The contracts are fixed in XR-021 CONTRACTS.md (R2-03, R2-04, R2-10) and are not redefined here. This document records only the decisions internal to this repository.

## Decisions

### Failure classes live in the error type

`TransportError` is an enum: `Transient(String)` (a broker or acknowledgement failure, retried with backoff) and `Undeliverable(UndeliverableClass)` (a failure no retry can fix). The only class today is `PayloadTooLarge`, rendered `payload_too_large`. The pass stores the class as `last_error`, sets `undeliverable_at = now()` and never selects the row again. The depth gauge counts only publishable rows, so an undeliverable row neither inflates it nor affects readiness.

### The size check is in the transport

`NatsEventTransport::deliver` reads `max_payload` from the connected client's server info and refuses a body longer than `max_payload - 1024` before publishing. The check belongs to the transport because only the transport knows the carrier's limit; the pass only understands the class.

### Pass verdict and the readiness check

`run_once` returns a `PassVerdict`: `Failing` when at least one publish failed and the oldest publishable unpublished row has `attempt_count >= 3` or `occurred_at` older than 300 seconds; `Healthy` when the pass attempted rows and none failed (or the failure threshold is not met), or when no publishable row remains; `Waiting` when nothing was due but publishable rows remain in backoff. The literal contract wording "finds none" is read as "no publishable row exists": a pass that finds only rows waiting out their backoff changes nothing, otherwise a failing relay would flap to ready on every pass between two attempts of a 60 second backoff. `PublisherHealth` is a cloneable atomic flag the relay loop updates from the verdict; `RuntimeState` owns one and reports it as the `bus_publish` check, failing readiness while it is failing. The relay loop moves from the binary into the service library so a test can drive it with a real `RuntimeState`.

### Test broker with authorization

`tests/nats_transport.rs` spawns its own `nats-server` with an authorization block that gives the Instagram user a publish allowlist, runs the relay loop against it, and reloads the configuration (SIGHUP) to restore the omitted subject. CI extracts the `nats-server` binary from the pinned `nats:2-alpine` image the gate already runs, so no new image or download is introduced. A missing binary fails the test.
