//! `ibc view [replay.ibcr] [--port N] [--viewer-dir DIR]`: serve the built replay viewer and one replay on
//! localhost and print the URL.
//!
//! The viewer is a static site built with Node (`viewer/`, `npm run build`). It is deliberately not embedded
//! in this binary, so building `ibc` never needs Node: the files are served from a directory instead, found
//! through `--viewer-dir`, `IBC_VIEWER_DIR`, or a `viewer/dist` next to the working directory or the binary.
//!
//! The server is std-only, binds to 127.0.0.1, answers GET and HEAD, and serves nothing outside the viewer
//! directory except the one replay at `/replay.ibcr`.

use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const REPLAY_ROUTE: &str = "/replay.ibcr";
const VIEWER_DIR_ENV: &str = "IBC_VIEWER_DIR";

#[derive(Parser, Debug)]
#[command(name = "ibc view", about = "Serve the replay viewer on localhost and print its URL")]
struct Args {
    /// Replay to open (without one the viewer starts empty: drop a file onto the page)
    replay: Option<PathBuf>,
    /// Port to listen on (default: any free port)
    #[arg(long, default_value_t = 0)]
    port: u16,
    /// Directory with the built viewer (`viewer/dist`); also read from IBC_VIEWER_DIR
    #[arg(long)]
    viewer_dir: Option<PathBuf>,
}

struct Site {
    /// Canonical path of the viewer directory.
    root: PathBuf,
    replay: Option<PathBuf>,
}

struct Response {
    status: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
}

impl Response {
    fn text(status: &'static str, message: &str) -> Response {
        Response { status, content_type: "text/plain; charset=utf-8", body: format!("{message}\n").into_bytes() }
    }
}

pub fn run(args: Vec<String>) -> Result<()> {
    let args = Args::parse_from(std::iter::once("ibc view".to_string()).chain(args));
    let root = find_viewer_dir(args.viewer_dir.as_deref())?;
    if let Some(replay) = &args.replay {
        let bytes = std::fs::read(replay).with_context(|| format!("reading {}", replay.display()))?;
        ibc_replay::Replay::decode(&bytes).map_err(|e| anyhow!("{}: {e}", replay.display()))?;
    }
    let listener =
        TcpListener::bind(("127.0.0.1", args.port)).with_context(|| format!("listening on 127.0.0.1:{}", args.port))?;
    let port = listener.local_addr()?.port();
    let query = if args.replay.is_some() { format!("?replay={REPLAY_ROUTE}") } else { String::new() };
    println!("http://127.0.0.1:{port}/{query}");
    eprintln!("serving {} (Ctrl-C to stop)", root.display());
    serve(listener, Arc::new(Site { root, replay: args.replay }));
    Ok(())
}

/// The directory to serve: the first candidate that holds an `index.html`.
fn find_viewer_dir(flag: Option<&Path>) -> Result<PathBuf> {
    let explicit = flag.map(Path::to_path_buf).or_else(|| std::env::var_os(VIEWER_DIR_ENV).map(PathBuf::from));
    if let Some(dir) = explicit {
        if !dir.join("index.html").is_file() {
            bail!("{} has no index.html: build the viewer with `npm run build` in viewer/", dir.display());
        }
        return dir.canonicalize().with_context(|| format!("resolving {}", dir.display()));
    }
    // A checkout: `viewer/dist` beside the working directory or beside the binary (target/release/ibc).
    let mut starts = Vec::new();
    starts.extend(std::env::current_dir().ok());
    starts.extend(std::env::current_exe().ok());
    for start in &starts {
        for dir in start.ancestors() {
            let candidate = dir.join("viewer").join("dist");
            if candidate.join("index.html").is_file() {
                return candidate.canonicalize().with_context(|| format!("resolving {}", candidate.display()));
            }
        }
    }
    bail!(
        "the built viewer was not found. Build it (`cd viewer && npm install && npm run build`) and run \
         `ibc view` from the repository, or point --viewer-dir / {VIEWER_DIR_ENV} at a built `viewer/dist`"
    )
}

