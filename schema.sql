-- The Instagram Archive database, in one file.
--
-- `ratatoskr-instagram-archive` applies this at startup, to a fresh database. There is no migration
-- ledger and no incremental history: no database holds data that has to survive a schema change. A
-- schema change edits this file in place; the next fresh database has it.
--
-- One schema: `instagram_archive` — everything the Instagram bounded context owns. The table
-- inventory follows the planned data model in README.md.
--
-- Conventions, applied uniformly and stated once here:
--
--   * Identifiers are UUIDv7 minted by the application, never by the database. A database default
--     would produce v4, so there is deliberately no DEFAULT on any id column: a missing id is an
--     insert error rather than a silently wrong version.
--
--   * Closed vocabularies are `text` with a named CHECK, not a PostgreSQL enum: adding a value to
--     an enum cannot run inside one transaction and removing one is a table rewrite; a CHECK is
--     altered by one statement.
--
--   * Every timestamp is `timestamptz`. `timestamp` would silently record the server's local time.
--
--   * Hashes are stored in `bytea` and the column is named `*_hash`. No column here holds a
--     credential in a readable form: token material lives only as ciphertext produced inside this
--     bounded context (SECURITY.md).
--
--   * References to identifiers owned by other services (`*_ref`) or other schemas are plain uuid
--     columns with no REFERENCES clause. No foreign key crosses the schema boundary.
create schema instagram_archive;

comment on schema instagram_archive is
    'State owned exclusively by ratatoskr-instagram. Accounts, captures, resolved media, '
    'Data Export snapshots, raw evidence, availability observations, and the event machinery.';

-- ---------------------------------------------------------------------------------------------
-- accounts
-- ---------------------------------------------------------------------------------------------
--
-- A connected Instagram account of a supported type. One row per provider account; the provider
-- identity is stable while usernames are mutable display data.

create table instagram_archive.accounts (
    account_id          uuid        primary key,
    user_ref            uuid        not null,
    provider_account_id text        not null,
    username            text        not null,
    account_type        text        not null,
    connection_status   text        not null,
    scopes              text[]      not null default '{}',
    connected_at        timestamptz not null,
    updated_at          timestamptz not null default now(),
    constraint accounts_provider_account_id_key unique (provider_account_id),
    constraint accounts_account_type_check
        check (account_type in ('business', 'creator', 'personal', 'unknown')),
    constraint accounts_connection_status_check
        check (connection_status in ('connected', 'reauthorization_required', 'revoking', 'revoked'))
);

comment on table instagram_archive.accounts is
    'Connected Instagram accounts. user_ref names the Ratatoskr owner and crosses no schema.';

-- ---------------------------------------------------------------------------------------------
-- credentials
-- ---------------------------------------------------------------------------------------------
--
-- Provider token material, encrypted inside this bounded context. No column holds plaintext.

create table instagram_archive.credentials (
    credential_id          uuid        primary key,
    account_id             uuid        not null,
    access_token_envelope  bytea       not null,
    refresh_token_envelope bytea,
    key_version            integer     not null,
    granted_permissions    text[]      not null default '{}',
    expires_at             timestamptz,
    rotated_at             timestamptz,
    created_at             timestamptz not null default now(),
    constraint credentials_account_id_key unique (account_id),
    constraint credentials_account_id_fkey foreign key (account_id)
        references instagram_archive.accounts (account_id)
);

comment on table instagram_archive.credentials is
    'Encrypted OAuth token material for one account. Ciphertext only, versioned for rotation.';

-- A pending, owner-bound OAuth transaction. Raw state and authorization codes never reach rows.
create table instagram_archive.oauth_flows (
    flow_id                uuid        primary key,
    user_ref               uuid        not null,
    account_id             uuid,
    state_hash             bytea       not null,
    redirect_uri_hash      bytea       not null,
    pkce_verifier_envelope bytea,
    key_version            integer,
    expires_at             timestamptz not null,
    consumed_at            timestamptz,
    created_at             timestamptz not null default now(),
    constraint oauth_flows_state_hash_key unique (state_hash),
    constraint oauth_flows_account_id_fkey foreign key (account_id)
        references instagram_archive.accounts (account_id),
    constraint oauth_flows_pkce_key_pair_check check (
        (pkce_verifier_envelope is null and key_version is null)
        or (pkce_verifier_envelope is not null and key_version is not null)
    )
);

comment on table instagram_archive.oauth_flows is
    'Single-use owner-bound OAuth state hashes and optional encrypted PKCE verifiers.';

-- ---------------------------------------------------------------------------------------------
-- raw_records
-- ---------------------------------------------------------------------------------------------
--
-- Raw evidence before normalization: resolver responses, API payloads, export sections, unknown
-- records preserved for future parser versions, and separately-provenanced user uploads.

create table instagram_archive.raw_records (
    raw_record_id uuid        primary key,
    record_kind   text        not null,
    blob_ref      text        not null,
    content_hash  bytea       not null,
    byte_size     bigint      not null,
    body          bytea       not null,
    observed_at   timestamptz not null,
    constraint raw_records_record_kind_check
        check (record_kind in
            ('oembed_response', 'api_response', 'export_section', 'unknown_export_record',
             'user_upload'))
);

