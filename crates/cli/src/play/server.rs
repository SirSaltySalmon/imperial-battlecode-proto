//! The localhost HTTP server of `ibc play`: the built viewer's play page and the session's endpoints.
//!
//! | Route | |
//! |---|---|
//! | `GET /` | the play page (`play.html` of the built viewer); other paths are the viewer's files |
//! | `GET /setup` | map text, rules, names, the player's seat and the mode (fixed for the session) |
//! | `GET /state` | the player's team's fogged view after the latest tick, the last acts and their results |
//! | `POST /input` | an `Input` (held controls and queued actions) |
//! | `POST /step` | optional `Input`, then one tick; answers with the state after it |
//! | `POST /run` | `{"running": bool}`: start or pause the real-time clock |
//! | `POST /quit` | end the match where it is (recorded as unfinished) and stop `ibc play` |
//!
//! Std only, bound to 127.0.0.1 like `ibc view`. Requests must name a local host (DNS rebinding), and every
//! POST must carry the header `X-IBC-Play: 1`: a page on another site cannot send a custom header to this
//! origin without a CORS preflight, which is never granted, so it cannot steer the player's unit.

use super::input::Input;
use super::session::Session;
use crate::view::{content_type, is_local_host, percent_decode, safe_relative};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub const PLAY_HEADER: &str = "x-ibc-play";
const MAX_BODY: usize = 64 * 1024;

pub struct Site {
    /// Canonical path of the built viewer.
    pub root: PathBuf,
    pub session: Arc<Session>,
}

#[derive(Debug)]
pub struct Response {
    pub status: &'static str,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Response {
    fn json(value: &Value) -> Response {
        Response { status: "200 OK", content_type: "application/json", body: value.to_string().into_bytes() }
    }

    fn error(status: &'static str, message: &str) -> Response {
        let body = json!({ "error": status, "message": message }).to_string().into_bytes();
        Response { status, content_type: "application/json", body }
    }
}

/// A parsed request: what the routes need.
#[derive(Debug, Default)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub host: Option<String>,
    pub play_header: bool,
    pub body: Vec<u8>,
}

pub fn serve(listener: TcpListener, site: Arc<Site>) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let site = Arc::clone(&site);
        std::thread::spawn(move || {
            let _ = handle(stream, &site);
        });
    }
}

fn handle(stream: TcpStream, site: &Site) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let request = read_request(&stream)?;
    let response = respond(site, &request);
    let mut out = stream;
    write!(
        out,
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        response.status,
        response.content_type,
        response.body.len()
    )?;
    if request.method != "HEAD" {
        out.write_all(&response.body)?;
    }
    out.flush()
}

fn read_request(stream: &TcpStream) -> std::io::Result<Request> {
    let mut reader = BufReader::new(stream.try_clone()?.take((16 * 1024 + MAX_BODY) as u64));
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let mut req = Request {
        method: parts.next().unwrap_or("").to_string(),
        path: parts.next().unwrap_or("").to_string(),
        ..Request::default()
    };
    let mut length = 0usize;
    line.clear();
    while reader.read_line(&mut line)? > 2 {
        if let Some((name, value)) = line.split_once(':') {
            let value = value.trim();
            match name.trim().to_ascii_lowercase().as_str() {
                "host" => req.host = Some(value.to_string()),
                "content-length" => length = value.parse().unwrap_or(usize::MAX),
                PLAY_HEADER => req.play_header = value == "1",
                _ => {}
            }
        }
        line.clear();
    }
    if length <= MAX_BODY {
        req.body = vec![0; length];
        reader.read_exact(&mut req.body)?;
    } else {
        req.body = b"\xff".to_vec(); // rejected below as not JSON
    }
    Ok(req)
}

pub fn respond(site: &Site, req: &Request) -> Response {
    if !req.host.as_deref().map_or(true, is_local_host) {
        return Response::error("403 Forbidden", "not a local host name");
    }
    let path = req.path.split(['?', '#']).next().unwrap_or("");
    match req.method.as_str() {
        "GET" | "HEAD" => get(site, path),
        "POST" if !req.play_header => Response::error("403 Forbidden", "POST needs the header X-IBC-Play: 1"),
        "POST" => post(&site.session, path, &req.body),
        _ => Response::error("405 Method Not Allowed", "only GET, HEAD and POST"),
    }
}

fn get(site: &Site, path: &str) -> Response {
    match path {
        "/setup" => Response::json(&site.session.setup),
        "/state" => Response::json(&site.session.state()),
        "/" => static_file(site, "/play.html"),
        _ => static_file(site, path),
    }
}

fn post(session: &Session, path: &str, body: &[u8]) -> Response {
    let json: Value = match body.iter().all(u8::is_ascii_whitespace) {
        true => Value::Null,
        false => match serde_json::from_slice(body) {
            Ok(v) => v,
            Err(e) => return Response::error("400 Bad Request", &format!("body is not JSON: {e}")),
        },
    };
    let input = || -> Result<Option<Input>, Response> {
        match &json {
            Value::Null => Ok(None),
            v => serde_json::from_value(v.clone())
                .map(Some)
                .map_err(|e| Response::error("400 Bad Request", &format!("bad input: {e}"))),
        }
    };
    match path {
        "/input" => match input() {
            Ok(Some(i)) => {
                session.input(i);
                Response::json(&json!({ "ok": true }))
            }
            Ok(None) => Response::error("400 Bad Request", "POST /input needs an input object"),
            Err(r) => r,
        },
        "/step" => match input() {
            Ok(i) => Response::json(&session.step(i)),
            Err(r) => r,
        },
        "/run" => match json["running"].as_bool() {
            Some(on) => {
                session.set_running(on);
                Response::json(&session.state())
            }
            None => Response::error("400 Bad Request", r#"POST /run needs {"running": true|false}"#),
        },
        "/quit" => {
            session.quit();
            Response::json(&json!({ "ok": true }))
        }
        _ => Response::error("404 Not Found", "no such endpoint"),
    }
}

fn static_file(site: &Site, path: &str) -> Response {
    let Some(relative) = percent_decode(path).as_deref().and_then(safe_relative) else {
        return Response::error("400 Bad Request", "bad path");
    };
    let mut file = site.root.join(relative);
    if file.is_dir() {
        file.push("play.html");
    }
    // Resolving links and then checking the prefix keeps a symlink in the directory from leading outside it.
    match file.canonicalize() {
        Ok(real) if real.starts_with(&site.root) && real.is_file() => match std::fs::read(&real) {
            Ok(body) => Response { status: "200 OK", content_type: content_type(&real), body },
            Err(e) => Response::error("500 Internal Server Error", &e.to_string()),
        },
        _ => Response::error("404 Not Found", "not found"),
    }
}
