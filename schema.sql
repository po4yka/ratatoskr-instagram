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
    scopes              text        not null,
    connected_at        timestamptz not null,
    updated_at          timestamptz not null default now(),
    constraint accounts_provider_account_id_key unique (provider_account_id),
    constraint accounts_account_type_check
        check (account_type in ('business', 'creator', 'personal')),
    constraint accounts_connection_status_check
        check (connection_status in ('connected', 'reauthorization_required', 'revoked'))
);

comment on table instagram_archive.accounts is
    'Connected Instagram accounts. user_ref names the Ratatoskr owner and crosses no schema.';

-- ---------------------------------------------------------------------------------------------
-- credentials
-- ---------------------------------------------------------------------------------------------
--
-- Provider token material, encrypted inside this bounded context. No column holds plaintext.

create table instagram_archive.credentials (
    credential_id             uuid        primary key,
    account_id                uuid        not null,
    access_token_ciphertext   bytea       not null,
    token_version             integer     not null,
    scopes                    text        not null,
    refresh_token_ciphertext  bytea,
    expires_at                timestamptz,
    rotated_at                timestamptz,
    created_at                timestamptz not null default now(),
    constraint credentials_account_id_fkey foreign key (account_id)
        references instagram_archive.accounts (account_id)
);

comment on table instagram_archive.credentials is
    'Encrypted OAuth token material for one account. Ciphertext only, versioned for rotation.';

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
    observed_at   timestamptz not null,
    constraint raw_records_record_kind_check
        check (record_kind in
            ('oembed_response', 'api_response', 'export_section', 'unknown_export_record',
             'user_upload'))
);

comment on table instagram_archive.raw_records is
    'Content-addressed raw evidence. Bodies live in the BlobStore; rows reference them.';

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
    media_id           uuid        primary key,
    account_id         uuid,
    provider_media_id  text,
    permalink          text        not null,
    media_type         text        not null,
    caption            text,
    published_at       timestamptz,
    acquisition_method text        not null,
    saved_authority    text        not null,
    upstream_status    text        not null,
    created_at         timestamptz not null default now(),
    updated_at         timestamptz not null default now(),
    constraint media_provider_media_id_key unique (provider_media_id),
    constraint media_permalink_key unique (permalink),
    constraint media_account_id_fkey foreign key (account_id)
        references instagram_archive.accounts (account_id),
    constraint media_media_type_check
        check (media_type in ('image', 'video', 'carousel', 'reel', 'story')),
    constraint media_acquisition_method_check
        check (acquisition_method in
            ('official_api', 'share_extension', 'browser_extension', 'public_resolution',
             'data_export', 'legacy_import')),
    constraint media_saved_authority_check
        check (saved_authority in
            ('explicit_user_capture', 'export_observation', 'authoritative_platform_state',
             'legacy_observation')),
    constraint media_upstream_status_check
        check (upstream_status in ('available', 'unavailable', 'deleted', 'private', 'unknown'))
);

comment on table instagram_archive.media is
    'Resolved media sources with mandatory acquisition and saved-authority provenance.';

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
-- captures
-- ---------------------------------------------------------------------------------------------
--
-- An explicit Ratatoskr capture proves the user saved an item TO Ratatoskr at captured_at. It does
-- not prove membership in any native list. Deduplication keys arrive with capture intake (plan
-- item 3) and extend this definition in place.

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
    captured_at        timestamptz not null,
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
        check (status in ('accepted', 'resolved', 'unavailable', 'failed'))
);

comment on table instagram_archive.captures is
    'Explicit user captures. media_id stays open while the item is unresolved or unavailable.';

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
    snapshot_id      uuid        primary key,
    user_ref         uuid        not null,
    archive_hash     bytea       not null,
    blob_ref         text        not null,
    detected_version text,
    parser_version   text        not null,
    received_at      timestamptz not null,
    constraint export_snapshots_archive_hash_key unique (archive_hash)
);

comment on table instagram_archive.export_snapshots is
    'One immutable Data Export archive: its hash, its BlobStore reference, and who parsed it.';

-- ---------------------------------------------------------------------------------------------
-- import_runs
-- ---------------------------------------------------------------------------------------------

create table instagram_archive.import_runs (
    run_id              uuid        primary key,
    snapshot_id         uuid        not null,
    outcome             text        not null,
    records_processed   bigint      not null default 0,
    warnings_summary    text,
    completeness_report jsonb,
    started_at          timestamptz not null default now(),
    finished_at         timestamptz,
    constraint import_runs_snapshot_id_fkey foreign key (snapshot_id)
        references instagram_archive.export_snapshots (snapshot_id),
    constraint import_runs_outcome_check
        check (outcome in ('running', 'completed', 'completed_with_warnings', 'failed'))
);

comment on table instagram_archive.import_runs is
    'One restartable parse/reconcile pass over a snapshot, with its completeness evidence.';

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
