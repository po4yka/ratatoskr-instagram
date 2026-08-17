# Security Policy for Ratatoskr Instagram

Report vulnerabilities privately. Do not publish access tokens, private media, user exports, screenshots, account identifiers, or production provider responses.

Security review is required for OAuth, account capabilities, capture URLs, public resolution/oEmbed, private/unavailable content, media download, Data Export parsing, archive limits, external references, deletion, and logging.

Baseline: no server-side password/cookie/session automation; least-privilege official API access; explicit user capture; validate URLs and archives; bound bytes/files/decompression/time; treat captions/media metadata as hostile; owner-authorize all data; preserve provenance and uncertainty; never bypass provider privacy controls.
