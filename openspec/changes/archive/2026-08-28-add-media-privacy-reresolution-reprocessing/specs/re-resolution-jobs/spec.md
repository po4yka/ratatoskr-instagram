## Purpose

Defines finite scheduled refresh of recent eligible public captures while preserving supported resolver, privacy, provenance, and append-only evidence boundaries.

## ADDED Requirements

### Requirement: Automatic re-resolution selects only due recent live captures

A run SHALL consider only owner-held, non-removed captures within the configured recency window whose next-resolution deadline is due and whose latest supported outcome is resolved, temporarily unavailable, or resolution failed. Private, deleted, unsupported, permanently unavailable, not-due, older-than-window, and locally removed captures MUST be skipped with a closed reason in deterministic `(next_resolution_at, capture_id)` order.

#### Scenario: Eligibility excludes terminal and stale captures

- **WHEN** one run sees resolved, transient, private, deleted, unsupported, old, not-due, and locally removed captures
- **THEN** only due recent resolved/transient captures are admitted and every other capture has the expected closed skip reason

### Requirement: Every run and request is guarded by finite budgets before I/O

A run SHALL require non-zero finite limits for admitted items, provider requests, accepted response bytes, duration, and concurrency. Immediately before each request, the service SHALL revalidate ownership, live-removal state, eligibility, deadline, and all run and provider budgets, then atomically reserve capacity. Exhausting any guard MUST prevent the request and MUST NOT consume unreserved capacity. No database connection SHALL remain held during provider I/O.

#### Scenario: Exhausted guard performs zero requests

- **WHEN** any item, request, byte, duration, concurrency, or provider allowance is zero or exhausted before claim
- **THEN** the recording resolver sees no call, the item records the exact budget skip, and all counters remain within their configured maxima

#### Scenario: Deletion between selection and claim prevents resurrection

- **WHEN** a selected capture is privacy-deleted before its request is admitted
- **THEN** pre-I/O revalidation skips it with zero provider calls, revisions, normalized writes, or source facts

### Requirement: Accepted refreshes append truthful evidence without duplicate facts

Accepted re-resolution SHALL pass through the same supported public resolver and append-only raw/revision contract as initial resolution. Equal normalized content SHALL not append another update fact; changed content SHALL append one full current update. Timeout, malformed, unavailable, or over-budget responses SHALL preserve the prior projection and record only the outcome that was actually observed.

#### Scenario: Equal refresh adds no duplicate update

- **WHEN** a re-resolution yields normalized content equal to the current revision
- **THEN** the run records an unchanged result, preserves append-only evidence as permitted by policy, and appends no `social.source.updated.v1` duplicate

#### Scenario: Failed refresh preserves the last good projection

- **WHEN** a previously resolved capture later times out or returns malformed or unavailable evidence
- **THEN** its last good normalized projection remains current and no deletion or native-unsave fact is inferred
