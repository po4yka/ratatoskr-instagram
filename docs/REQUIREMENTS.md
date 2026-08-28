# Instagram connector requirements

## Goals

1. Connect supported professional accounts through official OAuth/capabilities.
2. Archive own accessible media/account records when permitted.
3. Accept explicit user captures from mobile/browser clients.
4. Resolve public posts/reels through supported public mechanisms and preserve provenance.
5. Import user-provided Data Exports safely and version parsers.
6. Retain media references by default and admit bytes only under explicit finite policy.
7. Delete capture or account-connection personal data completely without erasing independent evidence.
8. Refresh recent captures and reprocess retained exports within explicit budgets.

## Non-goals

Automatic authoritative synchronization of native Saved items, private-content bypass,
password/cookie login, hidden API interception, stealth scraping, or legacy monolith import.

## Requirements

- Every record carries acquisition method and saved-authority semantics.
- Capability matrix controls UI and background behavior; unsupported functions fail honestly.
- Explicit capture proves Ratatoskr user intent, not native Saved membership.
- Private/unavailable content remains a URL/note/status unless the user explicitly uploads an artifact.
- Raw export archives and unknown records are retained before normalization.
- External provider writes, if introduced, require separate consent/audit.
- Capture/connection privacy deletion removes owned content and credentials atomically with
  content-free audit, BlobStore work, local resurrection guards, and canonical downstream deletion
  requests; it does not claim downstream completion.
- Re-resolution rechecks owner/privacy state immediately before I/O and cannot exceed item, request,
  response-byte, deadline, concurrency, or provider budgets.
- Parser-version dry-run and apply share one deterministic report; omission never deletes prior
  evidence, and only an exact owner-matching retained receipt/parser pair is accepted.

First slice: explicit public post/reel URL capture -> supported resolution -> SocialSource -> Knowledge indexing -> unavailable-state fallback.
