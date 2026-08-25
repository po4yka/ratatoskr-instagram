# capture-intake Specification

## Purpose
Accepts an explicit user capture of one Instagram permalink: it canonicalizes the URL forms clients deliver into one stable permalink, deduplicates submissions by capture identity, stores the attempt with honest provenance, and records an unavailable outcome truthfully instead of fabricating content when resolution fails.

## Requirements

### Requirement: Delivered URL forms canonicalize to one stable permalink

The intake SHALL accept the Instagram URL forms clients actually deliver for posts, reels and IGTV videos — either URL scheme, the hosts `instagram.com`, `www.instagram.com`, `m.instagram.com`, `l.instagram.com`, `instagr.am`, and `www.instagr.am`, the path forms `/p/<shortcode>`, `/reel/<shortcode>`, `/reels/<shortcode>`, and `/tv/<shortcode>`, optionally prefixed by a username segment, with any query string or fragment — and SHALL canonicalize every accepted form to exactly `https://www.instagram.com/{p|reel|tv}/{shortcode}/`: scheme upgraded to `https`, host normalized to `www.instagram.com`, username prefixes removed, the `/reels/` alias normalized to `/reel/`, query strings and fragments stripped, and a trailing slash applied. Shortcode case SHALL be preserved. Canonicalization SHALL be deterministic: the same input always yields the same canonical permalink.

#### Scenario: Canonicalization across the delivered URL forms

- **WHEN** each URL in the documented acceptance table is canonicalized — plain and www hosts, http and https schemes, mobile and link-shim hosts, the `instagr.am` domains, all four path forms, username-prefixed paths, URLs carrying tracking queries and fragments, and forms with and without trailing slashes
- **THEN** every result is the exact expected canonical permalink recorded in the table, and repeating the canonicalization yields the identical string

#### Scenario: Shortcode case survives canonicalization

- **WHEN** a URL whose shortcode mixes upper-case, lower-case, digits, `-` and `_` is canonicalized
- **THEN** the canonical permalink carries that shortcode byte for byte, without case folding

### Requirement: Non-permalink URLs are refused with a typed reason

The intake SHALL refuse a URL that is not a canonicalizable post, reel, or IGTV permalink, and the refusal SHALL carry a typed reason that distinguishes a malformed URL, a non-Instagram host, a supported-host path that is not a permalink (profiles, stories, explore, login), and a missing or invalid shortcode. Refused URLs SHALL NOT create a capture record.

#### Scenario: Unsupported URL shapes are rejected by class

- **WHEN** a profile page, a story URL, an explore or tag page, a login redirect, a foreign host, a URL with credentials or an explicit port, and a permalink path with an empty or over-long shortcode are each submitted
- **THEN** each refusal names its reason class and no capture row exists for the submission

### Requirement: Capture identity is the user together with the canonical URL

A capture SHALL be identified by the pair `(user_ref, canonical_url)`. A submission that canonicalizes to the same URL for the same user SHALL reuse the existing capture record — same capture id, original captured time, original note — regardless of a different captured-at timestamp, note, client source, or platform idempotency key. Submissions differing in user or in canonical URL SHALL be distinct captures. Reuse SHALL hold under concurrent duplicate submissions: exactly one capture record exists afterwards.

#### Scenario: A duplicate submission reuses the existing capture

- **WHEN** the same user submits a second request that canonicalizes to the URL of an existing capture, carrying a different captured time, note, client source, and idempotency key
- **THEN** the intake reports reuse of the original capture id and no second capture row exists

#### Scenario: Different users and different URLs are different captures

- **WHEN** two users submit the same canonical URL, or one user submits two distinct canonical permalinks
- **THEN** each submission produces its own capture record

#### Scenario: Concurrent duplicates converge on one record

- **WHEN** two identical submissions race against each other
- **THEN** both succeed and exactly one capture record exists

### Requirement: Created captures carry explicit-capture provenance and correlation

Every created capture SHALL record acquisition provenance fixed at `explicit_user_capture` authority, the wire acquisition method implied by the client source (`share_extension` for the iOS share extension and Android share target, `browser_extension` for the browser extension), the client source itself, the canonical URL, and the user-supplied captured time. An optional platform idempotency key SHALL be stored for correlation but SHALL never participate in capture identity. A client source with no wire acquisition method in the current contract grammar SHALL be refused until a reviewed change extends that vocabulary.

#### Scenario: Provenance on a fresh capture

- **WHEN** a capture is created from each supported client source
- **THEN** the record's saved authority is `explicit_user_capture`, its acquisition method is the value implied by that client source, and its captured time equals the supplied instant

#### Scenario: Client sources without a contract acquisition method are refused

- **WHEN** a submission arrives with client source `telegram`
- **THEN** the intake refuses it with an explicit not-supported reason and stores nothing

### Requirement: Unavailable resolution preserves the attempt truthfully

When resolution of a captured source fails — deleted, private, region-limited, transiently down, unsupported shape, or unresolved failure — the service SHALL append an availability observation against the capture carrying the observed kind and observation time, and SHALL move the capture to status `unavailable`. The fallback SHALL preserve the capture's canonical URL, captured time, and note unchanged, SHALL create no media row, and SHALL never fabricate content. Observations accumulate: a further unavailability appends another observation. The transition applies to captures in `accepted` or `resolved` state; an unknown capture id is an error, not a silent write.

#### Scenario: The fallback record keeps what the user actually did

- **WHEN** an unavailable outcome of kind `private` is recorded against a fresh capture
- **THEN** the capture's status becomes `unavailable`, an observation row exists with availability `private` bound to that capture, and the canonical URL, captured time, and note are byte-identical to what was submitted, with no media row anywhere

#### Scenario: Unavailability observations accumulate without fabricating content

- **WHEN** an unavailability is recorded twice against the same capture with different kinds and times
- **THEN** two observations exist in order, the capture remains `unavailable`, and still no media row exists

#### Scenario: The fallback refuses unknown captures

- **WHEN** an unavailability is recorded for a capture id that does not exist
- **THEN** the call fails with a not-found error and writes nothing

### Requirement: The HTTP intake surface speaks the platform capture grammar

The service process SHALL expose `POST /v1/captures` on a dedicated product listener accepting the documented grammar — `platform` set to `instagram`, `canonical_url`, RFC 3339 `captured_at`, `source`, optional `note`, optional `Idempotency-Key` header — and answering `201` with the created capture for a first submission, `200` with the reused capture for a duplicate, and `400` with a machine-readable error code for a refused submission (unknown platform, unsupported URL, unsupported client source, unparsable body or timestamp). Responses SHALL NOT echo database errors or internal details.

#### Scenario: First submission and replay over HTTP

- **WHEN** the grammar-shaped request is posted and then posted again unchanged
- **THEN** the first answer is `201` naming the new capture and its canonical URL, and the replay answers `200` with the same capture id marked as reused

#### Scenario: Refused submissions answer with typed codes

- **WHEN** requests arrive with an unknown platform, a profile URL instead of a permalink, client source `telegram`, or an unparsable timestamp
- **THEN** each answers `400` with the error code naming the refusal class, and no capture row exists for any of them
