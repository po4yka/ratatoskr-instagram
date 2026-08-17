# Instagram connector requirements

## Goals

1. Connect supported professional accounts through official OAuth/capabilities.
2. Archive own accessible media/account records when permitted.
3. Accept explicit user captures from mobile/browser clients.
4. Resolve public posts/reels through supported public mechanisms and preserve provenance.
5. Import user-provided Data Exports safely and version parsers.

## Non-goals

Automatic authoritative synchronization of native Saved items, private-content bypass, password/cookie login, hidden API interception, or stealth scraping.

## Requirements

- Every record carries acquisition method and saved-authority semantics.
- Capability matrix controls UI and background behavior; unsupported functions fail honestly.
- Explicit capture proves Ratatoskr user intent, not native Saved membership.
- Private/unavailable content remains a URL/note/status unless the user explicitly uploads an artifact.
- Raw export archives and unknown records are retained before normalization.
- External provider writes, if introduced, require separate consent/audit.

First slice: explicit public post/reel URL capture -> supported resolution -> SocialSource -> Knowledge indexing -> unavailable-state fallback.
