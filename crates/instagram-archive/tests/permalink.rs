//! Permalink canonicalization contract: the acceptance table across every
//! URL form clients deliver, determinism, shortcode case, and refusals by
//! typed class.

use ratatoskr_instagram_archive::permalink::{PermalinkError, PermalinkKind, canonicalize};

/// One accepted form: the input a client delivers, the exact canonical output,
/// and the canonical path form it must carry.
struct Accepted {
    input: &'static str,
    url: &'static str,
    kind: PermalinkKind,
}

/// The acceptance table across every delivered URL form.
const ACCEPTED: [Accepted; 17] = [
    // plain post forms
    Accepted {
        input: "https://www.instagram.com/p/DHcxI7hpS5t/",
        url: "https://www.instagram.com/p/DHcxI7hpS5t/",
        kind: PermalinkKind::Post,
    },
    Accepted {
        input: "https://instagram.com/p/DHcxI7hpS5t/",
        url: "https://www.instagram.com/p/DHcxI7hpS5t/",
        kind: PermalinkKind::Post,
    },
    Accepted {
        input: "http://www.instagram.com/p/DHcxI7hpS5t/",
        url: "https://www.instagram.com/p/DHcxI7hpS5t/",
        kind: PermalinkKind::Post,
    },
    Accepted {
        input: "http://instagram.com/p/DHcxI7hpS5t",
        url: "https://www.instagram.com/p/DHcxI7hpS5t/",
        kind: PermalinkKind::Post,
    },
    Accepted {
        input: "https://www.instagram.com/p/C123_-abc?igsh=MTk0&utm_source=share#frag",
        url: "https://www.instagram.com/p/C123_-abc/",
        kind: PermalinkKind::Post,
    },
    // mobile and link-shim hosts
    Accepted {
        input: "https://m.instagram.com/p/DHcxI7hpS5t/",
        url: "https://www.instagram.com/p/DHcxI7hpS5t/",
        kind: PermalinkKind::Post,
    },
    Accepted {
        input: "https://l.instagram.com/p/DHcxI7hpS5t/?u=https%3A%2F%2Fexample.com",
        url: "https://www.instagram.com/p/DHcxI7hpS5t/",
        kind: PermalinkKind::Post,
    },
    // the instagr.am domains
    Accepted {
        input: "https://instagr.am/p/DHcxI7hpS5t/",
        url: "https://www.instagram.com/p/DHcxI7hpS5t/",
        kind: PermalinkKind::Post,
    },
    Accepted {
        input: "http://www.instagr.am/p/DHcxI7hpS5t/",
        url: "https://www.instagram.com/p/DHcxI7hpS5t/",
        kind: PermalinkKind::Post,
    },
    // reel forms, including the /reels/ alias
    Accepted {
        input: "https://www.instagram.com/reel/DHab_c9-x/",
        url: "https://www.instagram.com/reel/DHab_c9-x/",
        kind: PermalinkKind::Reel,
    },
    Accepted {
        input: "https://www.instagram.com/reels/DHab_c9-x/",
        url: "https://www.instagram.com/reel/DHab_c9-x/",
        kind: PermalinkKind::Reel,
    },
    Accepted {
        input: "https://instagram.com/reels/DHab_c9-x?utm_source=ig_web_copy_link",
        url: "https://www.instagram.com/reel/DHab_c9-x/",
        kind: PermalinkKind::Reel,
    },
    // legacy IGTV form, preserved as delivered
    Accepted {
        input: "https://www.instagram.com/tv/CH_TV-01/",
        url: "https://www.instagram.com/tv/CH_TV-01/",
        kind: PermalinkKind::Igtv,
    },
    // username-prefixed permalink paths drop the username segment
    Accepted {
        input: "https://www.instagram.com/someuser/p/DHcxI7hpS5t/",
        url: "https://www.instagram.com/p/DHcxI7hpS5t/",
        kind: PermalinkKind::Post,
    },
    Accepted {
        input: "https://instagram.com/other.user/reel/DHab_c9-x/",
        url: "https://www.instagram.com/reel/DHab_c9-x/",
        kind: PermalinkKind::Reel,
    },
    // uppercase scheme and host fold to lower case
    Accepted {
        input: "HTTPS://WWW.INSTAGRAM.COM/p/DHcxI7hpS5t/",
        url: "https://www.instagram.com/p/DHcxI7hpS5t/",
        kind: PermalinkKind::Post,
    },
    // surrounding share-sheet whitespace is trimmed
    Accepted {
        input: "  https://www.instagram.com/p/DHcxI7hpS5t/  ",
        url: "https://www.instagram.com/p/DHcxI7hpS5t/",
        kind: PermalinkKind::Post,
    },
];

