## 1. Durable own-media generations

- [x] 1.1 Add `crates/instagram-archive/tests/schema.rs::own_media_schema_carries_resumable_runs_watermarks_and_atomic_authority`, run it against the unchanged schema, and confirm its catalog assertion fails because the own-media state, run, staged-item, and authority objects are absent.
- [x] 1.2 Edit `schema.sql` in place and extend the schema inventory for due state, one resumable active run, staged candidate items, current authority, closed outcomes/reasons, and `own_media_page` provider usage; verify task 1.1 passes with `build-gate -- cargo nextest run --locked -p ratatoskr-instagram-archive --test schema own_media_schema_carries_resumable_runs_watermarks_and_atomic_authority`. No migration file or tooling: development status forbids both.

## 2. Supported lane and strict provider pages

- [x] 2.1 Replace the item-7 planning assertion with `crates/instagram-archive/tests/account_capabilities.rs::own_account_mode_is_supported_after_item_seven`, run it, and confirm the support-status assertion fails with `Planned` instead of `Supported`.
- [x] 2.2 Flip only `AcquisitionMode::OwnAccountSync` to `Supported` while retaining `official_api` and `authoritative_platform_state`; verify task 2.1 passes.
- [x] 2.3 Add synthetic `tests/fixtures/meta/own_media_page_1.json` and `own_media_page_2.json` plus `crates/instagram-archive/tests/provider.rs::own_media_request_is_owner_scoped_and_omits_ephemeral_fields`; run it and confirm the request-path/field assertion fails because the provider adapter has no reviewed own-media request.
- [x] 2.4 Extend the provider port, request class, and production/test adapters with the bounded connected-account own-media page operation so task 2.3 passes; verify the request uses the bearer header, connected account id, explicit non-ephemeral field set, optional continuation, and durable budget-before-I/O.
- [x] 2.5 Add `crates/instagram-archive/tests/provider.rs::own_media_fixture_rejects_foreign_owner_and_unknown_page_shape`, run it, and confirm the acceptance assertion fails because own-media parsing does not yet enforce owner identity and the closed page grammar.
- [x] 2.6 Implement strict page parsing, duplicate-id/foreign-owner refusal, bounded raw-byte preservation inputs, and typed retry classification so task 2.5 passes against the synthetic fixtures.

## 3. Capability-aware scheduled execution

- [x] 3.1 Add `crates/instagram-archive/tests/own_media_sync.rs::unsupported_account_job_records_noop_without_provider_contact`, run it, and confirm the terminal-outcome assertion fails because no no-op run is persisted; add only the minimal compiling own-media executor seam if required for RED.
- [x] 3.2 Implement due-account claiming and the pre-credential current-generation gate so task 3.1 passes with zero fake-provider calls, an `account_type_unsupported` no-op outcome, and unchanged authority/watermark.
- [x] 3.3 Add `crates/instagram-archive/tests/own_media_sync.rs::permission_downgrade_job_uses_current_generation_and_noops`, run it, and confirm its stored-generation/reason assertion fails because a stale available generation is still exercisable.
- [x] 3.4 Make every claimed job use only the current owner-bound credential and total capability row, recording the exact unavailable reason before credential opening; verify task 3.3 passes.
- [x] 3.5 Add `crates/instagram-archive/tests/config.rs::own_media_scheduler_is_disabled_by_default_and_rejects_unbounded_limits`, run it, and confirm the enabled-with-invalid-limits case is accepted by the current loader.
- [x] 3.6 Add strict finite own-media cadence, per-tick-account, page, and call-budget configuration plus the delayed scheduler loop over `run_due_once`; verify task 3.5 passes and direct scheduler tests require no wall-clock sleep.

## 4. Checkpoint and watermark safety

- [x] 4.1 Add `crates/instagram-archive/tests/own_media_sync.rs::completed_incremental_scan_advances_watermark_after_reaching_prior_media`, run it, and confirm the watermark assertion remains the prior provider id after the fixture traversal.
- [x] 4.2 Implement newest-first incremental traversal and completion-only watermark advancement so task 4.1 passes with the first newly observed provider id.
- [x] 4.3 Add `crates/instagram-archive/tests/own_media_sync.rs::failed_scan_retains_watermark_and_resumes_committed_cursor`, run it, and confirm the retry starts from the beginning or the old watermark changes instead of preserving the committed continuation.
- [x] 4.4 Persist each accepted page and next cursor, classify retryable termination without authority, and resume the single active run from that cursor so task 4.3 passes; verify request/page exhaustion stops before another provider call.

## 5. Atomic authority and authorization drift

- [x] 5.1 Add `crates/instagram-archive/tests/own_media_sync.rs::completion_atomically_swaps_retained_refreshed_and_new_media`, run it, and confirm the visible-set assertion exposes staged partial rows or never changes from the prior generation.
- [x] 5.2 Implement candidate seeding, staged provider-identity upserts, and one completion transaction that applies normalized media/revisions, moves authority, and advances the watermark so task 5.1 passes; prefix absence must retain old items and never infer deletion.
- [x] 5.3 Add `crates/instagram-archive/tests/own_media_sync.rs::capability_generation_change_before_completion_preserves_prior_authority`, run it, and confirm the authority assertion changes despite a simulated permission downgrade.
- [x] 5.4 Revalidate owner, connected state, provider identity, and capability generation under the completion lock; terminate stale work with the current limit reason so task 5.3 passes without authority or watermark change.

## 6. BlobRef truth and SocialSource publication

- [x] 6.1 Add `crates/instagram-archive/tests/own_media_sync.rs::metadata_only_completion_publishes_official_fact_with_raw_blob_and_warning`, run it, and confirm the outbox assertion finds no own-media fact or finds a fabricated media-byte BlobRef.
- [x] 6.2 Generalize source snapshot/outbox construction for completed own-media identities so task 6.1 passes with stable owner-scoped identity, `official_api` acquisition, provider publication time, raw-response BlobRef, sync checkpoint, empty media attachments, partial completeness, and a missing-media warning.
- [x] 6.3 Add `crates/instagram-archive/tests/own_media_sync.rs::unchanged_generation_emits_no_duplicate_and_changed_metadata_emits_one_update`, run it, and confirm the captured/updated event counts are duplicated or no update appears.
- [x] 6.4 Make completion publication transactional and idempotent by source/fact/content digest so task 6.3 passes, and ensure no caption, provider URL, raw body, token, or username enters ordinary telemetry.

## 7. Documentation and validation

- [x] 7.1 Update README, DEVELOPMENT, capability matrix, architecture, interfaces, data model, testing strategy, threat/security notes, and implementation-plan status for the supported official own-media lane, disabled-by-default schedule, watermark/authority semantics, metadata-only BlobRef boundary, no stories, and no foreign-media downloads. Documentation cannot start from a failing behavior test; verify terminology and command-list drift with `rg` and the existing gate.
- [x] 7.2 Run `git diff --check`, `openspec validate add-own-media-synchronization --strict`, `openspec validate --all --strict`, `cargo fetch --locked`, `cargo deny --locked check`, `cargo fmt --all -- --check`, `build-gate -- cargo clippy --workspace --all-targets --locked -- -D warnings`, `build-gate -- cargo build --workspace --locked`, `build-gate -- cargo test --workspace --locked`, `build-gate -- cargo test --workspace --locked --doc`, and `build-gate -- cargo build --workspace --locked --release` against PostgreSQL 17 at the documented disposable endpoint; review the final diff and observe every command green before marking this validation-only task complete.
