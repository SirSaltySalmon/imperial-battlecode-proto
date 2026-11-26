//! The map editor's side of `ibc view --editor [--maps DIR]`: list and read the maps of one folder, lint a
//! map with `ibc-maptool`'s checks, and save a map into that folder.
//!
//! | Route | |
//! |---|---|
//! | `GET /api/editor` | `{maps: [name, ...]}`: the `.ibcmap` files directly in the folder; `{editor: false}` without `--editor`. The page shows Lint and Save only with a list |
//! | `GET /maps/<name>.ibcmap` | one of those files |
//! | `POST /api/lint?name=<name>` | body: map text. `{errors, warnings, issues: [{severity, code, message}]}` |
//! | `POST /api/save?name=<name>[&overwrite=1]` | body: map text. Writes `<folder>/<name>.ibcmap`. 409 if it exists and `overwrite` is not set, 422 if the engine rejects the map |
//!
//! Nothing outside the folder is ever read or written: a name is a plain file stem (`[A-Za-z0-9_-]`, not a
//! Windows device name) and a file that resolves outside the folder (a symlink) is refused. A POST must carry
//! `X-IBC-Editor: 1`, which a page on another origin cannot send without a CORS preflight this server never
//! grants, and an `Origin`, if present, must be local.

use super::{is_local_host, percent_decode, Response};
use ibc_maptool::lint::{lint, Severity};
use serde_json::json;
use std::path::{Path, PathBuf};

pub const API_ROUTE: &str = "/api/editor";
pub const LINT_ROUTE: &str = "/api/lint";
pub const SAVE_ROUTE: &str = "/api/save";
pub const MAPS_PREFIX: &str = "/maps/";
/// Header every editor POST must carry.
pub const CSRF_HEADER: &str = "x-ibc-editor";
/// A 128x128 grid is about 16.5 KiB; this leaves room for header notes.
pub const MAX_BODY: usize = 64 * 1024;
const EXT: &str = "ibcmap";
const MAX_LISTED: usize = 500;

/// Is `name` a file stem the editor may read or write?
pub fn valid_name(name: &str) -> bool {
    const DEVICES: [&str; 4] = ["con", "prn", "aux", "nul"];
    let lower = name.to_ascii_lowercase();
    let device = DEVICES.contains(&lower.as_str())
        || ((lower.starts_with("com") || lower.starts_with("lpt"))
            && lower.len() == 4
            && lower.as_bytes()[3].is_ascii_digit());
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        && !device
}

/// `?a=b&c=d` (already split off the path) to the value of `key`, percent-decoded.
pub fn query_value(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (k == key).then(|| percent_decode(v)).flatten()
    })
}

/// A POST may change files on disk, so it must come from the editor page itself.
pub fn post_allowed(csrf: Option<&str>, origin: Option<&str>) -> bool {
    let origin_ok = match origin {
        None => true,
        Some(o) => o.strip_prefix("http://").is_some_and(is_local_host),
    };
    csrf == Some("1") && origin_ok
}

fn error(status: &'static str, code: &str, message: &str) -> Response {
    Response::json(status, &json!({ "error": code, "message": message }))
}

fn file_of(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.{EXT}"))
}

/// The file for `name` if it is inside `dir` (canonical). `Ok(None)`: it does not exist yet. On a
/// case-insensitive file system a file that differs only in case is refused, so one map never shows up
/// under two names.
fn existing_inside(dir: &Path, name: &str) -> Result<Option<PathBuf>, Response> {
    let file = file_of(dir, name);
    if std::fs::symlink_metadata(&file).is_err() {
        return Ok(None);
    }
    let wanted = format!("{name}.{EXT}");
    match file.canonicalize() {
        Ok(real) if real.starts_with(dir) && real.is_file() => match real.file_name().and_then(|f| f.to_str()) {
            Some(actual) if actual == wanted => Ok(Some(real)),
            actual => Err(error(
                "409 Conflict",
                "case",
                &format!("the folder has {} already; use that name", actual.unwrap_or("a file of that name")),
            )),
        },
        _ => Err(error("403 Forbidden", "outside", "that file is not a map inside the editor's folder")),
    }
}

