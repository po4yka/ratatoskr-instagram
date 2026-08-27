//! Authenticated streaming Data Export receipt and owner-scoped status API.

use std::convert::Infallible;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Poll;

use axum::body::{Body, Bytes};
use axum::http::{Request, StatusCode};
use futures_util::stream;
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;
use uuid::Uuid;

use ratatoskr_instagram_archive::Config;
use ratatoskr_instagram_archive::data_export::{ArchiveLimits, DataExportStore};
use ratatoskr_instagram_archive::test_support::{TestDatabase, synthetic_saved_posts_export_zip};
use ratatoskr_instagram_archive_service::product::{
    DataExportRuntime, product_router_with_data_exports,
};

const OWNER: &str = "018f1a2b-3c4d-7e6f-8a9b-0c1d2e3f4a5b";
const TOKEN: &str = "synthetic-owner-token-abcdefghijklmnopqrstuvwxyz";
const OTHER_OWNER: &str = "018f1a2b-3c4d-7e6f-8a9b-0c1d2e3f4a6c";
const OTHER_TOKEN: &str = "synthetic-other-token-abcdefghijklmnopqrstuvwxyz";

fn runtime(root: &Path) -> DataExportRuntime {
    runtime_with(root, &format!("{OWNER}:{TOKEN}"), 1_048_576)
}

fn runtime_with(root: &Path, tokens: &str, max_body_bytes: u64) -> DataExportRuntime {
    DataExportRuntime::new(export_config(root, tokens, max_body_bytes).data_export)
}

#[expect(
    clippy::expect_used,
    reason = "a malformed synthetic test configuration must fail at its construction site"
)]
fn export_config(root: &Path, tokens: &str, max_body_bytes: u64) -> Config {
    let blob_root = root.join("blobs");
    let staging_root = root.join("staging");
    let entries = vec![
        (
            "RATATOSKR__BUS__URL".to_owned(),
            "nats://127.0.0.1:4222".to_owned(),
        ),
        (
            "RATATOSKR__DATA_EXPORT__ENABLED".to_owned(),
            "true".to_owned(),
        ),
        (
            "RATATOSKR__DATA_EXPORT__BLOB_ROOT".to_owned(),
            blob_root.display().to_string(),
        ),
        (
            "RATATOSKR__DATA_EXPORT__STAGING_ROOT".to_owned(),
            staging_root.display().to_string(),
        ),
        (
            "RATATOSKR__DATA_EXPORT__BEARER_TOKENS".to_owned(),
            tokens.to_owned(),
        ),
        (
            "RATATOSKR__DATA_EXPORT__MAX_BODY_BYTES".to_owned(),
            max_body_bytes.to_string(),
        ),
        (
            "RATATOSKR__DATA_EXPORT__MAX_TOTAL_COMPRESSED_BYTES".to_owned(),
            max_body_bytes.to_string(),
        ),
        (
            "RATATOSKR__DATA_EXPORT__MAX_TOTAL_DECOMPRESSED_BYTES".to_owned(),
            "4194304".to_owned(),
        ),
        (
            "RATATOSKR__DATA_EXPORT__MAX_ENTRY_DECOMPRESSED_BYTES".to_owned(),
            "1048576".to_owned(),
        ),
    ];
    Config::from_environment(entries).expect("bounded synthetic Data Export config")
}

#[expect(
    clippy::expect_used,
    reason = "synthetic request construction and in-process routing are test invariants"
)]
async fn send_upload(router: axum::Router, token: &str, body: Body) -> axum::response::Response {
    router
        .oneshot(
            Request::post("/v1/data-exports")
                .header("content-type", "application/zip")
                .header("authorization", format!("Bearer {token}"))
                .body(body)
                .expect("request builds"),
        )
        .await
        .expect("router answers")
}

#[expect(
    clippy::expect_used,
    reason = "test-owned staging must remain readable for cleanup assertions"
)]
async fn staging_is_empty(root: &Path) -> bool {
    let staging = root.join("staging");
    let Ok(mut entries) = tokio::fs::read_dir(staging).await else {
        return true;
    };
    entries
        .next_entry()
        .await
        .expect("test staging directory reads")
        .is_none()
}

fn test_root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "ratatoskr-instagram-data-export-{}",
        Uuid::now_v7()
    ))
}

#[expect(
    clippy::expect_used,
    reason = "the product contract requires every tested response to be readable JSON"
)]
async fn response_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("response body reads")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("response is JSON")
}