comment on table instagram_archive.raw_records is
    'Content-addressed raw evidence. blob_ref is the lowercase hex SHA-256 of body — the '
    'BlobStore key once bodies move out of rows. Small payloads ride inline in body until '
    'that store exists; normalization reads the bytes, never a reconstruction.';

-- One complete permission-discovery generation. Missing permissions are inserted explicitly.
create table instagram_archive.account_permission_observations (
    observation_id   uuid        primary key,
    account_id       uuid        not null,
    generation_id    uuid        not null,
    permission_name  text        not null,
    permission_status text       not null,
    raw_record_id    uuid        not null,
    observed_at      timestamptz not null,
    constraint account_permission_observations_generation_permission_key
        unique (account_id, generation_id, permission_name),
    constraint account_permission_observations_account_id_fkey foreign key (account_id)
        references instagram_archive.accounts (account_id),
    constraint account_permission_observations_raw_record_id_fkey foreign key (raw_record_id)
        references instagram_archive.raw_records (raw_record_id),
    constraint account_permission_observations_status_check
        check (permission_status in ('granted', 'declined', 'expired', 'absent', 'unknown'))
);

-- A total closed matrix for the latest generation of one account.
create table instagram_archive.account_capabilities (
    account_id       uuid        not null,
    generation_id    uuid        not null,
    capability       text        not null,
    capability_state text        not null,
    reason            text        not null,
    observed_at       timestamptz not null,
    constraint account_capabilities_pkey primary key (account_id, capability),
    constraint account_capabilities_account_id_fkey foreign key (account_id)
        references instagram_archive.accounts (account_id),
    constraint account_capabilities_capability_check check (capability in
        ('account_identity_read', 'own_media_read', 'content_publish', 'comment_management',
         'message_management', 'native_saved_read')),
    constraint account_capabilities_state_check
        check (capability_state in ('available', 'unavailable', 'not_supported')),
    constraint account_capabilities_reason_check check (reason in
        ('granted', 'account_type_unsupported', 'permission_declined', 'permission_expired',
         'permission_absent', 'permission_unknown', 'missing_permission',
         'write_consent_required', 'provider_not_supported', 'revoked',
         'reauthorization_required'))
);

-- Redacted append-only lifecycle evidence. detail may contain only closed operational facts.
create table instagram_archive.account_credential_audit (
    audit_id     uuid        primary key,
    account_id   uuid        not null,
    change_kind  text        not null,
    outcome      text        not null,
    detail       jsonb       not null default '{}',
    occurred_at  timestamptz not null,
    constraint account_credential_audit_account_id_fkey foreign key (account_id)
        references instagram_archive.accounts (account_id),
    constraint account_credential_audit_change_kind_check check (change_kind in
        ('authorized', 'refreshed', 'reauthorization_required', 'revoked')),
    constraint account_credential_audit_outcome_check check (outcome in
        ('succeeded', 'provider_failed', 'provider_unsupported', 'authentication_failed'))
);

-- One committed reservation per provider HTTP attempt. It deliberately stores no URL or payload.
create table instagram_archive.provider_api_usage (
    usage_id           uuid        primary key,
    operation_id       uuid        not null,
    account_id         uuid,
    request_class      text        not null,
    attempt_ordinal    integer     not null,
    state              text        not null,
    outcome            text,
    http_status        smallint,
    call_count_percent smallint,
    cpu_time_percent   smallint,
    total_time_percent smallint,
    started_at         timestamptz not null,
    finished_at        timestamptz,
    constraint provider_api_usage_operation_ordinal_key unique (operation_id, attempt_ordinal),
    constraint provider_api_usage_account_id_fkey foreign key (account_id)
        references instagram_archive.accounts (account_id),
    constraint provider_api_usage_attempt_ordinal_check check (attempt_ordinal > 0),
    constraint provider_api_usage_request_class_check check (request_class in
        ('code_exchange', 'account_discovery', 'permission_discovery', 'token_refresh',
         'token_revoke', 'own_media_page')),
    constraint provider_api_usage_state_check check (state in ('started', 'completed')),
    constraint provider_api_usage_outcome_check check (outcome is null or outcome in
        ('succeeded', 'authentication', 'validation', 'rate_limited', 'server', 'network',
         'response_refused', 'provider_unsupported')),
    constraint provider_api_usage_terminal_check check (
        (state = 'started' and outcome is null and finished_at is null)
        or (state = 'completed' and outcome is not null and finished_at is not null)
    ),
    constraint provider_api_usage_call_count_percent_check
        check (call_count_percent between 0 and 100),
    constraint provider_api_usage_cpu_time_percent_check
        check (cpu_time_percent between 0 and 100),
    constraint provider_api_usage_total_time_percent_check
        check (total_time_percent between 0 and 100)
);

-- ---------------------------------------------------------------------------------------------
-- profiles
-- ---------------------------------------------------------------------------------------------

create table instagram_archive.profiles (
    profile_id          uuid        primary key,
    account_id          uuid,
    provider_profile_id text        not null,
    display_name        text,
    follower_count      bigint,
    media_count         bigint,
    observed_at         timestamptz not null,
    raw_record_id       uuid,
    constraint profiles_provider_profile_id_key unique (provider_profile_id),
    constraint profiles_account_id_fkey foreign key (account_id)
        references instagram_archive.accounts (account_id),
    constraint profiles_raw_record_id_fkey foreign key (raw_record_id)
        references instagram_archive.raw_records (raw_record_id)
);

