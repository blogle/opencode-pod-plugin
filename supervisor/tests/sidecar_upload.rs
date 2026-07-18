use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::routing::post;
use axum::Router;
use opencode_supervisor::checkpoint::{CheckpointArtifact, CheckpointMetadata};
use opencode_supervisor::sidecar::upload;
use std::fs;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
struct Received(Arc<Mutex<Option<(HeaderMap, Bytes)>>>);

#[tokio::test]
async fn uploads_raw_bundle_with_metadata_header() {
    let temporary = tempfile::tempdir().unwrap();
    let bundle_path = temporary.path().join("checkpoint.bundle");
    let metadata_path = temporary.path().join("checkpoint.json");
    let bundle = b"raw git bundle bytes\0\xff";
    fs::write(&bundle_path, bundle).unwrap();
    let metadata = CheckpointMetadata {
        workspace_id: "wrk_upload".to_owned(),
        created_at: "2026-07-17T00:00:00.000Z".to_owned(),
        head: "1111111111111111111111111111111111111111".to_owned(),
        branch: Some("main".to_owned()),
        status_sha256: "status".to_owned(),
        state_sha256: "state".to_owned(),
        bundle_sha256: "bundle".to_owned(),
        checkpoint_oid: "2222222222222222222222222222222222222222".to_owned(),
        bundle_ref: "refs/opencode/checkpoints/test".to_owned(),
        head_ref: "refs/opencode/heads/test".to_owned(),
        has_changes: true,
        format_version: 1,
    };
    fs::write(&metadata_path, serde_json::to_vec(&metadata).unwrap()).unwrap();
    let artifact = CheckpointArtifact {
        metadata: metadata.clone(),
        metadata_path,
        bundle_path,
    };
    let received = Received(Arc::new(Mutex::new(None)));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route(
            "/v1/workspaces/wrk_upload/checkpoints",
            post(receive_upload),
        )
        .with_state(received.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    upload(&artifact, &format!("http://{address}"), "gateway-secret")
        .await
        .unwrap();
    let (headers, body) = received.0.lock().await.take().unwrap();
    assert_eq!(body.as_ref(), bundle);
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "application/octet-stream"
    );
    assert_eq!(
        headers.get(header::AUTHORIZATION).unwrap(),
        "Bearer gateway-secret"
    );
    let header_metadata: CheckpointMetadata = serde_json::from_slice(
        headers
            .get("x-opencode-checkpoint-metadata")
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    assert_eq!(header_metadata.head, metadata.head);
    assert_eq!(header_metadata.head_ref, metadata.head_ref);
    server.abort();
}

async fn receive_upload(
    State(received): State<Received>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    *received.0.lock().await = Some((headers, body));
    StatusCode::CREATED
}
