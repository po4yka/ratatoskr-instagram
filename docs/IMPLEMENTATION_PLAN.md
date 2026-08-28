# Instagram connector implementation plan

1. Scaffold service, config, telemetry, health, errors, and `instagram_archive` schema.
2. Define capability matrix, acquisition, authority, upstream status, and SocialSource contracts.
3. Implement explicit capture intake, URL canonicalization, idempotency, and unavailable fallback.
4. Implement supported public resolution and raw revision storage.
5. Publish SocialSource and integrate with Knowledge.
6. Add encrypted official account OAuth and capability discovery.
7. Add own-media synchronization for supported account types. **Complete.**
8. Implement immutable safe Data Export intake, versioned parser, and completeness report.
9. Add media policy, privacy deletion, re-resolution, and restartable parser reprocessing tooling. **Complete.**
10. Add separately approved provider writes only after explicit ADR/consent design.

Definition of Done: provenance is truthful, no hidden session access exists, archives are safely parsed, private data authorized, schema/events/tests and the planned workspace capture flow pass.
