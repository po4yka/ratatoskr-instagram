-- Run as a PostgreSQL administrator after Platform's `01-database-and-roles.sql` created the shared
-- database `ratatoskr`, and before the first `ratatoskr-instagram` start. Set the password
-- separately with `\password ratatoskr_instagram`; no credential belongs in this repository. The
-- service creates its own `instagram_archive` schema from `schema.sql` at start, which is why the
-- role needs `create` on the database and nothing more.

do $$
begin
    if not exists (select 1 from pg_roles where rolname = 'ratatoskr_instagram') then
        create role ratatoskr_instagram login;
    end if;
end
$$;

alter role ratatoskr_instagram nosuperuser nocreatedb nocreaterole noreplication nobypassrls;
grant connect, create on database ratatoskr to ratatoskr_instagram;

\connect ratatoskr
revoke all on schema public from ratatoskr_instagram;
