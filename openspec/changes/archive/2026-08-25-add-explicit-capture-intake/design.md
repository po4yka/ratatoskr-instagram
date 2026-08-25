# Design: explicit capture intake

## Context

Plan item 2 left behind: provenance types (`capability.rs`) whose `ExplicitCapture` mode is `Planned`; an owned schema with a `captures` table whose comment reserves dedup keys for this item; a service process with one loopback operator listener; strict typed configuration; a test harness that builds disposable databases from `schema.sql` per test. The binding development status forbids migrations and any second API version. The wire acquisition vocabulary is pinned value-for-value to `ratatoskr-social-contracts@361fe94` by an alignment test.

See proposal.md for motivation.

## Goals / Non-Goals

**Goals:**

- One canonicalization implementation that every later lane (resolver, importer, events) can reuse.
- Duplicate-share convergence enforced by the database, not by caller discipline.
- An intake record that stays truthful when resolution fails.
- A product HTTP surface shaped exactly like the documented platform grammar so platform integration needs no translation layer.

**Non-Goals:**

- Fetching anything from Instagram (plan item 4 owns resolution).
- Media storage policy (plan item 9).
- Caller authentication machinery; see the boundary decision below.
- Event publication and outbox wiring (plan item 5).

## Decisions

### Capture identity is `(user_ref, canonical_url)`, not a derived hash key

A UNIQUE constraint over the pair makes duplicate convergence a database property, deterministic across processes and versions with zero algorithm choices to defend. Deriving a SHA-256 dedup key over the same pair adds a dependency and a domain-separation discussion while encoding strictly less information than the pair itself. The prompt's "deterministic idempotency keys" is satisfied by identity determinism: same logical capture in, same row out. Alternative considered (hash column + unique index) rejected as indirection without a payoff at this scale; if URL length ever made text indexes painful, the hash derivation becomes a contained follow-up.

### The platform `Idempotency-Key` is stored for correlation, never identity

The grammar carries it, so intake accepts and persists it (`client_idempotency_key`, nullable). Including it in identity would break accidental double-share convergence — two deliveries of the same share carry different platform keys by design. Identity stays `(user_ref, canonical_url)`; the stored key answers "which platform operation did this come from" in audits.

### Canonical permalink form: `https://www.instagram.com/{p|reel|tv}/{shortcode}/`

One output shape for every accepted input: scheme upgraded, host folded to `www.instagram.com`, username prefixes dropped (post identity is the shortcode), `/reels/` normalized to its canonical `/reel/` alias, query/fragment stripped (permalinks carry no load-bearing parameters), trailing slash appended, shortcode case preserved (the shortcode alphabet is case-sensitive). `/tv/` is kept distinct rather than rewritten: rewriting it to `/p/` or `/reel/` would guess at provider redirect behavior this service must not depend on; the form is accepted and preserved until evidence justifies more. Hosts accepted: the four Instagram hosts plus both `instagr.am` domains. Everything else — foreign hosts, profiles, stories, explore, login, credentials or ports in the authority, empty or oversized shortcodes — is refused by class.

### Timestamps via the `time` crate through sqlx

`captured_at` is an RFC 3339 instant in the grammar and a `timestamptz` in the schema. `time::OffsetDateTime` with sqlx's `time` feature binds it natively and validates at parse time; string pass-through would let PostgreSQL's looser parser accept non-RFC3339 shapes the grammar does not mean. Exact-pinned per repository convention.

### Intake persistence lives on `Database`, not a new store type

The crate has exactly one storage abstraction and one pool. `submit_capture` and `record_capture_unavailable` are inherent async methods in a `capture` module (`impl Database`), matching how the codebase already grows surface without new layers. Insert uses `ON CONFLICT DO NOTHING` followed by a read of the winner inside one transaction, which makes the reuse path race-free without application locks.

### Unavailable fallback writes an observation and flips status; nothing else

`record_capture_unavailable(capture_id, kind, observed_at)` appends to `availability_observations` (capture-bound, media NULL — no media row exists yet and none may be invented) and sets `status = 'unavailable'` where the current status is `accepted` or `resolved`. Unknown ids error. Repeats append history. The note, URL, and captured time are never touched: the fallback preserves the attempt exactly as the user made it.

### Product listener as a second loopback plane with its own config key

Operator routes stay alone on their listener; captures get `RATATOSKR__API__LISTEN_ADDRESS` (default `127.0.0.1:9083`, loopback-only like admin). One axum router per plane keeps exposure decisions separate — the operator plane is probe-only forever.

**Authentication boundary, stated honestly:** the endpoint trusts its caller to name the acting `user_ref`. In the fleet, that caller is `ratatoskr-platform` speaking service-to-service over the internal network; user-facing authentication lives there, not here. Until eventing/platform integration lands, the listener's loopback default is the deployment guardrail, and this boundary is recorded here and in README rather than papered over with placeholder auth.

### Telegram client source is refused until the contract vocabulary grows

`captures_client_source_check` admits `telegram` as delivery metadata, but the acquisition-method grammar pinned to the contracts has no honest value for it (`share_extension`/`browser_extension` would misdescribe the channel). Intake therefore refuses `telegram` with an explicit not-supported reason; extending the vocabulary is a reviewed cross-repo change, not a local mapping.

### Capability flip is part of this change

The matrix is a commitment device: landing the lane without flipping `ExplicitCapture` to `Supported` would leave docs claiming less than the tree does — the same defect class as claiming more. The flip is test-first like everything else, and `docs/CAPABILITY_MATRIX.md`'s row moves with it.

## Risks / Trade-offs

- [Duplicate submissions cannot update captured_at/note] → Deliberate: reuse means the first attempt wins, matching "retries converge". A user who wants a new record for the same URL gets one only if the product later asks for multi-capture semantics (AGENTS.md leaves that door open).
- [Loopback-only defaults could mask a missing auth layer when the listener is opened up] → The boundary is written in design, README, and spec; opening the listener is a deployment act that cannot happen silently through config review.
- [`l.instagram.com` link-shim acceptance] → Its path identifies the same media; refusing it would break real shares. Tracking context is stripped with the rest of the URL.
- [PostgreSQL loose timestamp parsing bypassed] → Mitigated by parsing RFC 3339 in Rust before anything reaches SQL.

## Migration Plan

Schema edits apply in place on the next fresh database (binding development status: no database holds data that must survive). Rollback is reverting the branch; nothing external consumes the surface yet.

## Open Questions

None deferred: `/tv/` preservation and telegram refusal are recorded decisions revisitable with evidence, not unknowns blocking the work.