comment on table instagram_archive.profiles is
    'Author/profile observations as they were seen at observed_at.';

-- ---------------------------------------------------------------------------------------------
-- media
-- ---------------------------------------------------------------------------------------------
--
-- Resolved post/reel/carousel sources: own media from the official lane or public representations
-- reached through supported resolution. Provenance is mandatory on every row.

create table instagram_archive.media (
    media_id            uuid        primary key,
    account_id          uuid,
    provider_media_id   text,
    permalink           text        not null,
    media_type          text        not null,
    caption             text,
    published_at        timestamptz,
    acquisition_method  text        not null,
    saved_authority     text        not null,
    upstream_status     text        not null,
    current_revision_id uuid,
    blob_ref            text,
    content_hash        bytea,
    byte_size           bigint,
    media_state         text        not null default 'metadata_only',
    retention_class     text        not null default 'metadata_only',
    retention_deadline  timestamptz,
    retention_reason    text,
    created_at          timestamptz not null default now(),
    updated_at          timestamptz not null default now(),
    constraint media_provider_media_id_key unique (provider_media_id),
    constraint media_permalink_key unique (permalink),
    constraint media_account_id_fkey foreign key (account_id)
        references instagram_archive.accounts (account_id),
    constraint media_media_type_check
        check (media_type in ('image', 'video', 'carousel', 'reel', 'story', 'unknown')),
    constraint media_acquisition_method_check
        check (acquisition_method in
            ('official_api', 'share_extension', 'browser_extension', 'public_resolution',
             'data_export', 'legacy_import')),
    constraint media_saved_authority_check
        check (saved_authority in
            ('explicit_user_capture', 'export_observation', 'authoritative_platform_state',
             'legacy_observation')),
    constraint media_upstream_status_check
        check (upstream_status in ('available', 'unavailable', 'deleted', 'private', 'unknown')),
    constraint media_media_state_check
        check (media_state in ('metadata_only', 'bytes_archived')),
    constraint media_retention_class_check
        check (retention_class in ('metadata_only', 'explicit_archive', 'policy_archive')),
    constraint media_retention_reason_check
        check (retention_reason is null or length(retention_reason) between 1 and 64),
    constraint media_archived_evidence_check check (
        (media_state = 'metadata_only' and blob_ref is null and content_hash is null
            and byte_size is null and retention_class = 'metadata_only')
        or
        (media_state = 'bytes_archived' and blob_ref is not null and content_hash is not null
            and byte_size is not null and byte_size >= 0
            and retention_class in ('explicit_archive', 'policy_archive'))
    )
);

comment on table instagram_archive.media is
    'Resolved media sources with mandatory acquisition and saved-authority provenance. '
    'current_revision_id names the newest media_revisions row this projection was read from.';

comment on column instagram_archive.media.media_type is
    'The type evidence actually proves. A plain post permalink reveals nothing through the '
    'oEmbed grammar, so it stores unknown rather than a guess.';

comment on constraint media_acquisition_method_check on instagram_archive.media is
    'How this record was obtained. Closed vocabulary; never silently upgraded.';
comment on constraint media_saved_authority_check on instagram_archive.media is
    'What the acquisition proves about saved state. An explicit capture never proves native state.';

-- ---------------------------------------------------------------------------------------------
-- media_relations
-- ---------------------------------------------------------------------------------------------

create table instagram_archive.media_relations (
    relation_id     uuid primary key,
    parent_media_id uuid not null,
    child_media_id  uuid not null,
    relation_kind   text not null,
    constraint media_relations_parent_child_kind_key unique (parent_media_id, child_media_id, relation_kind),
    constraint media_relations_parent_media_id_fkey foreign key (parent_media_id)
        references instagram_archive.media (media_id),
    constraint media_relations_child_media_id_fkey foreign key (child_media_id)
        references instagram_archive.media (media_id),
    constraint media_relations_relation_kind_check
        check (relation_kind in ('carousel_item', 'reel_cover', 'linked_post'))
);

comment on table instagram_archive.media_relations is
    'Edges between one source and its parts: carousel children, reel covers, linked posts.';

-- ---------------------------------------------------------------------------------------------
-- media_revisions
-- ---------------------------------------------------------------------------------------------
--
-- One immutable resolution attempt per row. A revision names the content-addressed raw payload
-- that answered and the parser version that interprets it; re-resolution appends a new row and
-- nothing ever updates or deletes an existing one.

create table instagram_archive.media_revisions (
    revision_id     uuid        primary key,
    media_id        uuid        not null,
    raw_record_id   uuid        not null,
    parser_version  text        not null,
    resolved_at     timestamptz not null,
    constraint media_revisions_media_id_fkey foreign key (media_id)
        references instagram_archive.media (media_id),
    constraint media_revisions_raw_record_id_fkey foreign key (raw_record_id)
        references instagram_archive.raw_records (raw_record_id)
);

comment on table instagram_archive.media_revisions is
    'Immutable resolution history. media.current_revision_id points at the newest one; every '
    'older revision stays byte-identical forever, so re-resolution never overwrites history.';

