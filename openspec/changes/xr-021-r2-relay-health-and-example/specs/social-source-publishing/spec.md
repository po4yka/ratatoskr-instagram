## ADDED Requirements

### Requirement: A delivery failure is transient or undeliverable and an undeliverable row never blocks the outbox

The transport SHALL refuse an envelope larger than the connected server's `max_payload` minus 1 KiB as undeliverable, and the outbox pass SHALL then mark the row undeliverable at once with the safe error class `payload_too_large`, increment `instagram_outbox_undeliverable_total{class}`, never select the row again, and continue with the next rows. Every other delivery failure SHALL be transient: the row stays unpublished with an attempt count and a backoff and is never deleted or dead-lettered (XR-021 CONTRACTS.md R2-04).

#### Scenario: An oversize envelope is undeliverable and later rows publish

- **WHEN** a pass runs over an oversize row followed by a healthy row
- **THEN** the oversize row has `undeliverable_at` and `last_error` `payload_too_large`, the counter `instagram_outbox_undeliverable_total{class="payload_too_large"}` is one, and the healthy row is published in the same pass

#### Scenario: The transport measures the body against the server limit

- **WHEN** the transport is asked to deliver a body longer than the server's `max_payload` minus 1 KiB
- **THEN** it returns the undeliverable `payload_too_large` error and publishes nothing
