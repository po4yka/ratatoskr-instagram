## Why

The `ratatoskr_events` stream drops its oldest messages after 7 days or at 1 GiB, and the Instagram relay marks a `social.source.removed.v1` outbox row published at the PubAck and never looks at it again. A deletion fact that Knowledge does not consume in time is therefore lost silently, and nothing tells the service that Knowledge applied it. The owner's stream policy (XR-021 CONTRACTS.md R5-11) is that a deletion fact is kept and published again until Knowledge acknowledges it with the registered `knowledge.deletion.acknowledged.v1`. This change implements rule 4 of R5-11 for Instagram and cites it without restating it.

## What Changes

- `instagram_archive.outbox_events` gains `acknowledged_at`, `ack_outcome`, `resend_count` and `next_resend_at` (`schema.sql` edited in place, no migration). Only `social.source.removed.v1` rows use them.
- **BREAKING** `publishing::run_once` takes a `ResendPolicy`; every caller changes. Marking a removal published arms `next_resend_at`; a due, unacknowledged removal is published again with the stored bytes and the same `event_id`, and the delay doubles up to a cap. A failed republish uses the existing failure backoff and never touches `published_at`, the pass verdict or the unpublished depth.
- New strict configuration keys `RATATOSKR__TOMBSTONE__RESEND_AFTER_SECONDS` (default 600, 30 to 86400) and `RATATOSKR__TOMBSTONE__RESEND_CAP_SECONDS` (default 21600, at least the first, at most 604800), documented in `deploy/systemd/instagram.conf.example`.
- A supervised consumer of the Edge-provisioned durable `ratatoskr_instagram_deletion_acks` records Knowledge's answer; `refused` also ends the resend, logs an ERROR line without content and counts `instagram_tombstone_refused_total`. The gauges `instagram_unacknowledged_tombstones` and `instagram_oldest_unacknowledged_tombstone_age_seconds` are served on the operator listener and never feed readiness.
- `deploy/nats/identity.conf` gains the three permissions for that durable, equal to the stanza Platform ships.
- Both privacy-deletion statements that remove an aggregate's outbox rows keep an unacknowledged removal and no longer keep an acknowledged one.

## Capabilities

### New Capabilities

- `removal-acknowledgement`: the resend, the acknowledgement and the consumer.

### Modified Capabilities

- `archive-schema`: the four outbox columns.

## Impact

- `schema.sql`, `crates/instagram-archive` (`src/publishing/pump.rs`, new `src/publishing/removal_ack.rs`, `src/config.rs` and new `src/config/publisher.rs`, `src/privacy_deletion/apply.rs`), `services/instagram-archive` (`src/deletion_acks.rs`, `src/bus.rs`, `src/relay.rs`, `src/main.rs`), tests, `deploy/nats/identity.conf`, `deploy/systemd/instagram.conf.example`, `README.md`, `DEVELOPMENT.md`.
- Out of scope: erasing social facts from the events stream (R5-15), Knowledge's side, Platform's durable and ACL.
