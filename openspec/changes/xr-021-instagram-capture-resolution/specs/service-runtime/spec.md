## ADDED Requirements

### Requirement: A configured bus requires the resolution surface and a live bus lifecycle

The process SHALL refuse to start when the bus is configured and the public resolution surface is not, and SHALL leave readiness and exit with a non-zero code when a bus task stops before an orderly shutdown (XR-021 CONTRACTS.md S02 rule 5).

#### Scenario: A bus without a resolution surface does not start

- **WHEN** `check-config` runs with the bus URL and no resolution token path
- **THEN** the exit code is 78 and the report names the missing key
