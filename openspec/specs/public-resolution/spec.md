# public-resolution Specification

## Purpose
Resolves supported public permalinks (posts, reels, IGTV) through the single approved official embed/oEmbed-style surface, preserves every raw response as an immutable parser-versioned revision before normalization, produces deterministic normalized media records linked to those revisions, and reports unsupported or unavailable sources truthfully instead of fabricating content.

## Requirements

### Requirement: A supported permalink resolves through the approved public surface into a preserved raw revision

The service SHALL resolve canonical Instagram permalinks (`/p/<shortcode>/`, `/reel/<shortcode>/`, `/tv/<shortcode>/`) by fetching their public representation through the approved official embed/oEmbed-style surface and no other access path: no private-page scraping, no authenticated crawling, no browser-session state. A successful resolution SHALL store the raw response body byte for byte as content-addressed raw evidence of kind `oembed_response` before any normalization, together with its content hash, byte size, observation time, and the version of the parser that will interpret it. One successful resolution SHALL append exactly one new immutable revision bound to the resolved source; nothing SHALL update or delete an existing revision.

#### Scenario: A resolved permalink leaves a raw revision behind

- **WHEN** the approved surface answers a supported permalink with a recorded payload and the resolver runs against a fresh archive
- **THEN** one raw evidence row of kind `oembed_response` exists whose bytes equal the payload exactly, whose content hash matches those bytes, and which carries the parser version and observation time of this attempt

### Requirement: Re-resolution appends revisions and never overwrites history

Resolving the same permalink again SHALL append a further revision rather than mutate any prior one. Every earlier revision SHALL keep its original raw payload reference, content hash, parser version, and observation time byte-identical after later resolutions, and the normalized source SHALL link to the most recent revision while all older revisions remain retrievable in order.

#### Scenario: A second resolution leaves the first revision untouched

- **WHEN** a permalink is resolved once and then resolved again against a changed recorded payload
- **THEN** two revisions exist in resolution order, the first revision's stored fields are identical to what the first attempt wrote, and the second carries the new payload under its own identity

#### Scenario: The normalized source points at the newest revision

- **WHEN** two resolutions of one permalink have completed
- **THEN** the normalized media row links to the second revision and both revisions remain bound to that media row in order

### Requirement: Normalization from a recorded payload is deterministic

Normalizing a raw payload SHALL be deterministic: parsing the same recorded payload always yields the same normalized field values, independent of how often it is parsed or when. The normalized media record SHALL project only what the documented surface grammar exposes and what its storage owns — the media type implied by the permalink kind, and the title text when the grammar carries one. Fields the grammar does not define, and evidence the normalized store does not model (author identity, dimensions, thumbnails, embed markup), SHALL survive only inside the raw revision, never guessed into a normalized column.

#### Scenario: The same fixture parses identically twice

- **WHEN** one recorded oEmbed payload is normalized twice within one archive session
- **THEN** both normalizations produce field-for-field identical values, and repeating the whole resolve-and-store cycle a second time yields a normalized media row equal to the first except for its new revision linkage

#### Scenario: Unknown payload fields stay out of the normalized record

- **WHEN** a recorded payload carries fields outside the documented surface grammar
- **THEN** the normalized media record equals the record produced from the same payload with those fields removed, and the raw revision still contains the complete original bytes

### Requirement: Normalized records carry truthful provenance and capture linkage

A media row produced by public resolution SHALL record acquisition method `public_resolution`, saved authority `explicit_user_capture` — the mode's ceiling: observing upstream content proves nothing about native saved state — and an upstream status collapsed from the resolution observation. When the resolution belongs to an existing capture, the capture SHALL link to that media row and take status `resolved`; its captured time, note, and canonical URL remain untouched.

#### Scenario: Provenance on a publicly resolved media row

- **WHEN** a supported permalink resolves successfully on behalf of an existing accepted capture
- **THEN** the media row shows acquisition method `public_resolution`, saved authority `explicit_user_capture`, upstream status `available`, and the capture row links to it with status `resolved` while its captured time and note are byte-identical to submission

### Requirement: Unsupported and failed outcomes are reported truthfully

When the approved surface reports that an object cannot be resolved through it, the service SHALL append an availability observation of kind `unsupported` and learn nothing else: no media row, no revision. Every other reported failure keeps its own kind — `deleted`, `private`, `temporarily_unavailable`, `unavailable` for unclassified failures, and `resolution_failed` when the attempt fails before classification was possible — and none may be rewritten to another kind, in particular not to `deleted`. A failed outcome SHALL leave any prior successful revisions intact and move the capture to status `unavailable`.

#### Scenario: An unsupported object fabricates nothing

- **WHEN** the approved surface reports that the requested object type is not resolvable through it
- **THEN** an observation of kind `unsupported` is appended against the capture, the capture becomes `unavailable`, and no media row and no revision exist anywhere

#### Scenario: Failure kinds survive verbatim without inventing deletion

- **WHEN** resolutions against recorded payloads end in the private, transiently-unavailable, and transport-failure outcomes in turn
- **THEN** three observations exist carrying `private`, `temporarily_unavailable`, and `resolution_failed` respectively, no observation claims `deleted`, and still no media row or revision exists
