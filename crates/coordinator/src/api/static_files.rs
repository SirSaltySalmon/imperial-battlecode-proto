//! The human-facing pages: the web platform (`web/dist`) at `/` and the replay viewer (`viewer/dist`) at
//! `/viewer/`, both plain files from disk. This is the router's fallback, so it only sees paths no API route
//! took. Unknown `/v1/...` paths answer the API's JSON 404 (`no_route`), never the web page, so clients can
//! tell an endpoint that does not exist from a page.
//!
//! The web platform routes on the client: a missing path that does not look like an asset (`/device`,
//! `/teams/abc`) gets `index.html`. The viewer has no such fallback; a missing viewer file is a 404.
//! Files carry an `ETag` (size and modification time), so a `no-cache` file is revalidated with a 304.

use super::AppState;
use crate::error::ApiError;
use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Vite names everything under `assets/` by content hash, so those files never change under their name.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";
/// Everything else (`index.html`, engine builds named by version) is revalidated on every load.
const REVALIDATE: &str = "no-cache";
const OCTET_STREAM: &str = "application/octet-stream";

/// Who may frame a page. The platform: nobody (the device-approval page must not be clickjacked). The
/// viewer: pages of the same origin.
#[derive(Clone, Copy, PartialEq)]
enum Framing {
    Deny,
    SameOrigin,
}

pub async fn serve(State(app): AppState, method: Method, uri: Uri, headers: HeaderMap) -> Response {
    let path = uri.path();
    if path == "/v1" || path.starts_with("/v1/") {
        return ApiError::new(
            StatusCode::NOT_FOUND,
            "no_route",
            format!("no such endpoint: {method} {path}"),
        )
        .into_response();
    }
    if method != Method::GET && method != Method::HEAD {
        return not_found();
    }
    if path == "/viewer" && app.cfg.viewer_dir.is_some() {
        // The viewer's asset URLs are relative, so it must be loaded from a directory URL.
        let to = match uri.query() {
            Some(q) => format!("/viewer/?{q}"),
            None => "/viewer/".to_string(),
        };
        return redirect(&to);
    }
    let (root, rest, framing) = match path.strip_prefix("/viewer/") {
        Some(rest) => (app.cfg.viewer_dir.clone(), rest, Framing::SameOrigin),
        None => (app.cfg.web_dir.clone(), &path[1..], Framing::Deny),
    };
    let (Some(root), Some(relative)) = (root, safe_relative(rest)) else {
        return not_found();
    };
    let file = if relative.as_os_str().is_empty() {
        PathBuf::from("index.html")
    } else {
        relative
    };
    let etag = headers.get(header::IF_NONE_MATCH);
    if let Some(found) = read_file(root.join(&file)).await {
        return file_response(&file, found, framing, etag);
    }
    if framing == Framing::Deny && is_client_route(&file) {
        if let Some(found) = read_file(root.join("index.html")).await {
            return file_response(Path::new("index.html"), found, framing, etag);
        }
    }
    not_found()
}

/// A missing web path is a client route unless it is under `assets/` or ends in a known file type.
fn is_client_route(file: &Path) -> bool {
    !in_assets(file) && (file.extension().is_none() || content_type(file) == OCTET_STREAM)
}

fn in_assets(file: &Path) -> bool {
    file.components()
        .next()
        .is_some_and(|c| c.as_os_str() == "assets")
}

