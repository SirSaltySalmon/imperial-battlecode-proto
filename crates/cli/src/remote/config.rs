//! Where the platform is and who we are: `credentials.json` in the per-user config directory, overridden by
//! `IBC_SERVER` / `IBC_TOKEN` and `--server`.
//!
//! A stored token is only ever sent to the server it was issued by: pointing `--server` or `IBC_SERVER`
//! somewhere else drops it (an `IBC_TOKEN` is sent to whichever server is chosen).

use super::client::{CliError, CliResult};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const ENV_SERVER: &str = "IBC_SERVER";
pub const ENV_TOKEN: &str = "IBC_TOKEN";
/// Overrides the config directory (tests, CI, several accounts side by side).
pub const ENV_CONFIG_DIR: &str = "IBC_CONFIG_DIR";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    pub server: String,
    pub token: String,
}

/// The kind of a bearer token, from its prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// `ibu…`: a user signed in (device login); acts for the user's team.
    User,
    /// `ibt…`: a team token handed out by the organisers.
    Team,
    Unknown,
}

impl TokenKind {
    pub fn of(token: &str) -> TokenKind {
        if token.starts_with("ibu") {
            TokenKind::User
        } else if token.starts_with("ibt") {
            TokenKind::Team
        } else {
            TokenKind::Unknown
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            TokenKind::User => "user",
            TokenKind::Team => "team",
            TokenKind::Unknown => "unknown",
        }
    }
}

/// `https://host/v1/` -> `https://host`. Anything that is not an http(s) URL is a usage error.
pub fn normalize_server(raw: &str) -> CliResult<String> {
    let s = raw.trim().trim_end_matches('/');
    let s = s.strip_suffix("/v1").unwrap_or(s).trim_end_matches('/');
    let host = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"));
    match host {
        Some(h) if !h.is_empty() => Ok(s.to_string()),
        _ => Err(CliError::usage(format!(
            "server {raw:?} is not an http:// or https:// URL"
        ))),
    }
}

/// The per-user config directory of `ibc`: `%APPDATA%\ibc` on Windows, `$XDG_CONFIG_HOME/ibc` or
/// `~/.config/ibc` elsewhere.
pub fn config_dir() -> CliResult<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(dir) = var(ENV_CONFIG_DIR) {
        return Ok(dir);
    }
    let base = if cfg!(windows) {
        var("APPDATA")
    } else {
        var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|h| h.join(".config")))
    };
    base.map(|b| b.join("ibc")).ok_or_else(|| {
        CliError::other(format!(
            "cannot find a per-user config directory; set {ENV_CONFIG_DIR}"
        ))
    })
}

pub fn credentials_path() -> CliResult<PathBuf> {
    Ok(config_dir()?.join("credentials.json"))
}

pub fn load(path: &Path) -> CliResult<Option<Credentials>> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|e| {
            CliError::other(format!(
                "{}: {e} (delete it or run `ibc logout`)",
                path.display()
            ))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CliError::other(format!("{}: {e}", path.display()))),
    }
}

/// Write the credentials, readable by the owner only on Unix.
pub fn save(path: &Path, creds: &Credentials) -> CliResult<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| CliError::other(format!("{}: {e}", dir.display())))?;
    }
    let text =
        serde_json::to_string_pretty(creds).map_err(|e| CliError::other(e.to_string()))? + "\n";
    // A fresh owner-only file renamed over the old one: never a half-written or world-readable token.
    let tmp = path.with_extension("json.tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    use std::io::Write;
    opts.open(&tmp)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|e| CliError::other(format!("{}: {e}", path.display())))
}

/// The server and token a command uses.
#[derive(Debug, PartialEq, Eq)]
pub struct Resolved {
    pub server: String,
    pub token: Option<String>,
}