/// GET routes of the editor. `None` for any other path, and for the map files without `--editor`.
pub fn get(dir: Option<&Path>, path: &str) -> Option<Response> {
    if path != API_ROUTE && !path.starts_with(MAPS_PREFIX) {
        return None;
    }
    let Some(dir) = dir else {
        // Answered rather than a 404, so the editor page under plain `ibc view` logs no failed request.
        return (path == API_ROUTE).then(|| Response::json("200 OK", &json!({ "editor": false })));
    };
    if path == API_ROUTE {
        return Some(Response::json("200 OK", &json!({ "maps": list(dir) })));
    }
    let file = &path[MAPS_PREFIX.len()..];
    let Some(name) = file.strip_suffix(".ibcmap").filter(|n| valid_name(n)) else {
        return Some(error("404 Not Found", "not_found", "no such map"));
    };
    Some(match existing_inside(dir, name) {
        Ok(Some(real)) => match std::fs::read(real) {
            Ok(body) => Response::ok("text/plain; charset=utf-8", body),
            Err(e) => error("500 Internal Server Error", "read", &e.to_string()),
        },
        Ok(None) => error("404 Not Found", "not_found", "no such map"),
        Err(r) => r,
    })
}

/// Map names (file stems) directly in `dir`, sorted.
pub fn list(dir: &Path) -> Vec<String> {
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == EXT))
        .filter_map(|p| p.file_stem()?.to_str().map(str::to_string))
        .filter(|n| valid_name(n))
        .collect();
    names.sort();
    names.truncate(MAX_LISTED);
    names
}

/// POST routes of the editor (the caller has checked `post_allowed`).
pub fn post(dir: Option<&Path>, path: &str, query: &str, body: &[u8]) -> Response {
    let Some(dir) = dir else {
        return error("404 Not Found", "no_editor", "start the server with `ibc view --editor`");
    };
    let Ok(text) = std::str::from_utf8(body) else {
        return error("400 Bad Request", "encoding", "the map text is not UTF-8");
    };
    let name = query_value(query, "name");
    match path {
        LINT_ROUTE => lint_response(text, name.as_deref().filter(|n| valid_name(n))),
        SAVE_ROUTE => match name {
            Some(name) if valid_name(&name) => {
                save(dir, &name, text, query_value(query, "overwrite").as_deref() == Some("1"))
            }
            _ => error("400 Bad Request", "bad_name", "a map name is 1 to 64 letters, digits, `_` or `-`"),
        },
        _ => error("404 Not Found", "not_found", "unknown editor route"),
    }
}

fn lint_response(text: &str, name: Option<&str>) -> Response {
    let issues = lint(text, name, &ibc_engine::Ruleset::builtin());
    let errors = issues.iter().filter(|i| i.severity == Severity::Error).count();
    let list: Vec<_> = issues
        .iter()
        .map(|i| {
            let severity = if i.severity == Severity::Error { "error" } else { "warning" };
            json!({ "severity": severity, "code": i.code, "message": i.message })
        })
        .collect();
    Response::json("200 OK", &json!({ "errors": errors, "warnings": issues.len() - errors, "issues": list }))
}