create index media_revisions_media_idx
    on instagram_archive.media_revisions (media_id);

alter table instagram_archive.media
    add constraint media_current_revision_id_fkey foreign key (current_revision_id)
    references instagram_archive.media_revisions (revision_id);

-- ---------------------------------------------------------------------------------------------
-- own-media synchronization
-- ---------------------------------------------------------------------------------------------
--
-- Provider pages are staged under one resumable run. Staged rows have no current-state authority:
-- only a completed run named by own_media_authority is visible as the account's own-media set.

create table instagram_archive.own_media_sync_runs (
    run_id                                uuid        primary key,
    account_id                            uuid        not null,
    user_ref                              uuid        not null,
    capability_generation_id              uuid,
    start_watermark_provider_media_id      text,
    candidate_watermark_provider_media_id  text,
    next_cursor                            text,
    status                                 text        not null,
    outcome_reason                         text,
    page_count                             bigint      not null default 0,
    item_count                             bigint      not null default 0,
    started_at                             timestamptz not null,
    updated_at                             timestamptz not null,
    finished_at                            timestamptz,
    constraint own_media_sync_runs_account_id_fkey foreign key (account_id)
        references instagram_archive.accounts (account_id),
    constraint own_media_sync_runs_status_check check (status in
        ('running', 'retryable', 'completed', 'capability_noop', 'failed')),
    constraint own_media_sync_runs_reason_check check (outcome_reason is null or outcome_reason in
        ('completed', 'account_type_unsupported', 'permission_declined', 'permission_expired',
         'permission_absent', 'permission_unknown', 'reauthorization_required', 'revoked',
         'owner_mismatch', 'capability_changed', 'budget_exhausted', 'page_limit',
         'provider_retryable', 'response_refused')),
    constraint own_media_sync_runs_terminal_check check (
        (status in ('running', 'retryable') and finished_at is null)
        or (status in ('completed', 'capability_noop', 'failed') and finished_at is not null)
    ),
    constraint own_media_sync_runs_counts_check check (page_count >= 0 and item_count >= 0)
);

create unique index own_media_sync_runs_one_active_per_account
    on instagram_archive.own_media_sync_runs (account_id)
    where status in ('running', 'retryable');

create table instagram_archive.own_media_sync_state (
    account_id                   uuid        primary key,
    watermark_provider_media_id  text,
    next_due_at                  timestamptz not null,
    last_run_id                  uuid,
    last_outcome                 text,
    updated_at                   timestamptz not null,
    constraint own_media_sync_state_account_id_fkey foreign key (account_id)
        references instagram_archive.accounts (account_id),
    constraint own_media_sync_state_last_run_id_fkey foreign key (last_run_id)
        references instagram_archive.own_media_sync_runs (run_id),
    constraint own_media_sync_state_last_outcome_check check (last_outcome is null or last_outcome in
        ('completed', 'capability_noop', 'retryable', 'failed'))
);

create table instagram_archive.own_media_sync_items (
    run_id                     uuid        not null,
    provider_media_id          text        not null,
    owner_provider_account_id  text        not null,
    media_type                 text        not null,
    permalink                  text        not null,
    caption                    text,
    published_at               timestamptz,
    media_url                  text,
    thumbnail_url              text,
    raw_record_id              uuid        not null,
    observed_at                timestamptz not null,
    constraint own_media_sync_items_pkey primary key (run_id, provider_media_id),
    constraint own_media_sync_items_run_id_fkey foreign key (run_id)
        references instagram_archive.own_media_sync_runs (run_id),
    constraint own_media_sync_items_raw_record_id_fkey foreign key (raw_record_id)
        references instagram_archive.raw_records (raw_record_id),
    constraint own_media_sync_items_media_type_check
        check (media_type in ('image', 'video', 'carousel', 'reel'))
);

create table instagram_archive.own_media_authority (
    account_id    uuid        primary key,
    run_id        uuid        not null,
    activated_at  timestamptz not null,
    constraint own_media_authority_account_id_fkey foreign key (account_id)
        references instagram_archive.accounts (account_id),
    constraint own_media_authority_run_id_fkey foreign key (run_id)
        references instagram_archive.own_media_sync_runs (run_id)
);

-- ---------------------------------------------------------------------------------------------
-- captures
-- ---------------------------------------------------------------------------------------------
--
-- An explicit Ratatoskr capture proves the user saved an item TO Ratatoskr at captured_at. It does
-- not prove membership in any native list. Capture identity is the pair (user_ref, canonical_url):
-- a repeated delivery of the same share by the same user reuses the existing row instead of
-- creating a second one.