/// Server: `--server` > `IBC_SERVER` > stored. Token: `IBC_TOKEN` > the stored token when it belongs to the
/// chosen server. No server anywhere is "sign in first" (exit 3).
pub fn resolve(
    flag_server: Option<&str>,
    env_server: Option<&str>,
    env_token: Option<&str>,
    stored: Option<&Credentials>,
) -> CliResult<Resolved> {
    let raw = flag_server
        .or(env_server)
        .or(stored.map(|c| c.server.as_str()))
        .ok_or_else(|| {
            CliError::auth(format!(
            "no server configured: run `ibc login --server URL`, pass --server or set {ENV_SERVER}"
        ))
        })?;
    let server = normalize_server(raw)?;
    let token = match env_token.filter(|t| !t.is_empty()) {
        Some(t) => Some(t.to_string()),
        None => stored
            .filter(|c| normalize_server(&c.server).is_ok_and(|s| s == server))
            .map(|c| c.token.clone()),
    };
    Ok(Resolved { server, token })
}

/// `resolve` with the real environment and credentials file.
pub fn resolve_env(flag_server: Option<&str>) -> CliResult<Resolved> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let stored = load(&credentials_path()?)?;
    resolve(
        flag_server,
        env(ENV_SERVER).as_deref(),
        env(ENV_TOKEN).as_deref(),
        stored.as_ref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creds(server: &str) -> Credentials {
        Credentials {
            server: server.into(),
            token: "ibu_stored".into(),
        }
    }

    #[test]
    fn server_urls_are_normalized() {
        assert_eq!(
            normalize_server("https://ibc.example/v1/").unwrap(),
            "https://ibc.example"
        );
        assert_eq!(
            normalize_server(" http://127.0.0.1:8080/ ").unwrap(),
            "http://127.0.0.1:8080"
        );
        assert!(normalize_server("ibc.example").is_err());
        assert!(normalize_server("https://").is_err());
    }

    #[test]
    fn flag_beats_env_beats_stored() {
        let stored = creds("https://stored");
        let r = resolve(
            Some("https://flag"),
            Some("https://env"),
            None,
            Some(&stored),
        )
        .unwrap();
        assert_eq!(r.server, "https://flag");
        let r = resolve(None, Some("https://env"), None, Some(&stored)).unwrap();
        assert_eq!(r.server, "https://env");
        let r = resolve(None, None, None, Some(&stored)).unwrap();
        assert_eq!(
            r,
            Resolved {
                server: "https://stored".into(),
                token: Some("ibu_stored".into())
            }
        );
    }

    #[test]
    fn stored_token_only_goes_to_its_own_server() {
        let stored = creds("https://stored/v1");
        assert_eq!(
            resolve(Some("https://other"), None, None, Some(&stored))
                .unwrap()
                .token,
            None
        );
        assert_eq!(
            resolve(Some("https://stored/"), None, None, Some(&stored))
                .unwrap()
                .token
                .as_deref(),
            Some("ibu_stored")
        );
        // IBC_TOKEN goes wherever the server is.
        let r = resolve(Some("https://other"), None, Some("ibt_env"), Some(&stored)).unwrap();
        assert_eq!(r.token.as_deref(), Some("ibt_env"));
    }

    #[test]
    fn no_server_is_an_auth_error() {
        let e = resolve(None, None, Some("ibt_x"), None).unwrap_err();
        assert_eq!(e.exit, super::super::client::EXIT_AUTH);
    }

    #[test]
    fn credentials_round_trip() {
        let dir = std::env::temp_dir().join(format!("ibc-cfg-test-{}", std::process::id()));
        let path = dir.join("credentials.json");
        assert_eq!(load(&path).unwrap(), None);
        save(&path, &creds("https://x")).unwrap();
        assert_eq!(load(&path).unwrap(), Some(creds("https://x")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn token_kinds() {
        assert_eq!(TokenKind::of("ibu_abc"), TokenKind::User);
        assert_eq!(TokenKind::of("ibt_abc"), TokenKind::Team);
        assert_eq!(TokenKind::of("xyz"), TokenKind::Unknown);
    }
}
