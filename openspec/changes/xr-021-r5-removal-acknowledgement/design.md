## Context

The authoritative cross-repository text is XR-021 CONTRACTS.md R5-11. This document records only decisions internal to `ratatoskr-instagram`.

## Decisions

### State lives on the outbox row

A removal row moves through: unpublished, published and armed (`next_resend_at` set), resent any number of times (`resend_count` grows, `next_resend_at` moves out), acknowledged (`acknowledged_at`, `ack_outcome`). The row id is the event id, so the acknowledgement's `acknowledged_event_id` finds the row by primary key, and the tenant check compares the acknowledgement's `owner` with the `tenant_id` of the stored envelope. The database also checks that the four columns are used only by removals and that `ack_outcome` is present exactly when `acknowledged_at` is.

### The publishing module owns the transitions

`publishing/removal_ack.rs` holds `mark_published` (arming), `record_resend`, `acknowledge`, `unacknowledged_removals` and the due-resend selection, so the pump and the consumer share one definition of each. `record_resend` computes `now() + least(resend_after * 2^resend_count, cap)` from the pre-update `resend_count`, clamping the exponent so the arithmetic cannot overflow. The pass selects `published_at is not null and acknowledged_at is null and next_resend_at <= now() and next_attempt_at <= now()` and republishes the stored envelope. A failed republish records the existing backoff on `next_attempt_at` and leaves `published_at`, `next_resend_at` and `resend_count` alone. Because the pass verdict and `instagram_outbox_unpublished_depth` read only unpublished rows, an unacknowledged removal affects neither; the pass reports resends in their own counters (`resent`, `resend_failed`).

### Resend runs after the fresh rows and stops at a refusing broker

A pass publishes due unpublished rows first. When none of those failed, it republishes due removals, oldest due first, and stops at the first failed republish. A broker that refused one publish will refuse the next.

### `run_once` takes the policy

`ResendPolicy { after, cap }` is a parameter of `run_once` because `mark_published` needs `after` and the resend needs both. The alternative, a second entry point for resends, would leave two code paths that can disagree. The relay loop receives the policy from the configuration; tests pass millisecond delays directly, and the 30 second minimum is a configuration rule.

### Configuration is a section of its own

`config/publisher.rs` now holds `PublisherConfig` and the new `TombstoneConfig`, because `config.rs` sits at the 850-line cap. The events stream collapses a repeated `Nats-Msg-Id` inside its 120 second duplicate window, so a resend sooner than that is a no-op at the broker: the 600 second default is deliberate.

### The consumer mirrors the browser-command consumer

`services/instagram-archive/src/deletion_acks.rs` verifies the Edge-provisioned durable (name, filter `evt.knowledge.deletion.acknowledged.v1`, explicit acks, 30 second ack wait, not push based) before it reports ready, never creates one, and runs under the same supervisor as the other bus tasks. An envelope that does not decode, has another event type, another producer or an invalid payload is Termed: it cannot be attributed and redelivery cannot fix it. A valid acknowledgement of another family, another tenant or an unknown id is acknowledged and ignored. A database failure is Nak'd with a delay. A duplicate acknowledgement changes nothing because the update requires `acknowledged_at is null`. Readiness does not depend on Knowledge answering.

### The gauges are refreshed by the relay pass

The pass already runs every second against the same database, so it sets the two gauges at its end from one aggregate query. They go out through the existing Prometheus recorder on the operator listener.

### Privacy deletion keeps what Knowledge has not acknowledged

The two statements in `privacy_deletion/apply.rs` that remove an aggregate's other outbox rows kept every removal row. They now use `not (event_type = 'social.source.removed.v1' and acknowledged_at is null)`: an unacknowledged removal survives, and a removal Knowledge has answered is ordinary history that goes with the aggregate.

## Risks

A removal Knowledge never acknowledges is republished forever with the delay doubling to the cap (6 hours by default); that is the R5-11 rule, and the gauges make it visible.