fn serve(listener: TcpListener, site: Arc<Site>) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let site = Arc::clone(&site);
        // A page load is a handful of small requests; a thread each keeps a slow client from blocking the rest.
        std::thread::spawn(move || {
            let _ = handle(stream, &site);
        });
    }
}

fn handle(stream: TcpStream, site: &Site) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let mut reader = BufReader::new(stream.try_clone()?.take(16 * 1024));
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    // Read the headers to the blank line (so the client sees a clean close), keeping only Host.
    let mut host = None;
    let mut line = String::new();
    while reader.read_line(&mut line)? > 2 {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("host") {
                host = Some(value.trim().to_string());
            }
        }
        line.clear();
    }
    let mut parts = request_line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let response = match method {
        _ if !host.as_deref().map_or(true, is_local_host) => Response::text("403 Forbidden", "not a local host name"),
        "GET" | "HEAD" => respond(site, target),
        _ => Response::text("405 Method Not Allowed", "only GET and HEAD are supported"),
    };
    let mut out = stream;
    write!(
        out,
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        response.status,
        response.content_type,
        response.body.len()
    )?;
    if method != "HEAD" {
        out.write_all(&response.body)?;
    }
    out.flush()
}

/// A page on another site whose name is re-pointed at 127.0.0.1 (DNS rebinding) could otherwise read the
/// replay through the browser. Such a request still carries the foreign name in `Host`.
fn is_local_host(host: &str) -> bool {
    let name = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => host.rsplit_once(':').map_or(host, |(name, _port)| name),
    };
    matches!(name.to_ascii_lowercase().as_str(), "127.0.0.1" | "localhost" | "::1")
}

fn respond(site: &Site, target: &str) -> Response {
    let path = target.split(['?', '#']).next().unwrap_or("");
    let Some(path) = percent_decode(path) else {
        return Response::text("400 Bad Request", "bad path encoding");
    };
    if path == REPLAY_ROUTE {
        // Read on every request: re-running a match and reloading the page shows the new replay.
        return match site.replay.as_ref().map(std::fs::read) {
            Some(Ok(body)) => Response { status: "200 OK", content_type: "application/octet-stream", body },
            Some(Err(e)) => Response::text("500 Internal Server Error", &format!("reading the replay: {e}")),
            None => Response::text("404 Not Found", "no replay was given to `ibc view`"),
        };
    }
    let Some(relative) = safe_relative(&path) else {
        return Response::text("400 Bad Request", "bad path");
    };
    let mut file = site.root.join(relative);
    if file.is_dir() {
        file.push("index.html");
    }
    // Resolving links and then checking the prefix keeps a symlink in the directory from leading outside it.
    match file.canonicalize() {
        Ok(real) if real.starts_with(&site.root) && real.is_file() => match std::fs::read(&real) {
            Ok(body) => Response { status: "200 OK", content_type: content_type(&real), body },
            Err(e) => Response::text("500 Internal Server Error", &e.to_string()),
        },
        _ => Response::text("404 Not Found", "not found"),
    }
}

