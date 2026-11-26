//! The web platform and the viewer served from disk: content types, cache headers, the client-route
//! fallback, and that no request path reaches a file outside the served directories.

mod common;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use common::Harness;
use http_body_util::BodyExt;
use ibc_coordinator::util::random_hex;
use std::path::{Path, PathBuf};
use tower::ServiceExt;

struct Site {
    h: Harness,
    root: PathBuf,
}

impl Drop for Site {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn write(path: &Path, body: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// `<root>/web`, `<root>/viewer`, and a secret beside them that must never be served.
fn site() -> Site {
    let root = std::env::temp_dir().join(format!("ibc-static-test-{}", random_hex(8)));
    let (web, viewer) = (root.join("web"), root.join("viewer"));
    write(&web.join("index.html"), b"<!doctype html>web");
    write(&web.join("assets/index-AbC123.js"), b"console.log(1)");
    write(&web.join("favicon.svg"), b"<svg/>");
    write(&viewer.join("index.html"), b"<!doctype html>viewer");
    write(&viewer.join("engine/0.1.0.wasm"), b"\0asm\x01\0\0\0");
    write(&root.join("secret.txt"), b"secret");
    let h = Harness::new(|cfg| {
        cfg.web_dir = Some(web.clone());
        cfg.viewer_dir = Some(viewer.clone());
    });
    Site { h, root }
}

async fn get(site: &Site, method: &str, path: &str) -> (StatusCode, HeaderMap, Vec<u8>) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let res = ibc_coordinator::api::router(site.h.app.clone())
        .oneshot(req)
        .await
        .unwrap();
    let (status, headers) = (res.status(), res.headers().clone());
    let body = res.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, headers, body)
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}

#[tokio::test]
async fn files_have_their_content_types_and_cache_headers() {
    let s = site();
    let (status, h, body) = get(&s, "GET", "/").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"<!doctype html>web");
    assert_eq!(header(&h, "content-type"), "text/html; charset=utf-8");
    assert_eq!(header(&h, "cache-control"), "no-cache");

    let (status, h, _) = get(&s, "GET", "/assets/index-AbC123.js").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(header(&h, "content-type"), "text/javascript; charset=utf-8");
    assert!(header(&h, "cache-control").contains("immutable"));

    let (status, h, _) = get(&s, "GET", "/favicon.svg").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(header(&h, "content-type"), "image/svg+xml");

    let (status, h, body) = get(&s, "GET", "/viewer/engine/0.1.0.wasm").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(header(&h, "content-type"), "application/wasm");
    assert_eq!(header(&h, "x-content-type-options"), "nosniff");
    assert_eq!(&body[..4], b"\0asm");

    let (status, _, body) = get(&s, "GET", "/viewer/?replay=/v1/matches/x/replay").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"<!doctype html>viewer");

    let (status, h, _) = get(&s, "HEAD", "/").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(header(&h, "content-type"), "text/html; charset=utf-8");
}

#[tokio::test]
async fn pages_cannot_be_framed_by_other_sites() {
    let s = site();
    let (_, h, _) = get(&s, "GET", "/device?code=X").await;
    assert_eq!(header(&h, "x-frame-options"), "DENY");
    assert_eq!(
        header(&h, "content-security-policy"),
        "frame-ancestors 'none'"
    );
    let (_, h, _) = get(&s, "GET", "/viewer/").await;
    assert_eq!(header(&h, "x-frame-options"), "SAMEORIGIN");
}

#[tokio::test]
async fn unchanged_files_revalidate_with_304() {
    let s = site();
    let (_, h, _) = get(&s, "GET", "/viewer/engine/0.1.0.wasm").await;
    let etag = header(&h, "etag").to_string();
    assert!(etag.starts_with('"'), "{etag}");
    let req = Request::builder()
        .uri("/viewer/engine/0.1.0.wasm")
        .header("if-none-match", &etag)
        .body(Body::empty())
        .unwrap();
    let res = ibc_coordinator::api::router(s.h.app.clone())
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    assert!(body.is_empty());
}

#[tokio::test]
async fn client_routes_fall_back_to_the_page_but_api_and_files_do_not() {
    let s = site();
    for path in [
        "/device?code=ABCD-1234",
        "/teams/t1/",
        "/teams/my.team",
        "/admin",
    ] {
        let (status, _, body) = get(&s, "GET", path).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(body, b"<!doctype html>web", "{path}");
    }
    // A missing asset is a 404, not the page (a script tag would otherwise run HTML).
    let (status, _, _) = get(&s, "GET", "/assets/missing.js").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // The viewer has no client routes.
    let (status, _, _) = get(&s, "GET", "/viewer/replays/index.json").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // An unknown endpoint is the API's JSON 404, so clients can tell it from a page.
    let (status, h, body) = get(&s, "GET", "/v1/no-such-endpoint").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(header(&h, "content-type"), "application/json");
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["error"], "no_route");
    // Existing API routes are untouched.
    let (status, _, body) = get(&s, "GET", "/healthz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"ok\n");
    // Writes never reach the files.
    let (status, _, _) = get(&s, "POST", "/index.html").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn viewer_without_slash_redirects_keeping_the_query() {
    let s = site();
    let (status, h, _) = get(&s, "GET", "/viewer?replay=/v1/matches/m1/replay").await;
    assert_eq!(status, StatusCode::MOVED_PERMANENTLY);
    assert_eq!(
        header(&h, "location"),
        "/viewer/?replay=/v1/matches/m1/replay"
    );
}

#[tokio::test]
async fn no_path_escapes_the_served_directories() {
    let s = site();
    for path in [
        "/../secret.txt",
        "/%2e%2e/secret.txt",
        "/%2E%2E%2Fsecret.txt",
        "/assets/..%2f..%2fsecret.txt",
        "/viewer/../secret.txt",
        "/viewer/%2e%2e/secret.txt",
        "/viewer/..%5csecret.txt",
        "/viewer/engine/..%2f..%2f..%2fsecret.txt",
        "/./secret.txt",
    ] {
        let (status, _, body) = get(&s, "GET", path).await;
        assert_ne!(body, b"secret", "{path} served the secret");
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn without_page_directories_only_the_api_answers() {
    let h = Harness::new(|_| {});
    let req = Request::builder().uri("/").body(Body::empty()).unwrap();
    let res = ibc_coordinator::api::router(h.app.clone())
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}