create table instagram_archive.captures (
    capture_id         uuid        primary key,
    user_ref           uuid        not null,
    media_id           uuid,
    canonical_url      text        not null,
    acquisition_method text        not null,
    saved_authority    text        not null,
    client_source      text        not null,
    status             text        not null,
    note               text,
    client_idempotency_key text,
    captured_at        timestamptz not null,
    next_resolution_at timestamptz,
    created_at         timestamptz not null default now(),
    constraint captures_media_id_fkey foreign key (media_id)
        references instagram_archive.media (media_id),
    constraint captures_acquisition_method_check
        check (acquisition_method in
            ('official_api', 'share_extension', 'browser_extension', 'public_resolution',
             'data_export', 'legacy_import')),
    constraint captures_saved_authority_check
        check (saved_authority in
            ('explicit_user_capture', 'export_observation', 'authoritative_platform_state',
             'legacy_observation')),
    constraint captures_client_source_check
        check (client_source in
            ('ios_share_extension', 'android_share_target', 'browser_extension', 'telegram')),
    constraint captures_status_check
        check (status in ('accepted', 'resolved', 'unavailable', 'failed', 'tombstoned')),
    constraint captures_user_canonical_key unique (user_ref, canonical_url)
);

comment on table instagram_archive.captures is
    'Explicit user captures. media_id stays open while the item is unresolved or unavailable. '
    '(user_ref, canonical_url) is the deduplicating identity; client_idempotency_key records the '
    'platform operation key for correlation and never participates in identity.';

-- ---------------------------------------------------------------------------------------------
-- capture_analysis_links
-- ---------------------------------------------------------------------------------------------

create table instagram_archive.capture_analysis_links (
    capture_id uuid not null,
    content_digest text not null,
    completed_at timestamptz not null,
    constraint capture_analysis_links_pkey primary key (capture_id, content_digest),
    constraint capture_analysis_links_capture_id_fkey foreign key (capture_id)
        references instagram_archive.captures (capture_id)
);

comment on table instagram_archive.capture_analysis_links is
    'Observational linkage from a preserved capture revision to a completed Knowledge analysis. '
    'Analysis result contents remain owned by Knowledge.';

comment on constraint captures_acquisition_method_check on instagram_archive.captures is
    'How the capture reached this service. Closed vocabulary; enforced by the database.';
comment on constraint captures_saved_authority_check on instagram_archive.captures is
    'The authority the capture proves. ExplicitUserCapture is the honest ceiling for a share.';

-- ---------------------------------------------------------------------------------------------
-- capture_notes
-- ---------------------------------------------------------------------------------------------

create table instagram_archive.capture_notes (
    note_id    uuid        primary key,
    capture_id uuid        not null,
    body       text        not null,
    created_at timestamptz not null default now(),
    constraint capture_notes_capture_id_fkey foreign key (capture_id)
        references instagram_archive.captures (capture_id)
);

comment on table instagram_archive.capture_notes is
    'User-authored notes attached to a capture. Private content; never logged or exported to events.';

-- ---------------------------------------------------------------------------------------------
-- export_snapshots
-- ---------------------------------------------------------------------------------------------

create table instagram_archive.export_snapshots (
    snapshot_id       uuid        primary key,
    user_ref          uuid        not null,
    archive_hash      bytea       not null,
    archive_blob_ref  text        not null,
    archive_byte_size bigint      not null,
    received_at       timestamptz not null,
    constraint export_snapshots_user_archive_hash_key unique (user_ref, archive_hash),
    constraint export_snapshots_snapshot_user_key unique (snapshot_id, user_ref),
    constraint export_snapshots_byte_size_check check (archive_byte_size >= 0)
);

comment on table instagram_archive.export_snapshots is
    'One immutable owner-scoped Data Export archive receipt. Bytes remain only in protected content-addressed storage.';

-- ---------------------------------------------------------------------------------------------
-- import_runs
-- ---------------------------------------------------------------------------------------------

create table instagram_archive.import_runs (
    run_id            uuid        primary key,
    snapshot_id       uuid        not null,
    user_ref          uuid        not null,
    state             text        not null,
    detected_layout   text,
    parser_id         text,
    records_processed bigint      not null default 0,
    warning_count     bigint      not null default 0,
    failure_class     text,
    received_at       timestamptz not null,
    updated_at        timestamptz not null,
    finished_at       timestamptz,
    constraint import_runs_snapshot_id_key unique (snapshot_id),
    constraint import_runs_snapshot_id_fkey foreign key (snapshot_id)
        references instagram_archive.export_snapshots (snapshot_id),
    constraint import_runs_snapshot_user_fkey foreign key (snapshot_id, user_ref)
        references instagram_archive.export_snapshots (snapshot_id, user_ref),
    constraint import_runs_state_check
        check (state in ('received', 'inspected', 'parsed', 'reconciled', 'failed')),
    constraint import_runs_counters_check
        check (records_processed >= 0 and warning_count >= 0),
    constraint import_runs_failure_check check (
        (state = 'failed' and failure_class is not null and finished_at is not null)
        or (state <> 'failed' and failure_class is null)
    ),
    constraint import_runs_reconciled_finished_check check (
        state <> 'reconciled' or finished_at is not null
    )
);

comment on table instagram_archive.import_runs is
    'Durable received to inspected to parsed to reconciled state for one immutable archive; failed is terminal.';

create index import_runs_worker_state_idx
    on instagram_archive.import_runs (state, updated_at)
    where state not in ('reconciled', 'failed');

