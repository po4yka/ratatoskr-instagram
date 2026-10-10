## ADDED Requirements

### Requirement: The outbox publishes through JetStream and never marks an undelivered row published

The service SHALL deliver outbox rows through a JetStream transport that publishes to `evt.<event_type>` for a closed list of four event types, sets `Nats-Msg-Id` to the event id, and returns success only after the JetStream acknowledgement (XR-021 CONTRACTS.md S02 rules 2 to 4, S03). An event type outside the list SHALL be a transport error, and the schema SHALL refuse to store one. The transport SHALL refuse an envelope whose id differs from its outbox row id, because that id is the `Nats-Msg-Id`. When no bus is configured no publisher SHALL start and rows SHALL remain unpublished. A failed delivery SHALL leave the row unpublished with a backoff, and a row whose backoff has not elapsed SHALL not be selected, so a failing head row cannot starve later rows.

#### Scenario: A delivered row reaches the broker with its identity

- **WHEN** the transport delivers a captured envelope
- **THEN** the broker holds one message on `evt.social.source.captured.v1` whose `Nats-Msg-Id` equals the event id

#### Scenario: An unlisted type is refused

- **WHEN** the transport is asked to deliver an unlisted event type
- **THEN** it returns a transport error and the row stays unpublished

#### Scenario: A failing head row does not starve later rows

- **WHEN** a batch of one is run with a head row that always fails and a later healthy row
- **THEN** the healthy row is delivered on the next pass
