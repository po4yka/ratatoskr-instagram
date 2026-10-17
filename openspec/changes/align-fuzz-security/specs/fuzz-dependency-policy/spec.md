## ADDED Requirements

### Requirement: The standalone fuzz graph passes dependency policy

CI SHALL check the standalone fuzz workspace with locked metadata and the full repository dependency policy. The global license allowlist and advisory rejection rules SHALL apply to that graph. A license exception SHALL be limited to libfuzzer-sys exactly 0.4.13.

#### Scenario: The fuzz graph contains a blocked dependency

- **WHEN** locked fuzz metadata is stale or its dependency graph violates the repository policy
- **THEN** the corresponding CI gate fails

#### Scenario: Both graphs satisfy the policy

- **WHEN** locked root and fuzz graphs satisfy the repository policy, with the exact reviewed tool exception
- **THEN** both dependency checks succeed without advisory suppression
