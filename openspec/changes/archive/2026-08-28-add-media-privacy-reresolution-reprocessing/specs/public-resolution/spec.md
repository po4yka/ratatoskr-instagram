## ADDED Requirements

### Requirement: Scheduled re-resolution uses the same supported public contract

Automatic and operator-triggered re-resolution SHALL use the same canonical permalink validation, approved public surface, finite network limits, response validation, append-only evidence, parser stamping, deterministic normalization, and truthful failure classifications as initial resolution. It MUST NOT use authenticated browser state, private page data, hidden APIs, or a broader redirect policy.

#### Scenario: Initial and scheduled resolution refuse the same invalid response

- **WHEN** the same redirecting, oversized, malformed, or source-mismatched response is supplied to initial and scheduled resolution
- **THEN** both paths return the same closed refusal class and commit no normalized content from that response

### Requirement: Scheduled request admission is revalidated immediately before I/O

Selection alone SHALL grant no request authority. Immediately before scheduled I/O, the service SHALL confirm the capture remains owner-held, locally live, policy-due, supported, recent, and within every run and provider budget. A failed check MUST start no request and MUST append no resolution evidence or source fact.

#### Scenario: Removed selected capture starts no request

- **WHEN** privacy deletion commits after selection but before request admission
- **THEN** scheduled resolution performs no network I/O and cannot recreate the capture, media, revision, or source fact
