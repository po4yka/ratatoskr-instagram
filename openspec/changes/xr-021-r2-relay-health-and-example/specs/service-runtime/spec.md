## ADDED Requirements

### Requirement: Publisher health reaches readiness without stopping the relay

The service SHALL report a `bus_publish` readiness check that fails when the last outbox pass attempted a publish, at least one publish failed, and the oldest publishable unpublished row has `attempt_count >= 3` or is older than 300 seconds, and that passes again after a pass that publishes every due row or finds no publishable row. The relay SHALL keep running and the process SHALL NOT exit because of it; an undeliverable row SHALL NOT fail the check (XR-021 CONTRACTS.md R2-03, R2-04).

#### Scenario: A refused subject flips readiness until a clean pass

- **WHEN** the broker refuses publishes on the subject of the head row and its third attempt fails
- **THEN** `/health/ready` answers 503 with a failing `bus_publish` check, the relay is still running, and after the broker permits the subject and the row is delivered `/health/ready` answers 200

### Requirement: The service ships a loadable operator example

The repository SHALL ship `deploy/systemd/instagram.conf.example` that sets every key the capture flow needs, with the operator listener `127.0.0.1:9082`, the product API `127.0.0.1:9083`, the bus URL, the nkey seed path `/etc/ratatoskr/instagram.nkey` under `RATATOSKR__BUS__NKEY_SEED_PATH`, and the public resolution keys including `ACCESS_TOKEN_PATH`, and a test SHALL load it through the configuration loader (XR-021 CONTRACTS.md R2-10).

#### Scenario: The shipped example loads

- **WHEN** the example is loaded with temporary files substituted for its absolute secret paths
- **THEN** the configuration is valid and carries the documented listeners, bus keys and public resolution values
