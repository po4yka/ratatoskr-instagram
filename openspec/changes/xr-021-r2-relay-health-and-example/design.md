## Context

The contracts are fixed in XR-021 CONTRACTS.md (R2-03, R2-04, R2-10) and are not redefined here. This document records only the decisions internal to this repository.

## Decisions

### Failure classes live in the error type

`TransportError` is an enum: `Transient(String)` (a broker or acknowledgement failure, retried with backoff) and `Undeliverable(UndeliverableClass)` (a failure no retry can fix). The only class today is `PayloadTooLarge`, rendered `payload_too_large`. The pass stores the class as `last_error`, sets `undeliverable_at = now()` and never selects the row again. The depth gauge counts only publishable rows, so an undeliverable row neither inflates it nor affects readiness.

### The size check is in the transport

`NatsEventTransport::deliver` reads `max_payload` from the connected client's server info and refuses a body longer than `max_payload - 1024` before publishing. The check belongs to the transport because only the transport knows the carrier's limit; the pass only understands the class.

### Pass verdict and the readiness check

`run_once` returns a `PassVerdict` after the pass. The oldest publishable unpublished row is "stuck" when its `attempt_count >= 3` or its `occurred_at` is older than 300 seconds. `Failing`: at least one publish failed in this pass and the oldest row is stuck. `Healthy`: no publishable row remains, or the oldest row is not stuck. `Waiting`: no publish failed in this pass but the oldest row is stuck and waiting out its backoff; the previous verdict stands. The literal contract wording "a pass that publishes every due row or finds none" is read with that last case in mind: a failing head row is not due between its attempts (a 60 second backoff), so treating every such pass as clean would flap readiness to 200 on every pass between two attempts and would hide a subject-specific ACL drift whenever any other subject still publishes. A pass that fails and finds the oldest row not yet stuck is healthy, which is the contract's tolerance of the first two attempts.

`PublisherHealth` is a cloneable atomic flag the relay loop updates from the verdict; `RuntimeState` owns one, reports it as the `bus_publish` check (reason `publish_failing`) beside the `bus` check, and fails readiness while it is failing. A failing pass increments `instagram_outbox_publisher_failing_total{class="publish_failing"}`. The relay loop moves from the binary into the service library so a test can drive it with a real `RuntimeState`.

### Test broker with authorization

`tests/nats_transport.rs` spawns its own `nats-server` with an authorization block that gives the Instagram user a publish allowlist, runs the relay loop against it, and reloads the configuration (SIGHUP) to restore the omitted subject. CI extracts the `nats-server` binary from the pinned `nats:2-alpine` image the gate already runs, so no new image or download is introduced. A missing binary fails the test.