fn save(dir: &Path, name: &str, text: &str, overwrite: bool) -> Response {
    if let Err(e) = ibc_engine::Map::parse(text) {
        return error("422 Unprocessable Entity", "engine_parse", &format!("the engine rejects this map: {e}"));
    }
    let target = match existing_inside(dir, name) {
        Ok(Some(_)) if !overwrite => {
            return error("409 Conflict", "exists", &format!("{name}.{EXT} exists; save again to overwrite it"))
        }
        Ok(Some(real)) => real,
        Ok(None) => file_of(dir, name),
        Err(r) => return r,
    };
    let text = text.replace("\r\n", "\n");
    match std::fs::write(&target, text) {
        Ok(()) => {
            eprintln!("saved {}", target.display());
            Response::json("200 OK", &json!({ "file": format!("{name}.{EXT}") }))
        }
        Err(e) => error("500 Internal Server Error", "write", &e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAP: &str = "name: t\nteam_size: 1\n---\n#######\n#a.#.d#\n#..#11#\n#...11#\n#######\n";

    fn folder(name: &str) -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("ibc-editor-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let maps = base.join("maps");
        std::fs::create_dir_all(&maps).unwrap();
        std::fs::write(base.join("secret.ibcmap"), MAP).unwrap();
        std::fs::write(maps.join("one.ibcmap"), MAP).unwrap();
        std::fs::write(maps.join("notes.txt"), "x").unwrap();
        (base, maps.canonicalize().unwrap())
    }

    fn body(r: &Response) -> serde_json::Value {
        serde_json::from_slice(&r.body).unwrap()
    }

    #[test]
    fn names_are_plain_stems() {
        for ok in ["custom1", "my-map_2", "A"] {
            assert!(valid_name(ok), "{ok}");
        }
        let long = "x".repeat(65);
        for bad in ["", "../x", "..", "a/b", "a\\b", "c:x", "x.ibcmap", "con", "NUL", "com1", "LPT9", "a b", &long] {
            assert!(!valid_name(bad), "{bad}");
        }
        assert!(valid_name("console") && valid_name("com10"));
    }

    #[test]
    fn lists_and_reads_maps_only() {
        let (base, maps) = folder("read");
        assert_eq!(body(&get(Some(&maps), API_ROUTE).unwrap())["maps"], json!(["one"]));
        let r = get(Some(&maps), "/maps/one.ibcmap").unwrap();
        assert_eq!((r.status, r.body.as_slice()), ("200 OK", MAP.as_bytes()));
        for bad in ["/maps/../secret.ibcmap", "/maps/notes.txt", "/maps/missing.ibcmap", "/maps/one"] {
            assert_ne!(get(Some(&maps), bad).unwrap().status, "200 OK", "{bad}");
        }
        assert!(get(Some(&maps), "/index.html").is_none());
        assert_eq!(body(&get(None, API_ROUTE).unwrap()), json!({ "editor": false }));
        assert!(get(None, "/maps/one.ibcmap").is_none());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn lint_reports_maptool_findings() {
        let (base, maps) = folder("lint");
        let r = post(Some(&maps), LINT_ROUTE, "name=t", MAP.as_bytes());
        assert_eq!(r.status, "200 OK");
        let open = MAP.replace("#######\n#a", ".######\n#a");
        let v = body(&post(Some(&maps), LINT_ROUTE, "name=t", open.as_bytes()));
        assert!(v["errors"].as_u64().unwrap() >= 1, "{v}");
        assert!(v["issues"].as_array().unwrap().iter().any(|i| i["code"] == "open-border"), "{v}");
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn save_writes_inside_the_folder_only() {
        let (base, maps) = folder("save");
        let r = post(Some(&maps), SAVE_ROUTE, "name=custom1", MAP.as_bytes());
        assert_eq!(r.status, "200 OK", "{}", String::from_utf8_lossy(&r.body));
        assert_eq!(std::fs::read_to_string(maps.join("custom1.ibcmap")).unwrap(), MAP);
        assert_eq!(post(Some(&maps), SAVE_ROUTE, "name=custom1", MAP.as_bytes()).status, "409 Conflict");
        assert_eq!(post(Some(&maps), SAVE_ROUTE, "name=custom1&overwrite=1", MAP.as_bytes()).status, "200 OK");
        for query in ["name=..%2Fsecret", "name=../secret", "name=%2E%2E", "name=sub%2Fx", "name=con", "", "name="] {
            let r = post(Some(&maps), SAVE_ROUTE, query, MAP.as_bytes());
            assert_eq!(r.status, "400 Bad Request", "{query}");
        }
        assert_eq!(std::fs::read_to_string(base.join("secret.ibcmap")).unwrap(), MAP);
        if maps.join("ONE.ibcmap").exists() {
            // A case-insensitive file system: `ONE` is `one.ibcmap`, and is refused rather than overwritten.
            let r = post(Some(&maps), SAVE_ROUTE, "name=ONE&overwrite=1", MAP.as_bytes());
            assert_eq!((r.status, body(&r)["error"].as_str()), ("409 Conflict", Some("case")));
        }
        let broken = post(Some(&maps), SAVE_ROUTE, "name=broken", b"name: x\n---\n###\n");
        assert_eq!(broken.status, "422 Unprocessable Entity");
        assert!(!maps.join("broken.ibcmap").exists());
        assert_eq!(post(None, SAVE_ROUTE, "name=x", MAP.as_bytes()).status, "404 Not Found");
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn posts_need_the_editor_header_and_a_local_origin() {
        assert!(post_allowed(Some("1"), None));
        assert!(post_allowed(Some("1"), Some("http://127.0.0.1:18020")));
        assert!(post_allowed(Some("1"), Some("http://localhost:1")));
        assert!(!post_allowed(None, None));
        assert!(!post_allowed(Some("1"), Some("http://evil.example")));
        assert!(!post_allowed(Some("1"), Some("null")));
        assert!(!post_allowed(Some("1"), Some("https://127.0.0.1.evil.example")));
    }
}
