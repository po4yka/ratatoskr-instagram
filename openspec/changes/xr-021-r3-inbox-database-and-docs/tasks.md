## Tasks

Behaviour tasks run against a private `nats-server` that the test spawns itself (`nats-server` on `PATH`) and the disposable-database harness. Cross-repository behaviour is defined in XR-021 CONTRACTS.md R3-01, R3-11 and R3-19.

- [x] 1.1 RED: Add `services/instagram-archive/tests/inbox_prefix.rs::the_lane_completes_jetstream_requests_under_its_inbox_prefix`, a broker helper that runs the stanza read from `deploy/nats/identity.conf`, and `deploy/nats/identity.conf` itself, with a signature-only `nats_transport::connect_bus` that connects without the inbox prefix. Expected failure: the durable verification or the publish times out because the stanza lets the identity subscribe only to `_INBOX_instagram.>`.
- [x] 1.2 GREEN: `connect_bus` applies `custom_inbox_prefix(INBOX_PREFIX)` to both the nkey and the anonymous options; `BusLane::connect` calls it.
- [x] 2.1 RED: Add `services/instagram-archive/tests/boot.rs::the_example_names_the_shared_database_and_the_role_file_exists`. Expected failure: the example names `ratatoskr_instagram` as the database and `deploy/postgres/role.sql` does not exist.
- [x] 2.2 GREEN (configuration and documentation): edit the example, add `deploy/postgres/role.sql` and a README deployment paragraph.
- [x] 3.1 Documentation (no failing test can precede it): rewrite the README workspace-integration paragraph from the workspace as it is (R3-19).
- [ ] 4.1 Gate: run the full command list from `DEVELOPMENT.md`, `cargo deny --locked check`, the 850-line check, `git diff --check` and `openspec validate --all --strict`; tick tasks only with evidence.
