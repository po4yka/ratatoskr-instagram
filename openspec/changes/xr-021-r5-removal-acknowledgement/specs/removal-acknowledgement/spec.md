## ADDED Requirements

### Requirement: A removal is republished until Knowledge acknowledges it

The service SHALL arm `next_resend_at = published_at + resend_after` when it marks a `social.source.removed.v1` outbox row published, SHALL publish the stored envelope bytes again, with the same `event_id`, whenever a published, unacknowledged removal is due, and SHALL then set `resend_count = resend_count + 1` and `next_resend_at = now() + least(resend_after * 2^resend_count, resend_cap)`. The resend SHALL stop only when `acknowledged_at` is set (XR-021 CONTRACTS.md R5-11 rule 4).

#### Scenario: The first copy arms the first resend

- **WHEN** a removal row is published and a row of another event type is published
- **THEN** the removal's `next_resend_at` equals its `published_at` plus the resend delay and the other row has none

#### Scenario: A due removal is published again with the same id and the delay doubles

- **WHEN** a published, unacknowledged removal is due and a pass runs against a transport that records its calls
- **THEN** the transport receives the stored body under the same `event_id`, `resend_count` is one higher and the next resend is due after the delay times two to the power of the old count, never more than the cap

#### Scenario: A failed republish backs off and changes nothing else

- **WHEN** the transport refuses the republish of a due removal
- **THEN** the row's `published_at`, `resend_count` and `next_resend_at` are unchanged, its failure backoff is set, and the pass verdict stays healthy

#### Scenario: An unacknowledged removal is not unpublished work

- **WHEN** a pass runs while an unacknowledged removal is published
- **THEN** the unpublished depth is zero, the verdict is healthy and the gauges `instagram_unacknowledged_tombstones` and `instagram_oldest_unacknowledged_tombstone_age_seconds` report the removal

### Requirement: An acknowledgement ends the resend of its own row only

The service SHALL record `acknowledged_at` and `ack_outcome` (`applied` or `refused`) on the removal whose `event_id` equals the acknowledged event id and whose envelope tenant equals the acknowledgement's owner. An acknowledgement of an unknown id, of another tenant's row or of a row that is not a removal SHALL change nothing, and a second acknowledgement of one row SHALL change nothing.

#### Scenario: An applied acknowledgement stops the resend

- **WHEN** Knowledge acknowledges a removal as `applied` and the removal later becomes due
- **THEN** the row records the acknowledgement and no later pass republishes it

#### Scenario: A refused acknowledgement stops the resend too

- **WHEN** Knowledge acknowledges a removal as `refused`
- **THEN** the row records `refused`, no later pass republishes it, an ERROR line without content is logged and `instagram_tombstone_refused_total` increases by one

#### Scenario: An acknowledgement for no row is ignored

- **WHEN** an acknowledgement names an unknown id or another owner's removal
- **THEN** no row changes and the delivery is acknowledged to the broker

### Requirement: The service consumes its acknowledgement durable

The service SHALL consume `ratatoskr_instagram_deletion_acks` on `ratatoskr_events`, verifying filter `evt.knowledge.deletion.acknowledged.v1`, explicit acknowledgement and a 30 second ack wait before it reports ready and never creating the durable. A message that is not a valid acknowledgement from `ratatoskr-knowledge` SHALL be terminated, and readiness SHALL NOT depend on Knowledge answering.

#### Scenario: A real broker round trip

- **WHEN** a removal is published, nobody answers, the removal is republished under the same `Nats-Msg-Id` after the delay, and Knowledge then publishes the acknowledgement
- **THEN** the row records the acknowledgement and the resend stops

#### Scenario: A mismatched durable keeps the consumer not ready

- **WHEN** the durable has another filter or another ack wait
- **THEN** the lane refuses to start the consumer and the process does not report ready

### Requirement: Privacy deletion keeps an unacknowledged removal

The privacy-deletion statements that remove an aggregate's outbox rows SHALL keep every removal that Knowledge has not acknowledged.

#### Scenario: A later deletion keeps the unacknowledged removal and drops the answered one

- **WHEN** a capture has an unacknowledged removal and an acknowledged removal and its owner's privacy deletion runs
- **THEN** the unacknowledged removal still exists and the acknowledged one is gone

### Requirement: The resend timing is strict configuration

`RATATOSKR__TOMBSTONE__RESEND_AFTER_SECONDS` SHALL default to 600 and accept 30 to 86400, and `RATATOSKR__TOMBSTONE__RESEND_CAP_SECONDS` SHALL default to 21600 and accept values from the first key's value to 604800.

#### Scenario: Out-of-range values are refused

- **WHEN** the delay is 29, or the cap is below the delay or above 604800
- **THEN** the configuration fails naming the key and not the value