#[test]
fn every_accepted_form_yields_its_exact_canonical_permalink() {
    for entry in &ACCEPTED {
        let canonical = canonicalize(entry.input).expect("every table entry must be accepted");
        assert_eq!(
            canonical.url, entry.url,
            "canonical form for {:?}",
            entry.input
        );
        assert_eq!(
            canonical.kind, entry.kind,
            "path form for {:?}",
            entry.input
        );
        assert!(
            canonical
                .url
                .ends_with(&format!("/{}/", canonical.shortcode)),
            "the canonical URL ends with the shortcode: {canonical:?}"
        );
    }
}

#[test]
fn canonicalization_is_deterministic() {
    let input = "https://instagram.com/reels/DHab_c9-x?utm_source=x";
    let first = canonicalize(input).expect("the input is accepted");
    let second = canonicalize(input).expect("the input is accepted");
    assert_eq!(first, second, "the same input must yield the same output");
}

#[test]
fn shortcode_case_survives_canonicalization() {
    let canonical = canonicalize("https://www.instagram.com/p/CdE1f2G3h-i_/").expect("valid");
    assert_eq!(
        canonical.shortcode, "CdE1f2G3h-i_",
        "case must be preserved"
    );
}

/// Refusal classes: each malformed shape names its reason.
#[test]
fn non_urls_are_refused_as_malformed() {
    for input in [
        "",
        "not a url",
        "www.instagram.com/p/DHcxI7hpS5t/",
        "ftp://www.instagram.com/p/DHcxI7hpS5t/",
        "https:///p/DHcxI7hpS5t/",
        "https://www.instagram.com/p/has space/",
    ] {
        assert_eq!(
            canonicalize_err(input),
            Some(PermalinkError::Malformed),
            "{input:?}"
        );
    }
}

#[test]
fn foreign_hosts_are_refused() {
    for input in [
        "https://example.com/p/DHcxI7hpS5t/",
        "https://instagram.invalid/p/DHcxI7hpS5t/",
        "https://fakeinstagram.com/p/DHcxI7hpS5t/",
        "https://instagram.com.evil.example/p/DHcxI7hpS5t/",
        "https://wwww.instagram.com/p/DHcxI7hpS5t/",
    ] {
        assert_eq!(
            canonicalize_err(input),
            Some(PermalinkError::ForeignHost),
            "{input:?}"
        );
    }
}

#[test]
fn non_permalink_paths_on_supported_hosts_are_refused() {
    for input in [
        "https://www.instagram.com/someuser/",
        "https://www.instagram.com/stories/someuser/46828441234567/",
        "https://www.instagram.com/explore/tags/rust/",
        "https://www.instagram.com/accounts/login/",
        "https://www.instagram.com/",
    ] {
        assert_eq!(
            canonicalize_err(input),
            Some(PermalinkError::NotAPermalink),
            "{input:?}"
        );
    }
}

#[test]
fn authorities_with_credentials_or_ports_are_refused() {
    for input in [
        "https://someone@www.instagram.com/p/DHcxI7hpS5t/",
        "https://www.instagram.com:8443/p/DHcxI7hpS5t/",
        "https://user@instagram.com/p/DHcxI7hpS5t/",
    ] {
        assert_eq!(
            canonicalize_err(input),
            Some(PermalinkError::UnsupportedAuthority),
            "{input:?}"
        );
    }
}

#[test]
fn missing_and_invalid_shortcodes_are_refused() {
    let long = format!("https://www.instagram.com/p/{}/", "a".repeat(65));
    for (input, why) in [
        ("https://www.instagram.com/p//", "empty shortcode"),
        (long.as_str(), "over-long shortcode"),
        (
            "https://www.instagram.com/p/ab%20cd/",
            "percent-encoded shortcode",
        ),
        (
            "https://www.instagram.com/p/abc$def/",
            "character outside the alphabet",
        ),
    ] {
        assert_eq!(
            canonicalize_err(input),
            Some(PermalinkError::InvalidShortcode),
            "{why}: {input:?}"
        );
    }
}

fn canonicalize_err(input: &str) -> Option<PermalinkError> {
    canonicalize(input).err()
}
