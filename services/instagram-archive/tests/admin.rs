//! Operator plane contract: liveness, readiness, metrics, version, caching.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;

use ratatoskr_instagram_archive_service::RuntimeState;

#[expect(
    clippy::expect_used,
    reason = "router-test helper: an unanswered request or unreadable body is the failure"
)]
async fn get_status(
    state: std::sync::Arc<RuntimeState>,
    path: &str,
) -> (StatusCode, String, Option<String>) {
    let router = ratatoskr_instagram_archive_service::admin_router(state, || {
        "instagram_stub_metrics 1".to_owned()
    });
    let response = router
        .oneshot(
            Request::builder()
                .uri(path)
                .body(Body::empty())
                .expect("a valid request"),
        )
        .await
        .expect("the router answers");
    let status = response.status();
    let cache_control = response
        .headers()
        .get("cache-control")
        .map(|value| value.to_str().expect("ASCII header").to_owned());
    let body = response
        .into_body()
        .collect()
        .await
        .expect("a collectible body")
        .to_bytes();
    (
        status,
        String::from_utf8(body.to_vec()).expect("UTF-8 bodies"),
        cache_control,
    )
}

#[tokio::test]
async fn live_answers_ok_in_every_state() {
    let state = std::sync::Arc::new(RuntimeState::new());
    state.mark_startup_complete();
    state.begin_draining();

    for _ in 0..3 {
        let (status, body, _) = get_status(std::sync::Arc::clone(&state), "/health/live").await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            body.contains("live"),
            "the body must state liveness: {body}"
        );
    }
}

#[tokio::test]
async fn ready_transitions_with_startup_and_drain() {
    let state = std::sync::Arc::new(RuntimeState::new());

    let (status, body, _) = get_status(std::sync::Arc::clone(&state), "/health/ready").await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "not ready at start"
    );
    assert!(body.contains("not_ready"), "{body}");
    assert!(body.contains("startup_incomplete"), "{body}");

    state.mark_startup_complete();
    let (status, body, _) = get_status(std::sync::Arc::clone(&state), "/health/ready").await;
    assert_eq!(status, StatusCode::OK, "ready after startup completes");
    assert!(body.contains("\"ready\""), "{body}");

    state.begin_draining();
    let (status, body, _) = get_status(state, "/health/ready").await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "draining is not ready"
    );
    assert!(body.contains("shutdown_requested"), "{body}");
}

#[tokio::test]
async fn database_check_is_visible_without_flipping_readiness() {
    let state = std::sync::Arc::new(RuntimeState::new());
    state.mark_startup_complete();
    state.set_database_reachable(false);

    let (status, body, _) = get_status(std::sync::Arc::clone(&state), "/health/ready").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a down dependency must not flap readiness"
    );
    assert!(body.contains("\"database\""), "{body}");
    assert!(body.contains("\"fail\""), "{body}");
    assert!(body.contains("dependency_unavailable"), "{body}");

    state.set_database_reachable(true);
    let (_, body, _) = get_status(state, "/health/ready").await;
    assert!(body.contains("\"pass\""), "recovered probe passes: {body}");
}

#[tokio::test]
async fn absent_database_is_not_reported_as_a_passing_check() {
    let state = std::sync::Arc::new(RuntimeState::new());
    state.mark_startup_complete();

    let (_, body, _) = get_status(state, "/health/ready").await;
    assert!(
        !body.contains("\"database\""),
        "an unconfigured database reports no check: {body}"
    );
}

#[tokio::test]
async fn metrics_returns_the_rendered_prometheus_text() {
    let (status, body, _) = get_status(std::sync::Arc::new(RuntimeState::new()), "/metrics").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("instagram_stub_metrics"),
        "the rendered exposition text must be served verbatim: {body}"
    );
}

#[tokio::test]
async fn version_carries_the_build_identity() {
    let (status, body, _) = get_status(std::sync::Arc::new(RuntimeState::new()), "/version").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("ratatoskr-instagram-archive"), "{body}");
    assert!(body.contains(env!("CARGO_PKG_VERSION")), "{body}");
    assert!(body.contains("git_sha"), "{body}");
    assert!(body.contains("rust_version"), "{body}");
}

#[tokio::test]
async fn every_response_forbids_caching_and_unknown_paths_are_bare_404s() {
    for path in [
        "/health/live",
        "/health/ready",
        "/metrics",
        "/version",
        "/nope",
    ] {
        let (status, _, cache_control) =
            get_status(std::sync::Arc::new(RuntimeState::new()), path).await;
        if path == "/nope" {
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
            assert!(cache_control.is_some(), "even the 404 forbids caching");
        } else {
            assert_eq!(
                cache_control.as_deref(),
                Some("no-store"),
                "{path} must forbid caching"
            );
        }
    }
}