/// A URL path as a relative file path, or `None` if it tries to leave the directory.
fn safe_relative(path: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in path.strip_prefix('/')?.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            p if p.contains(['\\', ':', '\0']) => return None,
            p => out.push(p),
        }
    }
    Some(out)
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "wasm" => "application/wasm",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A viewer directory with a few files, and a secret next to it that must never be served.
    fn site(name: &str, replay: bool) -> (PathBuf, Site) {
        let base = std::env::temp_dir().join(format!("ibc-view-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("dist");
        std::fs::create_dir_all(root.join("engine")).unwrap();
        std::fs::write(root.join("index.html"), "<!doctype html>viewer").unwrap();
        std::fs::write(root.join("engine").join("0.1.0.wasm"), b"\0asm").unwrap();
        std::fs::write(base.join("secret.txt"), "secret").unwrap();
        std::fs::write(base.join("match.ibcr"), b"IBCRdata").unwrap();
        let replay = replay.then(|| base.join("match.ibcr"));
        (base, Site { root: root.canonicalize().unwrap(), replay })
    }

    #[test]
    fn serves_viewer_files_with_types() {
        let (base, site) = site("files", false);
        let index = respond(&site, "/?replay=/replay.ibcr");
        assert_eq!((index.status, index.content_type), ("200 OK", "text/html; charset=utf-8"));
        assert_eq!(index.body, b"<!doctype html>viewer");
        let wasm = respond(&site, "/engine/0.1.0.wasm");
        assert_eq!((wasm.status, wasm.content_type), ("200 OK", "application/wasm"));
        assert_eq!(respond(&site, "/engine/0%2E1.0.wasm").status, "200 OK");
        assert_eq!(respond(&site, "/missing.js").status, "404 Not Found");
        assert_eq!(respond(&site, "/replay.ibcr").status, "404 Not Found");
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn never_leaves_the_viewer_directory() {
        let (base, site) = site("escape", false);
        for target in [
            "/../secret.txt",
            "/%2e%2e/secret.txt",
            "/engine/../../secret.txt",
            "/..%5csecret.txt",
            "/c:/windows/win.ini",
            "secret.txt",
            "/%zz",
            "/%ff",
        ] {
            let r = respond(&site, target);
            assert_ne!(r.status, "200 OK", "{target}");
            assert_ne!(r.body, b"secret", "{target}");
        }
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn serves_the_replay_over_http() {
        let (base, site) = site("http", true);
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || serve(listener, Arc::new(site)));
        let request = |text: &str| {
            let mut s = TcpStream::connect(addr).unwrap();
            s.write_all(text.as_bytes()).unwrap();
            let mut out = Vec::new();
            s.read_to_end(&mut out).unwrap();
            String::from_utf8_lossy(&out).into_owned()
        };
        let get = request("GET /replay.ibcr HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
        assert!(get.starts_with("HTTP/1.1 200 OK\r\n"), "{get}");
        assert!(get.contains("Content-Length: 8\r\n") && get.ends_with("\r\n\r\nIBCRdata"), "{get}");
        let head = request("HEAD / HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert!(head.starts_with("HTTP/1.1 200 OK\r\n") && head.ends_with("\r\n\r\n"), "{head}");
        assert!(request("POST / HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 405 "));
        let rebound = request("GET /replay.ibcr HTTP/1.1\r\nhost: evil.example:8080\r\n\r\n");
        assert!(rebound.starts_with("HTTP/1.1 403 ") && !rebound.contains("IBCRdata"), "{rebound}");
        let local = format!("GET /replay.ibcr HTTP/1.1\r\nHost: localhost:{}\r\n\r\n", addr.port());
        assert!(request(&local).starts_with("HTTP/1.1 200 OK\r\n"));
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn recognises_local_host_names() {
        for ok in ["127.0.0.1", "127.0.0.1:8080", "localhost:1", "LOCALHOST", "[::1]:9", "[::1]"] {
            assert!(is_local_host(ok), "{ok}");
        }
        for bad in ["evil.example", "evil.example:80", "127.0.0.1.evil.example", "localhost.evil.example:1", ""] {
            assert!(!is_local_host(bad), "{bad}");
        }
    }

    #[test]
    fn finds_an_explicit_viewer_dir_or_explains() {
        let (base, site) = site("find", false);
        assert_eq!(find_viewer_dir(Some(&base.join("dist"))).unwrap(), site.root);
        let err = find_viewer_dir(Some(&base)).unwrap_err().to_string();
        assert!(err.contains("npm run build"), "{err}");
        std::fs::remove_dir_all(base).unwrap();
    }
}