-- Ordered evidence for every accepted compare-and-swap transition. No archive content or path is
-- copied into this operational history.
create table instagram_archive.import_run_transitions (
    transition_id uuid        primary key,
    run_id        uuid        not null,
    ordinal       integer     not null,
    from_state    text,
    to_state      text        not null,
    failure_class text,
    occurred_at   timestamptz not null,
    constraint import_run_transitions_run_ordinal_key unique (run_id, ordinal),
    constraint import_run_transitions_run_id_fkey foreign key (run_id)
        references instagram_archive.import_runs (run_id),
    constraint import_run_transitions_ordinal_check check (ordinal > 0),
    constraint import_run_transitions_state_check check (
        (from_state is null and to_state = 'received')
        or (from_state = 'received' and to_state in ('inspected', 'failed'))
        or (from_state = 'inspected' and to_state in ('parsed', 'failed'))
        or (from_state = 'parsed' and to_state in ('reconciled', 'failed'))
    ),
    constraint import_run_transitions_failure_check check (
        (to_state = 'failed' and failure_class is not null)
        or (to_state <> 'failed' and failure_class is null)
    )
);

comment on table instagram_archive.import_run_transitions is
    'Append-only ordered state transition evidence for a Data Export import run.';

-- Staged known and unknown records. The immutable archive BlobRef plus entry identity and digest
-- is the raw source; payload contains only bounded parsed JSON or normalized metadata.
create table instagram_archive.export_records (
    record_id        uuid        primary key,
    run_id           uuid        not null,
    evidence_key     text        not null,
    record_kind      text        not null,
    category         text        not null,
    entry_path       text        not null,
    entry_digest     bytea,
    entry_byte_size  bigint      not null,
    provider_id      text,
    canonical_url    text,
    payload          jsonb       not null,
    processed_at     timestamptz not null,
    constraint export_records_run_evidence_key unique (run_id, evidence_key),
    constraint export_records_run_id_fkey foreign key (run_id)
        references instagram_archive.import_runs (run_id),
    constraint export_records_record_kind_check check (record_kind in
        ('normalized', 'unknown_record', 'unknown_section', 'conflict', 'warning')),
    constraint export_records_entry_byte_size_check check (entry_byte_size >= 0)
);

comment on table instagram_archive.export_records is
    'Deterministic staged projections and retained unknown/conflict evidence for one import run.';

create table instagram_archive.export_completeness_reports (
    report_id          uuid        primary key,
    run_id             uuid        not null,
    matched            jsonb       not null,
    export_only        jsonb       not null,
    capture_only       jsonb       not null,
    non_comparable     jsonb       not null,
    matched_count      bigint      not null,
    export_only_count  bigint      not null,
    capture_only_count bigint      not null,
    non_comparable_count bigint    not null,
    categories         jsonb       not null,
    warnings           jsonb       not null,
    authority_disclaimer text      not null,
    created_at         timestamptz not null,
    constraint export_completeness_reports_run_id_key unique (run_id),
    constraint export_completeness_reports_run_id_fkey foreign key (run_id)
        references instagram_archive.import_runs (run_id),
    constraint export_completeness_reports_arrays_check check (
        jsonb_typeof(matched) = 'array'
        and jsonb_typeof(export_only) = 'array'
        and jsonb_typeof(capture_only) = 'array'
        and jsonb_typeof(non_comparable) = 'array'
        and jsonb_typeof(categories) = 'array'
        and jsonb_typeof(warnings) = 'array'
    ),
    constraint export_completeness_reports_math_check check (
        matched_count = jsonb_array_length(matched)
        and export_only_count = jsonb_array_length(export_only)
        and capture_only_count = jsonb_array_length(capture_only)
        and non_comparable_count = jsonb_array_length(non_comparable)
    ),
    constraint export_completeness_reports_authority_check check (
        authority_disclaimer = 'An export is one observation; it does not prove complete account history or Instagram native Saved membership, and absence does not prove unsave or deletion.'
    )
);

comment on table instagram_archive.export_completeness_reports is
    'Exact sorted overlap and gap sets. It reports evidence differences and never silently fills or deletes records.';

-- ---------------------------------------------------------------------------------------------
-- availability_observations
-- ---------------------------------------------------------------------------------------------

create table instagram_archive.availability_observations (
    observation_id   uuid        primary key,
    media_id         uuid,
    capture_id       uuid,
    availability     text        not null,
    reason_code      text,
    resolver_version text,
    observed_at      timestamptz not null,
    constraint availability_observations_media_id_fkey foreign key (media_id)
        references instagram_archive.media (media_id),
    constraint availability_observations_capture_id_fkey foreign key (capture_id)
        references instagram_archive.captures (capture_id),
    constraint availability_observations_subject_check
        check (media_id is not null or capture_id is not null),
    constraint availability_observations_availability_check
        check (availability in
            ('available', 'unavailable', 'deleted', 'private', 'temporarily_unavailable',
             'unsupported', 'resolution_failed'))
);

comment on table instagram_archive.availability_observations is
    'Upstream availability over time. Absence of a newer observation never implies deletion.';

-- ---------------------------------------------------------------------------------------------
-- outbox_events
-- ---------------------------------------------------------------------------------------------

create table instagram_archive.outbox_events (
    event_id        uuid        primary key,
    event_type      text        not null,
    aggregate_type  text        not null,
    aggregate_id    uuid        not null,
    payload         jsonb       not null,
    correlation_id  uuid,
    causation_id    uuid,
    occurred_at     timestamptz not null,
    published_at    timestamptz,
    attempt_count   integer     not null default 0,
    next_attempt_at timestamptz,
    constraint outbox_events_aggregate_type_check
        check (aggregate_type in ('capture', 'media', 'account', 'import'))
);

