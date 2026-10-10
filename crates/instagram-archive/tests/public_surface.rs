//! Contract coverage for the Meta oEmbed public surface (XR-021 CONTRACTS.md S10 CD5).

#![allow(
    clippy::expect_used,
    reason = "the mock-server assertions are the integration contract"
)]

use ratatoskr_instagram_archive::permalink::{CanonicalPermalink, canonicalize};
use ratatoskr_instagram_archive::public_surface::{
    HttpPublicSurface, MAX_BODY_BYTES, classify_response,
};
use ratatoskr_instagram_archive::{PublicSurface, SurfaceOutcome};
use secrecy::SecretString;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REEL_FIXTURE: &str = include_str!("fixtures/oembed/reel_public.json");
const TOKEN: &str = "app-token-that-must-never-leak";

fn permalink() -> CanonicalPermalink {
    canonicalize("https://www.instagram.com/reel/Cabc123/").expect("a supported permalink")
}

fn surface(server: &MockServer) -> HttpPublicSurface {
    HttpPublicSurface::new(
        &format!("{}/v25.0/instagram_oembed", server.uri()),
        SecretString::from(TOKEN),
    )
    .expect("the surface builds")
}

#[test]
fn the_classifier_maps_each_documented_status() {
    assert_eq!(
        classify_response(200, REEL_FIXTURE),
        SurfaceOutcome::Payload {
            body: REEL_FIXTURE.to_owned()
        },
        "a 200 JSON object is the payload, byte for byte"
    );
    assert_eq!(
        classify_response(200, "[1,2,3]"),
        SurfaceOutcome::Unavailable,
        "a 200 that is not an object proves nothing about the post"
    );
    assert_eq!(
        classify_response(200, "not json"),
        SurfaceOutcome::Unavailable
    );
    assert_eq!(classify_response(404, ""), SurfaceOutcome::Deleted);
    assert_eq!(classify_response(403, ""), SurfaceOutcome::Private);
    assert_eq!(classify_response(400, ""), SurfaceOutcome::Unsupported);
    for transient in [401, 429, 500, 502, 503, 599] {
        assert_eq!(
            classify_response(transient, ""),
            SurfaceOutcome::TemporarilyUnavailable,
            "{transient} may succeed later"
        );
    }
    for status in [301, 302, 410, 418] {
        assert_ne!(
            classify_response(status, ""),
            SurfaceOutcome::Deleted,
            "{status} is not a provider statement of deletion"
        );
    }
}

#[test]
fn a_server_error_or_timeout_is_never_a_deletion() {
    for status in 500..=599 {
        assert_ne!(classify_response(status, ""), SurfaceOutcome::Deleted);
    }
}

#[tokio::test]
async fn fetch_sends_the_permalink_and_token_as_query_parameters() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v25.0/instagram_oembed"))
        .and(query_param(
            "url",
            "https://www.instagram.com/reel/Cabc123/",
        ))
        .and(query_param("access_token", TOKEN))
        .respond_with(ResponseTemplate::new(200).set_body_string(REEL_FIXTURE))
        .expect(1)
        .mount(&server)
        .await;

    let outcome = surface(&server).fetch(&permalink()).await;

    assert_eq!(
        outcome,
        SurfaceOutcome::Payload {
            body: REEL_FIXTURE.to_owned()
        }
    );
}

#[tokio::test]
async fn fetch_does_not_follow_redirects() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v25.0/instagram_oembed"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", format!("{}/elsewhere", server.uri())),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/elsewhere"))
        .respond_with(ResponseTemplate::new(200).set_body_string(REEL_FIXTURE))
        .expect(0)
        .mount(&server)
        .await;

    let outcome = surface(&server).fetch(&permalink()).await;

    assert!(
        !matches!(outcome, SurfaceOutcome::Payload { .. }),
        "a redirect target is never trusted: {outcome:?}"
    );
}

#[tokio::test]
async fn fetch_rejects_a_body_over_the_cap() {
    let server = MockServer::start().await;
    let oversized = format!("{{\"title\":\"{}\"}}", "x".repeat(MAX_BODY_BYTES + 1));
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(oversized))
        .mount(&server)
        .await;

    assert_eq!(
        surface(&server).fetch(&permalink()).await,
        SurfaceOutcome::TransportFailure
    );
}

#[tokio::test]
async fn fetch_reports_a_refused_connection_as_a_transport_failure() {
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
        listener.local_addr().expect("its address")
    };
    let surface = HttpPublicSurface::new(
        &format!("http://{closed}/v25.0/instagram_oembed"),
        SecretString::from(TOKEN),
    )
    .expect("the surface builds");

    assert_eq!(
        surface.fetch(&permalink()).await,
        SurfaceOutcome::TransportFailure
    );
}

#[tokio::test]
async fn fetch_maps_provider_statuses_through_the_classifier() {
    for (status, expected) in [
        (404, SurfaceOutcome::Deleted),
        (403, SurfaceOutcome::Private),
        (400, SurfaceOutcome::Unsupported),
        (429, SurfaceOutcome::TemporarilyUnavailable),
        (503, SurfaceOutcome::TemporarilyUnavailable),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(status))
            .mount(&server)
            .await;
        assert_eq!(
            surface(&server).fetch(&permalink()).await,
            expected,
            "{status}"
        );
    }
}

#[test]
fn the_token_never_appears_in_debug_output_or_errors() {
    let surface = HttpPublicSurface::new(
        "https://graph.facebook.com/v25.0/instagram_oembed",
        SecretString::from(TOKEN),
    )
    .expect("the surface builds");
    let rendered = format!("{surface:?}");
    assert!(!rendered.contains(TOKEN), "{rendered}");

    let error = HttpPublicSurface::new("not a url", SecretString::from(TOKEN))
        .expect_err("an unparseable endpoint is refused");
    let rendered = format!("{error} {error:?}");
    assert!(!rendered.contains(TOKEN), "{rendered}");
    assert!(!rendered.contains("not a url"), "{rendered}");
}