/// A URL path below a served root as a relative file path, or `None` if it could leave the root: every
/// segment is percent-decoded, and `.`, `..`, separators, drive prefixes and NUL are refused, before and
/// after decoding. One trailing `/` is ignored; other empty segments (`a//b`) are refused.
pub fn safe_relative(path: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    let path = path.strip_suffix('/').unwrap_or(path);
    if path.is_empty() {
        return Some(out);
    }
    for raw in path.split('/') {
        let segment = percent_decode(raw)?;
        let bad = segment.is_empty()
            || segment == "."
            || segment == ".."
            || segment
                .chars()
                .any(|c| matches!(c, '/' | '\\' | ':' | '\0') || c.is_control());
        if bad {
            return None;
        }
        out.push(segment);
    }
    Some(out)
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let hex = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

struct Found {
    bytes: Vec<u8>,
    etag: String,
}

async fn read_file(path: PathBuf) -> Option<Found> {
    tokio::task::spawn_blocking(move || {
        let meta = std::fs::metadata(&path).ok().filter(|m| m.is_file())?;
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        let bytes = std::fs::read(&path).ok()?;
        let etag = format!("\"{:x}-{:x}\"", bytes.len(), modified);
        Some(Found { bytes, etag })
    })
    .await
    .ok()
    .flatten()
}

pub fn content_type(file: &Path) -> &'static str {
    let ext = file
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("wasm") => "application/wasm",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("txt" | "md" | "ibcmap") => "text/plain; charset=utf-8",
        _ => OCTET_STREAM,
    }
}

fn file_response(
    file: &Path,
    found: Found,
    framing: Framing,
    if_none_match: Option<&HeaderValue>,
) -> Response {
    let fresh = if_none_match.is_some_and(|v| v.as_bytes() == found.etag.as_bytes());
    let mut res = if fresh {
        let mut res = Response::new(Body::empty());
        *res.status_mut() = StatusCode::NOT_MODIFIED;
        res
    } else {
        Response::new(Body::from(found.bytes))
    };
    let cache = if in_assets(file) {
        IMMUTABLE
    } else {
        REVALIDATE
    };
    let (frame_options, csp) = match framing {
        Framing::Deny => ("DENY", "frame-ancestors 'none'"),
        Framing::SameOrigin => ("SAMEORIGIN", "frame-ancestors 'self'"),
    };
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type(file)),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    if let Ok(etag) = HeaderValue::from_str(&found.etag) {
        h.insert(header::ETAG, etag);
    }
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::X_FRAME_OPTIONS,
        HeaderValue::from_static(frame_options),
    );
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(csp),
    );
    res
}

fn redirect(to: &str) -> Response {
    match HeaderValue::from_str(to) {
        Ok(location) => (
            StatusCode::MOVED_PERMANENTLY,
            [(header::LOCATION, location)],
        )
            .into_response(),
        Err(_) => not_found(),
    }
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "not found\n",
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_paths_that_leave_the_root() {
        for bad in [
            "..",
            "../x",
            "a/../../x",
            "%2e%2e/x",
            "%2E%2E%2Fx",
            "a%2f..%2f..%2fx",
            "a%5c..%5cx",
            "C:%5cwindows",
            "c:/x",
            "./x",
            "a//b",
            "a//",
            "%00",
            "%zz",
            "%e2%82",
        ] {
            assert_eq!(safe_relative(bad), None, "{bad}");
        }
    }

    #[test]
    fn accepts_plain_paths() {
        assert_eq!(safe_relative(""), Some(PathBuf::new()));
        assert_eq!(safe_relative("teams/"), Some(PathBuf::from("teams")));
        assert_eq!(
            safe_relative("assets/index-AbC1.js"),
            Some(PathBuf::from("assets").join("index-AbC1.js"))
        );
        assert_eq!(
            safe_relative("engine/0.1.0%2Bdev.wasm"),
            Some(PathBuf::from("engine").join("0.1.0+dev.wasm"))
        );
    }

    #[test]
    fn content_types() {
        assert_eq!(content_type(Path::new("x.wasm")), "application/wasm");
        assert_eq!(
            content_type(Path::new("a/B.JS")),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(content_type(Path::new("noext")), OCTET_STREAM);
    }

    #[test]
    fn client_routes() {
        assert!(is_client_route(Path::new("device")));
        assert!(is_client_route(Path::new("teams/my.team")));
        assert!(!is_client_route(Path::new("favicon.ico")));
        assert!(!is_client_route(Path::new("assets/missing")));
    }
}