#[tokio::test]
async fn unknown_export_credential_is_refused_without_polling_body() {
    let test = TestDatabase::create().await.expect("fresh database");
    let root = test_root();
    let router = product_router_with_data_exports(test.database.clone(), Some(runtime(&root)));
    let polled = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&polled);
    let body = Body::from_stream(stream::poll_fn(move |_| {
        observed.store(true, Ordering::SeqCst);
        Poll::Ready(Some(Ok::<Bytes, Infallible>(Bytes::from_static(
            b"not a zip",
        ))))
    }));
    let response = router
        .oneshot(
            Request::post("/v1/data-exports")
                .header("content-type", "application/zip")
                .header(
                    "authorization",
                    "Bearer unknown-owner-token-abcdefghijklmnopqrstuvwxyz",
                )
                .body(body)
                .expect("request builds"),
        )
        .await
        .expect("router answers");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response_json(response).await,
        serde_json::json!({"error": "invalid_data_export_credential"})
    );
    assert!(
        !polled.load(Ordering::SeqCst),
        "body was polled before auth"
    );
    assert!(!root.exists(), "refused auth must not create storage roots");

    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn authenticated_upload_streams_exact_blob_before_inspection() {
    let test = TestDatabase::create().await.expect("fresh database");
    let root = test_root();
    let archive = synthetic_saved_posts_export_zip().expect("synthetic ZIP builds");
    let router = product_router_with_data_exports(test.database.clone(), Some(runtime(&root)));
    let response = router
        .oneshot(
            Request::post("/v1/data-exports")
                .header("content-type", "application/zip")
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(Body::from(archive.clone()))
                .expect("request builds"),
        )
        .await
        .expect("router answers");

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let receipt = response_json(response).await;
    assert_eq!(receipt["state"], "received");
    assert_eq!(receipt["archive"]["owner_service"], "ratatoskr-instagram");
    assert_eq!(receipt["archive"]["media_type"], "application/zip");
    assert_eq!(receipt["archive"]["length_bytes"], archive.len());
    let digest = receipt["archive"]["digest"]["hex"]
        .as_str()
        .expect("receipt exposes a typed digest");
    let stored = tokio::fs::read(root.join("blobs").join("sha256").join(digest))
        .await
        .expect("immutable archive is readable by its digest");
    assert_eq!(stored, archive, "stored archive bytes are exact");

    let state: String =
        sqlx::query_scalar("select state from instagram_archive.import_runs where run_id = $1")
            .bind(
                Uuid::parse_str(receipt["run_id"].as_str().expect("run id is text"))
                    .expect("run id is UUID"),
            )
            .fetch_one(test.database.pool())
            .await
            .expect("receipt is durable");
    assert_eq!(state, "received");

    tokio::fs::remove_dir_all(&root)
        .await
        .expect("test-owned storage cleans up");
    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn receipt_replay_is_owner_scoped_and_overgrowth_leaves_no_object() {
    let test = TestDatabase::create().await.expect("fresh database");
    let root = test_root();
    let archive = synthetic_saved_posts_export_zip().expect("synthetic ZIP builds");
    let tokens = format!("{OWNER}:{TOKEN},{OTHER_OWNER}:{OTHER_TOKEN}");
    let router = product_router_with_data_exports(
        test.database.clone(),
        Some(runtime_with(&root, &tokens, 1_048_576)),
    );

    let first = send_upload(router.clone(), TOKEN, Body::from(archive.clone())).await;
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    assert_eq!(
        first
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("no-store"),
        "private receipt evidence must not be cached"
    );
    let first = response_json(first).await;
    #[cfg(unix)]
    {
        let digest = first["archive"]["digest"]["hex"]
            .as_str()
            .expect("digest is text");
        let metadata = tokio::fs::metadata(root.join("blobs").join("sha256").join(digest))
            .await
            .expect("blob metadata reads");
        assert_eq!(
            metadata.permissions().mode() & 0o777,
            0o600,
            "raw export object must be private"
        );
    }
    let replay = send_upload(router.clone(), TOKEN, Body::from(archive.clone())).await;
    assert_eq!(replay.status(), StatusCode::OK);
    let replay = response_json(replay).await;
    assert_eq!(
        replay["run_id"], first["run_id"],
        "same owner replays one run"
    );

    let other = send_upload(router, OTHER_TOKEN, Body::from(archive)).await;
    assert_eq!(other.status(), StatusCode::ACCEPTED);
    let other = response_json(other).await;
    assert_ne!(
        other["run_id"], first["run_id"],
        "the same bytes never merge owner receipts"
    );
    let receipts: i64 = sqlx::query_scalar("select count(*) from instagram_archive.import_runs")
        .fetch_one(test.database.pool())
        .await
        .expect("receipt count answers");
    assert_eq!(receipts, 2);

    let overgrowth_root = test_root();
    let overgrowth_router = product_router_with_data_exports(
        test.database.clone(),
        Some(runtime_with(
            &overgrowth_root,
            &format!("{OWNER}:{TOKEN}"),
            1_024,
        )),
    );
    let chunks = stream::iter([
        Ok::<Bytes, Infallible>(Bytes::from(vec![b'a'; 700])),
        Ok(Bytes::from(vec![b'b'; 325])),
    ]);
    let refused = send_upload(overgrowth_router, TOKEN, Body::from_stream(chunks)).await;
    assert_eq!(refused.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        response_json(refused).await,
        serde_json::json!({"error": "archive_too_large"})
    );
    assert!(
        staging_is_empty(&overgrowth_root).await,
        "over-limit stream left a staging object"
    );
    assert!(
        !overgrowth_root.join("blobs").exists(),
        "over-limit stream published an immutable object"
    );

    tokio::fs::remove_dir_all(&root)
        .await
        .expect("test-owned storage cleans up");
    if overgrowth_root.exists() {
        tokio::fs::remove_dir_all(&overgrowth_root)
            .await
            .expect("test-owned overgrowth storage cleans up");
    }
    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn concurrent_existing_blob_is_verified_before_receipt() {
    let test = TestDatabase::create().await.expect("fresh database");
    let root = test_root();
    let archive = synthetic_saved_posts_export_zip().expect("synthetic ZIP builds");
    let tokens = format!("{OWNER}:{TOKEN},{OTHER_OWNER}:{OTHER_TOKEN}");
    let router = product_router_with_data_exports(
        test.database.clone(),
        Some(runtime_with(&root, &tokens, 1_048_576)),
    );

    let first = send_upload(router.clone(), TOKEN, Body::from(archive.clone())).await;
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    let first = response_json(first).await;
    let digest = first["archive"]["digest"]["hex"]
        .as_str()
        .expect("digest is text");
    let blob_path = root.join("blobs").join("sha256").join(digest);
    tokio::fs::remove_file(&blob_path)
        .await
        .expect("test removes only its own synthetic object");
    tokio::fs::create_dir(&blob_path)
        .await
        .expect("test installs an invalid concurrent winner object");

    let refused = send_upload(router, OTHER_TOKEN, Body::from(archive)).await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json(refused).await,
        serde_json::json!({"error": "immutable_blob_conflict"})
    );
    let receipts: i64 = sqlx::query_scalar("select count(*) from instagram_archive.import_runs")
        .fetch_one(test.database.pool())
        .await
        .expect("receipt count answers");
    assert_eq!(
        receipts, 1,
        "corrupt winner must be refused before DB receipt"
    );
    assert!(staging_is_empty(&root).await, "refusal left staging bytes");

    tokio::fs::remove_dir_all(&root)
        .await
        .expect("test-owned storage cleans up");
    test.cleanup().await.expect("cleanup must drop");
}

#[tokio::test]
async fn other_owner_cannot_read_import_report() {
    let test = TestDatabase::create().await.expect("fresh database");
    let root = test_root();
    let tokens = format!("{OWNER}:{TOKEN},{OTHER_OWNER}:{OTHER_TOKEN}");
    let config = export_config(&root, &tokens, 1_048_576);
    let store =
        DataExportStore::new(&test.database, &config.data_export).expect("validated store builds");
    let archive = synthetic_saved_posts_export_zip().expect("synthetic ZIP builds");
    let receipt = store
        .receive(
            Uuid::parse_str(OWNER).expect("owner UUID"),
            stream::iter([Ok::<Vec<u8>, Infallible>(archive)]),
        )
        .await
        .expect("archive receives");
    let run_id = receipt.receipt().run_id;
    store
        .inspect(run_id, ArchiveLimits::default())
        .await
        .expect("archive inspects");
    store
        .parse(run_id, ArchiveLimits::default())
        .await
        .expect("archive parses");
    store.reconcile(run_id).await.expect("report reconciles");

    let router = product_router_with_data_exports(
        test.database.clone(),
        Some(runtime_with(&root, &tokens, 1_048_576)),
    );
    let response = router
        .oneshot(
            Request::get(format!("/v1/data-exports/{run_id}"))
                .header("authorization", format!("Bearer {OTHER_TOKEN}"))
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    assert_eq!(
        response_json(response).await,
        serde_json::json!({"error": "data_export_not_found"})
    );

    tokio::fs::remove_dir_all(&root)
        .await
        .expect("test-owned storage cleans up");
    test.cleanup().await.expect("cleanup must drop");
}
