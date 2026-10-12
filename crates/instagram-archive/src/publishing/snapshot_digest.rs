//! The content digest of an Instagram source snapshot (XR-021 CONTRACTS.md R5-09).
//!
//! The digest says what the snapshot states about the post, so a changed caption or a post
//! deleted upstream is a different digest and Knowledge, which keys its analysis revision by
//! `(tenant, source, digest)`, analyses the update.

use ratatoskr_identifiers::{ContentDigest, DigestAlgorithm, DigestHex, canonical_json};
use ratatoskr_social_contracts::{
    PostPermalink, PostText, SocialMediaItem, SocialSourceSnapshot, UpstreamAvailability,
};
use sha2::Digest as _;

/// The domain separator of the preimage, so no other digest of this service can collide with it.
const DOMAIN: &str = "instagram.snapshot-content";

/// What the digest is computed over, in the declaration order that the canonical rendering keeps.
#[derive(serde::Serialize)]
struct Preimage<'a> {
    domain: &'static str,
    permalink: Option<&'a str>,
    external_post_id: &'a str,
    caption: Option<&'a str>,
    upstream: &'static str,
    media: &'a [SocialMediaItem],
}

const fn upstream_token(availability: UpstreamAvailability) -> &'static str {
    match availability {
        UpstreamAvailability::Available => "available",
        UpstreamAvailability::DeletedUpstream => "deleted",
        _ => "unavailable",
    }
}

/// The digest of what `snapshot` states about the post: SHA-256 over the canonical JSON of its
/// domain, permalink, provider shortcode, caption, upstream availability and media.
///
/// The caption is the snapshot's text, so an empty stored caption and a missing one are the same
/// fact. Capture time, provenance and the raw blob are not content and never enter the digest.
///
/// # Panics
///
/// Never in practice: the preimage holds strings, an enum token and contract values that always
/// serialize, and a SHA-256 hex rendering always parses.
#[must_use]
#[expect(
    clippy::expect_used,
    reason = "the preimage holds only strings, an enum token and contract values that serialize; \
              a failure here means the contracts' own types stopped serializing"
)]
pub fn snapshot_content_digest(snapshot: &SocialSourceSnapshot) -> ContentDigest {
    let preimage = Preimage {
        domain: DOMAIN,
        permalink: snapshot.permalink.as_ref().map(PostPermalink::as_str),
        external_post_id: snapshot.external_post_id.as_str(),
        caption: snapshot.text.as_ref().map(PostText::as_str),
        upstream: upstream_token(snapshot.upstream_availability),
        media: &snapshot.media,
    };
    let rendered = canonical_json(&preimage).expect("the digest preimage serializes");
    let hex = format!("{:x}", sha2::Sha256::digest(rendered.as_bytes()));
    ContentDigest {
        algorithm: DigestAlgorithm::Sha256,
        hex: DigestHex::parse(&hex).expect("a SHA-256 hex rendering parses"),
    }
}
