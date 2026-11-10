use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use kou_server::{Server, router};
use serde_json::Value;
use std::{fs, path::PathBuf};
use tower::ServiceExt;

fn setup() -> (tempfile::TempDir, Router) {
    let directory = tempfile::tempdir().unwrap();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../harness/sessionstore/localfile/testdata/golden-session.session.jsonl");
    fs::copy(
        fixture,
        directory.path().join("golden-session.session.jsonl"),
    )
    .unwrap();
    let app = router(Server::new(directory.path().into(), None).unwrap());
    (directory, app)
}

async fn get(app: &Router, path: &str) -> (StatusCode, Vec<u8>) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header("host", "localhost")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, bytes)
}

#[tokio::test]
async fn serves_read_only_view_pages_and_historical_item() {
    let (_dir, app) = setup();
    let (status, bytes) = get(&app, "/api/v2/sessions").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["sessions"][0]["id"],
        "golden-session"
    );
    let (status, bytes) = get(&app, "/api/v2/sessions/golden-session/view").await;
    assert_eq!(status, StatusCode::OK);
    let description: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(description["items"], 4);
    assert_eq!(description["read_only"], true);
    let generation = description["generation"].as_str().unwrap();
    let (status, bytes) = get(
        &app,
        &format!("/api/v2/sessions/golden-session/headers?generation={generation}&limit=2"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let page: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    assert_eq!(page["items"][0]["preview"], "hello");
    assert_eq!(page["more"], true);
    let (status, bytes) = get(
        &app,
        &format!("/api/v2/sessions/golden-session/items/4?generation={generation}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["Data"]["Operations"][0]["Status"],
        "ready"
    );
    let (status, _) = get(
        &app,
        &format!("/api/v2/sessions/golden-session/items/0?generation={generation}"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn rejects_stale_generation_and_refreshes_a_changed_cached_log() {
    let (dir, app) = setup();
    let (_, bytes) = get(&app, "/api/v2/sessions/golden-session/view").await;
    let old: Value = serde_json::from_slice(&bytes).unwrap();
    let generation = old["generation"].as_str().unwrap();
    let path = dir.path().join("golden-session.session.jsonl");
    let original = fs::read_to_string(&path).unwrap();
    fs::write(&path, original.replace(r#""step":2"#, r#""step":3"#)).unwrap();
    let (status, _) = get(
        &app,
        &format!("/api/v2/sessions/golden-session/headers?generation={generation}"),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (_, bytes) = get(&app, "/api/v2/sessions/golden-session/view").await;
    let new: Value = serde_json::from_slice(&bytes).unwrap();
    assert_ne!(new["generation"], old["generation"]);
}

#[tokio::test]
async fn export_is_verbatim_and_does_not_modify_the_source() {
    let (dir, app) = setup();
    let path = dir.path().join("golden-session.session.jsonl");
    let original = fs::read(&path).unwrap();
    let (_, bytes) = get(&app, "/api/v2/sessions/golden-session/view").await;
    let view: Value = serde_json::from_slice(&bytes).unwrap();
    let generation = view["generation"].as_str().unwrap();
    let (status, bytes) = get(
        &app,
        &format!("/api/v2/sessions/golden-session/export?generation={generation}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, original);
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[tokio::test]
async fn rejects_cross_origin_rebinding_and_missing_host() {
    let (_dir, app) = setup();
    for (host, origin, site) in [
        (None, None, None),
        (Some("evil.example"), None, None),
        (Some("localhost"), Some("https://evil.example"), None),
        (Some("localhost"), Some("http://localhost:9999"), None),
        (Some("127.0.0.1"), None, Some("cross-site")),
    ] {
        let mut request = Request::builder().uri("/api/health");
        if let Some(host) = host {
            request = request.header("host", host);
        }
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        if let Some(site) = site {
            request = request.header("sec-fetch-site", site);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    for host in ["localhost", "127.0.0.1:8091", "[::1]:8091"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .header("host", host)
                    .header("origin", format!("http://{host}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().contains_key("content-security-policy"));
    }
}

#[tokio::test]
async fn has_no_write_routes_and_rejects_unbounded_pages() {
    let (_dir, app) = setup();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v2/sessions")
                .header("host", "localhost")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    let (status, _) = get(&app, "/api/v2/sessions?limit=999999").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = get(&app, "/api/v2/sessions/golden-session/headers").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn does_not_follow_session_symlinks_or_invalid_ids() {
    let (dir, app) = setup();
    let target = dir.path().join("golden-session.session.jsonl");
    std::os::unix::fs::symlink(target, dir.path().join("linked.session.jsonl")).unwrap();
    let (status, _) = get(&app, "/api/v2/sessions/linked/view").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = get(&app, "/api/v2/sessions/bad_id/view").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