comment on table instagram_archive.outbox_events is
    'Transactional outbox. Rows become at-least-once publications; replay converges.';

create index outbox_events_unpublished_idx
    on instagram_archive.outbox_events (next_attempt_at)
    where published_at is null;

create unique index outbox_events_own_media_content_key
    on instagram_archive.outbox_events
       (aggregate_type, aggregate_id, event_type,
        (payload #>> '{payload,source,content_digest,hex}'))
    where aggregate_type = 'media'
      and event_type in ('social.source.captured.v1', 'social.source.updated.v1');

-- ---------------------------------------------------------------------------------------------
-- inbox_events
-- ---------------------------------------------------------------------------------------------

create table instagram_archive.inbox_events (
    consumer_name  text        not null,
    event_id       uuid        not null,
    consumed_at    timestamptz not null,
    handler_outcome text       not null,
    constraint inbox_events_consumer_name_event_id_pkey primary key (consumer_name, event_id),
    constraint inbox_events_handler_outcome_check
        check (handler_outcome in ('processed', 'rejected', 'skipped'))
);

comment on table instagram_archive.inbox_events is
    'Consumer inbox deduplication under at-least-once delivery.';

-- ---------------------------------------------------------------------------------------------
-- item-9 lifecycle operations
-- ---------------------------------------------------------------------------------------------
--
-- These are first-version definitions, not a migration ledger. Deletion audit is deliberately
-- structural: it contains identifiers, closed classes and counts, never content-bearing JSON.

create table instagram_archive.deletion_operations (
    operation_id uuid        primary key,
    user_ref     uuid        not null,
    target_kind  text        not null,
    target_id    uuid        not null,
    reason       text        not null,
    state        text        not null,
    requested_at timestamptz not null,
    updated_at   timestamptz not null default now(),
    finished_at  timestamptz,
    constraint deletion_operations_target_check
        check (target_kind in ('capture', 'connection')),
    constraint deletion_operations_reason_check
        check (reason in ('user_requested', 'retention_policy')),
    constraint deletion_operations_state_check
        check (state in ('planned', 'database_committed', 'pending_blob_deletion', 'complete', 'failed')),
    constraint deletion_operations_owner_operation_key unique (user_ref, operation_id)
);

comment on table instagram_archive.deletion_operations is
    'Content-free owner deletion audit and idempotency identity; target rows may be physically gone.';

create table instagram_archive.deletion_effects (
    operation_id   uuid   not null,
    data_class     text   not null,
    action         text   not null,
    affected_count bigint not null,
    primary key (operation_id, data_class, action),
    constraint deletion_effects_operation_id_fkey foreign key (operation_id)
        references instagram_archive.deletion_operations (operation_id),
    constraint deletion_effects_data_class_check
        check (data_class ~ '^[a-z][a-z0-9_]{0,63}$'),
    constraint deletion_effects_action_check
        check (action in ('delete', 'detach', 'retain_audit', 'retain_shared', 'not_applicable')),
    constraint deletion_effects_count_check check (affected_count >= 0)
);

comment on table instagram_archive.deletion_effects is
    'Bounded per-class row/blob counts for one deletion; no payload, URL, note, token, or body.';

create table instagram_archive.local_source_removals (
    user_ref         uuid        not null,
    social_source_id uuid        not null,
    capture_id       uuid,
    media_id         uuid,
    operation_id     uuid        not null,
    reason           text        not null,
    removed_at       timestamptz not null,
    primary key (user_ref, social_source_id),
    constraint local_source_removals_operation_id_fkey foreign key (operation_id)
        references instagram_archive.deletion_operations (operation_id),
    constraint local_source_removals_reason_check
        check (reason in ('user_requested', 'retention_policy')),
    constraint local_source_removals_subject_check
        check (num_nonnulls(capture_id, media_id) = 1)
);

comment on table instagram_archive.local_source_removals is
    'Content-free local-library removal guard; distinct from Instagram availability evidence.';

create table instagram_archive.blob_deletion_tasks (
    task_id            uuid        primary key,
    operation_id       uuid,
    blob_ref           text        not null,
    content_hash       bytea       not null,
    byte_size          bigint      not null,
    media_class        text        not null,
    state              text        not null,
    attempt_count      integer     not null default 0,
    last_failure_class text,
    created_at         timestamptz not null default now(),
    updated_at         timestamptz not null default now(),
    completed_at       timestamptz,
    constraint blob_deletion_tasks_operation_id_fkey foreign key (operation_id)
        references instagram_archive.deletion_operations (operation_id),
    constraint blob_deletion_tasks_blob_key unique (blob_ref, content_hash),
    constraint blob_deletion_tasks_state_check check (state in ('pending', 'complete')),
    constraint blob_deletion_tasks_attempt_count_check check (attempt_count >= 0),
    constraint blob_deletion_tasks_byte_size_check check (byte_size >= 0),
    constraint blob_deletion_tasks_media_class_check
        check (media_class in ('provider_media', 'data_export_archive', 'raw_response', 'user_upload')),
    constraint blob_deletion_tasks_failure_class_check
        check (last_failure_class is null or last_failure_class in
            ('storage_unavailable', 'digest_mismatch', 'still_referenced'))
);

comment on table instagram_archive.blob_deletion_tasks is
    'Durable idempotent deletion of an unreferenced Instagram-owned content-addressed object.';

create table instagram_archive.reresolution_runs (
    run_id               uuid        primary key,
    user_ref             uuid        not null,
    state                text        not null,
    max_items            integer     not null,
    max_requests         integer     not null,
    max_response_bytes   bigint      not null,
    max_concurrency      integer     not null,
    deadline_at          timestamptz not null,
    items_admitted       integer     not null default 0,
    requests_reserved    integer     not null default 0,
    response_bytes       bigint      not null default 0,
    started_at           timestamptz not null default now(),
    finished_at          timestamptz,
    constraint reresolution_runs_state_check
        check (state in ('running', 'completed', 'completed_with_failures', 'failed')),
    constraint reresolution_runs_positive_limits_check check (
        max_items > 0 and max_requests > 0 and max_response_bytes > 0 and max_concurrency > 0
    ),
    constraint reresolution_runs_counters_check check (
        items_admitted >= 0 and items_admitted <= max_items
        and requests_reserved >= 0 and requests_reserved <= max_requests
        and response_bytes >= 0 and response_bytes <= max_response_bytes
    )
);

comment on table instagram_archive.reresolution_runs is
    'Finite owner-scoped public re-resolution budget and bounded aggregate outcome.';

create table instagram_archive.reresolution_items (
    run_id          uuid        not null,
    capture_id      uuid        not null,
    state           text        not null,
    skip_reason     text,
    attempt_count   integer     not null default 0,
    response_bytes  bigint      not null default 0,
    claimed_at      timestamptz,
    finished_at     timestamptz,
    primary key (run_id, capture_id),
    constraint reresolution_items_run_id_fkey foreign key (run_id)
        references instagram_archive.reresolution_runs (run_id),
    constraint reresolution_items_state_check
        check (state in ('candidate', 'claimed', 'updated', 'unchanged', 'skipped', 'failed')),
    constraint reresolution_items_skip_reason_check
        check (skip_reason is null or skip_reason in
            ('not_due', 'too_old', 'not_live', 'privacy_terminal', 'unsupported', 'item_budget',
             'request_budget', 'byte_budget', 'deadline', 'concurrency', 'provider_budget')),
    constraint reresolution_items_counters_check check (attempt_count >= 0 and response_bytes >= 0)
);

comment on table instagram_archive.reresolution_items is
    'Deterministic candidate and outcome evidence; capture_id remains after local deletion for audit.';

create table instagram_archive.export_reprocessing_runs (
    reprocessing_run_id uuid        primary key,
    operation_id        uuid        not null,
    import_run_id       uuid        not null,
    user_ref            uuid        not null,
    detected_layout     text        not null,
    parser_id           text        not null,
    state               text        not null,
    plan_fingerprint    text        not null,
    state_fingerprint   text        not null,
    checkpoint_item_key text,
    report              jsonb,
    started_at          timestamptz not null default now(),
    updated_at          timestamptz not null default now(),
    finished_at         timestamptz,
    constraint export_reprocessing_runs_import_run_id_fkey foreign key (import_run_id)
        references instagram_archive.import_runs (run_id),
    constraint export_reprocessing_runs_owner_operation_key unique (user_ref, operation_id),
    constraint export_reprocessing_runs_state_check
        check (state in ('running', 'completed', 'completed_with_warnings', 'failed')),
    constraint export_reprocessing_runs_version_check
        check (length(detected_layout) between 1 and 128 and length(parser_id) between 1 and 128),
    constraint export_reprocessing_runs_fingerprint_check
        check (length(plan_fingerprint) = 64 and length(state_fingerprint) = 64)
);

comment on table instagram_archive.export_reprocessing_runs is
    'Separate parser-version operation over one immutable original export receipt; never a schema migration.';

create table instagram_archive.export_reprocessing_items (
    reprocessing_run_id uuid not null,
    item_key            text not null,
    classification      text not null,
    state               text not null,
    prospective_digest  text,
    applied_digest      text,
    primary key (reprocessing_run_id, item_key),
    constraint export_reprocessing_items_run_id_fkey foreign key (reprocessing_run_id)
        references instagram_archive.export_reprocessing_runs (reprocessing_run_id),
    constraint export_reprocessing_items_item_key_check check (length(item_key) between 1 and 512),
    constraint export_reprocessing_items_classification_check
        check (classification in
            ('normalized', 'unknown_record', 'unknown_section', 'conflict', 'warning', 'omitted')),
    constraint export_reprocessing_items_state_check
        check (state in ('planned', 'applied', 'skipped', 'conflict', 'warning')),
    constraint export_reprocessing_items_digest_check check (
        (prospective_digest is null or length(prospective_digest) = 64)
        and (applied_digest is null or length(applied_digest) = 64)
    )
);

comment on table instagram_archive.export_reprocessing_items is
    'Content-free deterministic checkpoint and digest outcome for one retained-export item.';
