# Tasks: add-social-source-publishing

Every behaviour task is a pair: the first adds a failing test, the second makes it pass. Tests run with `build-gate -- cargo test --locked -p ratatoskr-instagram-archive --test publishing <name>`; the full gate runs before the change completes.

## 1. Contracts dependency

- [x] 1.1 Add a failing test asserting the crate can construct and round-trip a `SocialSourceCaptured` payload through an event envelope (test: `tests/publishing.rs::first_resolution_emits_single_captured_fact`); expect failure because `ratatoskr-social-contracts` is not a dependency.
- [x] 1.2 Add the pinned git dependency on `ratatoskr-social-contracts` (D3) to `crates/instagram-archive/Cargo.toml` and lock it in `Cargo.lock`; verify the test from 1.1 now runs.

## 2. Snapshot construction

- [x] 2.1 Add a failing test that builds a snapshot from stored rows of a resolved capture (share-extension client, revision with text and one media item, no archived media bytes) and asserts acquisition `share_extension`, authority `explicit_user_capture`, partial completeness with one media warning, digest over normalized content, BlobRef raw reference (test: `tests/publishing.rs::snapshot_maps_resolved_capture_provenance_verbatim`).
- [x] 2.2 Implement the pure builder (D4) until the test passes.
- [x] 2.3 Add failing tests for refusals: unknown provenance token fails construction; identity uuidv5 is stable per `(owner, platform, permalink)`, differs across owners, equals across re-resolutions (tests: `snapshot_refuses_unknown_provenance_token`, `identity_is_stable_per_owner_and_permalink`); implement derivation (D2).

## 3. Captured emission

- [x] 3.1 Add a failing integration test: first successful resolution appends exactly one outbox row whose envelope event type is `social.source.captured.v1` and whose payload round-trips equal to stored state; replaying resolution adds none (test: `tests/publishing.rs::first_resolution_emits_single_captured_fact`).
- [x] 3.2 Append the fact inside the resolution transaction (D5) until the test passes.
- [x] 3.3 Add failing test: unavailable fallback emits no social-source fact and leaves prior outbox state untouched (`unavailable_outcome_emits_nothing`); implement the refusal path.

## 4. Updated emission and deletion observation

- [x] 4.1 Add failing tests: re-resolution with changed content appends exactly one `social.source.updated.v1` carrying the full new record under the same identity; later deleted-upstream observation republishes with `deleted_upstream` and untouched text/media (`changed_revision_emits_updated_fact`, `deletion_observation_republishes_content_untouched`).
- [x] 4.2 Wire updated emission into the re-resolution and observation paths until both tests pass.

## 5. Privacy, transactionality, delivery

- [x] 5.1 Add failing tests: a noted capture's serialized payload and envelope contain no note fragment; rolling back the triggering transaction leaves no outbox row; crash between delivery and acknowledgement redelivers byte-identical payload (`note_never_reaches_the_wire`, `rollback_leaves_no_fact`, `redelivery_is_byte_identical`).
- [x] 5.2 Implement publisher loop with claim/mark semantics and the transport seam (D5) until they pass.
- [x] 5.3 Add publisher metrics (unpublished depth, delivered, duplicate, failed counters) and verify they appear on `/metrics` output in the telemetry test file.

## 6. Documentation alignment

- [x] 6.1 Replace stale `social.source.upserted.v1` references with the published captured/updated names in `README.md`, root `AGENTS.md`, and `docs/`; state the unavailable-capture gap honestly where publication is described. Verification: `grep -rn "upserted" README.md AGENTS.md docs/` returns nothing.

## 7. Knowledge integration and local deletion propagation

The contracts gate is green: `social.source.removed.v1` and the typed
`knowledge.analysis.completed.v1` linkage fact are published. Knowledge owns
analysis internals; this service only supplies captured/updated preservation
facts, consumes the completion fact through its inbox, and stores the local
observation.

- [x] 7.1 Add failing linkage round-trip test for `knowledge.analysis.completed.v1`, then consume it idempotently into a capture/digest linkage record.
- [x] 7.2 Add failing tombstone test, then commit one `social.source.removed.v1` outbox fact and the tombstone atomically; a late completion must be skipped rather than resurrecting the capture.
- [x] 7.3 Keep unavailable-only captures un-emitted: their missing normalized snapshot remains an explicit product limitation, not a fabricated source.
