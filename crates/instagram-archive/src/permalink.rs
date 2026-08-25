//! Canonicalization of client-delivered Instagram URLs into stable permalinks.
//!
//! Every accepted form — either scheme, the four Instagram hosts plus both
//! `instagr.am` domains, `/p/`, `/reel/`, `/reels/`, and `/tv/` paths with an
//! optional username prefix, any query string or fragment — collapses to
//! exactly `https://www.instagram.com/{p|reel|tv}/{shortcode}/`. Path forms
//! are case-sensitive because Instagram serves them lower-case; shortcodes
//! are case-sensitive because their alphabet is. `/tv/` stays distinct rather
//! than being rewritten: rewriting would guess at provider redirect behavior.

/// The longest shortcode this service accepts. Real shortcodes are far
/// shorter; the bound exists so pathological input is refused, not parsed.
const SHORTCODE_MAX: usize = 64;

/// The canonical path form of a permalink.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PermalinkKind {
    /// `/p/<shortcode>` — a post.
    Post,
    /// `/reel/<shortcode>` — a reel.
    Reel,
    /// `/tv/<shortcode>` — a legacy IGTV video.
    Igtv,
}

impl PermalinkKind {
    /// The canonical path segment for this kind.
    #[must_use]
    pub const fn path_segment(self) -> &'static str {
        match self {
            Self::Post => "p",
            Self::Reel => "reel",
            Self::Igtv => "tv",
        }
    }
}

/// Why a URL is not a canonicalizable Instagram permalink.
///
/// The classes are closed and typed so refusals can be named to clients and
/// counted in telemetry without string matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PermalinkError {
    /// Not an absolute `http`/`https` URL at all, or carries invalid characters.
    Malformed,
    /// The host is not one of the accepted Instagram hosts.
    ForeignHost,
    /// An accepted host, but the path is not a post, reel, or IGTV permalink.
    NotAPermalink,
    /// The authority carries credentials or an explicit port.
    UnsupportedAuthority,
    /// A permalink-shaped path whose shortcode is missing, invalid, or oversized.
    InvalidShortcode,
}

impl std::fmt::Display for PermalinkError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = match self {
            Self::Malformed => "not an absolute http(s) URL",
            Self::ForeignHost => "not an Instagram host",
            Self::NotAPermalink => "an Instagram host, but not a post, reel, or IGTV permalink",
            Self::UnsupportedAuthority => "the URL authority carries credentials or a port",
            Self::InvalidShortcode => "missing, invalid, or oversized shortcode",
        };
        formatter.write_str(reason)
    }
}

impl std::error::Error for PermalinkError {}

/// One canonicalized Instagram permalink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalPermalink {
    /// The stable canonical form: `https://www.instagram.com/{form}/{shortcode}/`.
    pub url: String,
    /// Which canonical path form the permalink uses.
    pub kind: PermalinkKind,
    /// The shortcode exactly as delivered, case preserved.
    pub shortcode: String,
}

/// Canonicalizes one client-delivered Instagram URL into its stable permalink.
///
/// Deterministic: the same input always yields the same canonical permalink.
///
/// # Errors
///
/// Returns the typed [`PermalinkError`] class for every input that is not a
/// canonicalizable post, reel, or IGTV permalink.
pub fn canonicalize(input: &str) -> Result<CanonicalPermalink, PermalinkError> {
    let trimmed = input.trim();
    if trimmed.is_empty() || invalid_character(trimmed) {
        return Err(PermalinkError::Malformed);
    }

    let (scheme, rest) = trimmed.split_once("://").ok_or(PermalinkError::Malformed)?;
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")) {
        return Err(PermalinkError::Malformed);
    }

    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    if authority.is_empty() {
        return Err(PermalinkError::Malformed);
    }
    if authority.contains('@') || authority.contains(':') {
        return Err(PermalinkError::UnsupportedAuthority);
    }

    match_host(authority)?;

    let path = tail.split(['?', '#']).next().unwrap_or_default();
    let permalink = parse_path(path)?;
    Ok(CanonicalPermalink {
        url: format!(
            "https://www.instagram.com/{}/{}/",
            permalink.kind.path_segment(),
            permalink.shortcode
        ),
        kind: permalink.kind,
        shortcode: permalink.shortcode,
    })
}

struct ParsedPath {
    kind: PermalinkKind,
    shortcode: String,
}

/// Folds an accepted host onto the canonical host; refuses everything else.
fn match_host(authority: &str) -> Result<(), PermalinkError> {
    const ACCEPTED_HOSTS: [&str; 6] = [
        "instagram.com",
        "www.instagram.com",
        "m.instagram.com",
        "l.instagram.com",
        "instagr.am",
        "www.instagr.am",
    ];
    let host = authority.to_ascii_lowercase();
    if ACCEPTED_HOSTS.contains(&host.as_str()) {
        Ok(())
    } else {
        Err(PermalinkError::ForeignHost)
    }
}

/// The canonical form a path segment names, or `None` when the segment does
/// not name one. Case-sensitive on purpose.
#[must_use]
fn path_form(segment: &str) -> Option<PermalinkKind> {
    match segment {
        "p" => Some(PermalinkKind::Post),
        "reel" | "reels" => Some(PermalinkKind::Reel),
        "tv" => Some(PermalinkKind::Igtv),
        _ => None,
    }
}

/// Parses the path portion into a canonical kind and validated shortcode.
fn parse_path(path: &str) -> Result<ParsedPath, PermalinkError> {
    let mut segments: Vec<&str> = path.strip_prefix('/').unwrap_or(path).split('/').collect();
    if segments.len() > 1 && segments.last() == Some(&"") {
        segments.pop(); // one trailing-slash artifact
    }

    let (form_segment, shortcode) = match segments.as_slice() {
        // A lone form name with no shortcode after it is a shortcode failure,
        // not a path failure; anything else unshaped is not a permalink.
        [only] => match path_form(only) {
            Some(_) => return Err(PermalinkError::InvalidShortcode),
            None => return Err(PermalinkError::NotAPermalink),
        },
        [form, code] | [_, form, code] => (*form, *code),
        _ => return Err(PermalinkError::NotAPermalink),
    };

    let kind = path_form(form_segment).ok_or(PermalinkError::NotAPermalink)?;
    validate_shortcode(shortcode)?;
    Ok(ParsedPath {
        kind,
        shortcode: shortcode.to_owned(),
    })
}

/// Validates the shortcode alphabet: `[A-Za-z0-9_-]{1..=64}`, case preserved.
fn validate_shortcode(shortcode: &str) -> Result<(), PermalinkError> {
    let well_formed = !shortcode.is_empty()
        && shortcode.len() <= SHORTCODE_MAX
        && shortcode
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
    if well_formed {
        Ok(())
    } else {
        Err(PermalinkError::InvalidShortcode)
    }
}

/// Whether any character of the input can never appear in a shareable URL:
/// ASCII whitespace and control characters.
fn invalid_character(input: &str) -> bool {
    input
        .chars()
        .any(|character| character.is_ascii_whitespace() || character.is_control())
}
