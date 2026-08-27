# Security Policy for Ratatoskr Instagram

Report vulnerabilities privately. Do not publish access tokens, private media, user exports, screenshots, account identifiers, or production provider responses.

Security review is required for OAuth, account capabilities, capture URLs, public resolution/oEmbed, private/unavailable content, media download, Data Export parsing, archive limits, external references, deletion, and logging.

Baseline: no server-side password/cookie/session automation; least-privilege official API access; explicit user capture; validate URLs and archives; bound bytes/files/decompression/time; treat captions/media metadata as hostile; owner-authorize all data; preserve provenance and uncertainty; never bypass provider privacy controls.

Own-media scheduling is opt-in, capability-gated before credential opening, and restricted to the
connected account's official media edge. Page bodies are bounded and strictly parsed; foreign
owners and ephemeral/story shapes are refused. Provider media URLs are not BlobRefs and media bytes
are not downloaded. Partial runs retain the previous authority and watermark, while completion
revalidates owner, provider identity, connection, and capability generation atomically.
