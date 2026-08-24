# service-runtime

## Purpose

Defines the deployable-process contract of `ratatoskr-instagram`: how configuration enters the process, what it prints, which endpoints an operator can probe, and how the process starts and stops. Every later Instagram capability runs inside this contract unchanged.

## ADDED Requirements

### Requirement: Configuration loads from typed environment variables
The service SHALL read its entire runtime configuration from environment variables prefixed `RATATOSKR__`, with `__` separating nesting levels, into a finite typed structure. An empty environment SHALL yield a valid configuration whose admin listener binds to a loopback address on the documented default port. A variable under the prefix naming an unknown key SHALL be a configuration error, never silently ignored.

#### Scenario: Empty environment yields valid defaults
- **WHEN** the service loads configuration in an environment with no `RATATOSKR__` variables
- **THEN** loading succeeds, the admin listen address is loopback on the default port, and the database URL is unset

#### Scenario: Unknown prefixed key is refused
- **WHEN** the environment contains `RATATOSKR__NOT_A_REAL_SECTION__VALUE=1`
- **THEN** configuration loading fails naming the offending key

### Requirement: Invalid configuration refuses startup without echoing values
When configuration violates any validation rule, the service SHALL refuse to start, report every violation found in one report written to standard error before any log subscriber exists, and render none of the supplied values in that report. The standalone `check-config` invocation SHALL validate configuration without binding any listener, exiting 0 when valid and 78 when invalid.

#### Scenario: Multiple violations are reported together
- **WHEN** the environment sets two independent invalid values (a non-loopback admin bind and a zero connection limit) and the service runs `check-config`
- **THEN** the exit code is 78, the stderr report names both violated rules, and neither supplied value appears in the report

#### Scenario: Valid configuration validates without binding
- **WHEN** the environment holds only valid values and the service runs `check-config`
- **THEN** the exit code is 0 and no listener is opened

### Requirement: Structured telemetry initializes before listeners bind
The service SHALL install a JSON-formatted structured log pipeline filtered by a configurable filter expression, and SHALL refuse to start if telemetry cannot be initialized. Startup SHALL emit one structured record carrying the service identity (name, version, git SHA) and the effective, secret-free configuration view.

#### Scenario: Startup emits identity record as JSON
- **WHEN** the service starts with a valid configuration
- **THEN** the first log records parse as JSON and include the service name, crate version, and git SHA fields

### Requirement: Liveness endpoint answers independently of dependencies
The admin plane SHALL serve `GET /health/live` with HTTP 200 and a body stating liveness, from listener-bind until process exit including throughout shutdown drain. It MUST NOT consult the database or any other dependency.

#### Scenario: Liveness stays green while draining
- **WHEN** the process has received a shutdown signal but still serves requests
- **THEN** `GET /health/live` answers 200

### Requirement: Readiness endpoint reports computed checks
The admin plane SHALL serve `GET /health/ready` with 503 until startup completes, 200 once every configured listener is bound, and 503 again once draining begins. The body SHALL enumerate named checks in stable order — drain, startup, and database exactly when a database URL is configured — each pass/fail with a reason when failing. A failing database check SHALL be visible in the body without by itself flipping readiness away from 200. The readiness computation MUST NOT open a database connection during the request.

#### Scenario: Not ready before listeners are up
- **WHEN** the process has loaded configuration but not yet bound its listeners, and a client calls `GET /health/ready`
- **THEN** the answer is 503 with a failing startup check

#### Scenario: Ready after startup with a reachable database
- **WHEN** the process finished binding listeners against a reachable PostgreSQL and a client calls `GET /health/ready`
- **THEN** the answer is 200 and the database check passes

#### Scenario: Unreachable database is visible but does not fail readiness
- **WHEN** the database becomes unreachable after a ready start and a client calls `GET /health/ready`
- **THEN** the answer remains 200 while the body shows the database check failed with reason `dependency_unavailable`

### Requirement: Metrics and version expose build identity on the operator plane
The admin plane SHALL serve `GET /metrics` returning Prometheus text exposition including a build-info gauge labelled with version and git SHA, and `GET /version` returning the service name, role, crate version, git SHA (`unknown` outside a container build), and Rust toolchain version. All four admin routes SHALL respond with `Cache-Control: no-store`, and an unknown admin path SHALL return a plain 404 without an error envelope.

#### Scenario: Version carries the compiled git SHA
- **WHEN** the binary is built with the git SHA provided to the compiler environment and a client calls `GET /version`
- **THEN** the response body contains that SHA, the crate version, and the service name

#### Scenario: Metrics exposes build info
- **WHEN** a client calls `GET /metrics`
- **THEN** the response is Prometheus text containing a build-info series with non-empty version and git SHA labels, and the Content-Type is the Prometheus text exposition type

#### Scenario: Admin responses forbid caching
- **WHEN** any admin route answers, including the 404 for an unknown path
- **THEN** the response carries `Cache-Control: no-store`

### Requirement: Graceful shutdown exits cleanly
On SIGTERM or SIGINT the service SHALL stop accepting new work, finish within a configurable grace window, close the database pool, flush telemetry, and exit 0. A second signal during drain SHALL NOT extend the window beyond its bound.

#### Scenario: SIGTERM produces a clean exit
- **WHEN** a running healthy service receives SIGTERM
- **THEN** it stops serving within the shutdown timeout and exits with code 0
