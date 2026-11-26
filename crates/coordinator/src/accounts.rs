//! Accounts: the settings of sign-in, password hashing (argon2id) and the shapes of emails, names and codes.
//! The HTTP side is `api::auth` and `api::teams`, the SQL is `db::users` (docs/api/auth.md).

use crate::error::{ApiError, ApiResult};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use rand::{Rng, RngCore};

/// A browser session lasts this long from sign-in.
pub const SESSION_MS: i64 = 7 * 86_400_000;
/// A device code (CLI sign-in) must be approved within this time.
pub const DEVICE_TTL_MS: i64 = 600_000;
/// Seconds a CLI waits between two polls of a device code.
pub const DEVICE_INTERVAL_SECS: i64 = 5;
/// Teams one account may create over its lifetime (each team has its own unranked quota).
pub const MAX_TEAMS_CREATED: i64 = 3;
/// Name of the session cookie.
pub const SESSION_COOKIE: &str = "ibc_session";

#[derive(Clone, Debug)]
pub struct AuthConfig {
    /// Most members of one team (`IBC_TEAM_MAX`).
    pub team_max: i64,
    /// Mark the session cookie `Secure` (`IBC_COOKIE_SECURE=1`; set it behind HTTPS).
    pub cookie_secure: bool,
    /// Lower-case emails promoted to admin when they register or sign in (`IBC_ADMIN_EMAILS`).
    pub admin_emails: Vec<String>,
    /// Base URL of the web site, used in the device-login `verify_url` (`IBC_PUBLIC_URL`).
    pub public_url: String,
    /// argon2id memory cost in KiB. The default is OWASP's 19 MiB; tests lower it.
    pub argon2_memory_kib: u32,
    /// Failed sign-ins per email within `login_window_ms` before sign-in answers 429.
    pub login_max_failures: i64,
    pub login_window_ms: i64,
}

impl Default for AuthConfig {
    fn default() -> AuthConfig {
        AuthConfig {
            team_max: 4,
            cookie_secure: false,
            admin_emails: Vec::new(),
            public_url: "http://127.0.0.1:8080".to_string(),
            argon2_memory_kib: 19 * 1024,
            login_max_failures: 10,
            login_window_ms: 15 * 60_000,
        }
    }
}

impl AuthConfig {
    pub fn is_admin_email(&self, email: &str) -> bool {
        self.admin_emails.iter().any(|e| e == email)
    }
}

/// The command-line half of `AuthConfig` (flattened into `ibc-coordinator`'s flags).
#[derive(clap::Args, Clone, Debug)]
pub struct AuthArgs {
    /// Most members of one team
    #[arg(long, env = "IBC_TEAM_MAX", default_value_t = 4)]
    pub team_max: i64,
    /// Mark the session cookie Secure (set it when the site is served over HTTPS)
    #[arg(long, env = "IBC_COOKIE_SECURE", value_parser = clap::builder::FalseyValueParser::new())]
    pub cookie_secure: bool,
    /// Comma-separated emails that become admins when they register or sign in
    #[arg(long, env = "IBC_ADMIN_EMAILS", value_delimiter = ',')]
    pub admin_emails: Vec<String>,
    /// Public base URL of the site (device-login links point here). Default: http://<listen>
    #[arg(long, env = "IBC_PUBLIC_URL")]
    pub public_url: Option<String>,
}

impl AuthArgs {
    pub fn config(&self, listen: std::net::SocketAddr) -> anyhow::Result<AuthConfig> {
        if !(1..=64).contains(&self.team_max) {
            anyhow::bail!("--team-max must be between 1 and 64");
        }
        Ok(AuthConfig {
            team_max: self.team_max,
            cookie_secure: self.cookie_secure,
            admin_emails: self
                .admin_emails
                .iter()
                .filter_map(|e| normalize_email(e))
                .collect(),
            public_url: self
                .public_url
                .clone()
                .unwrap_or_else(|| default_public_url(listen))
                .trim_end_matches('/')
                .to_string(),
            ..AuthConfig::default()
        })
    }
}

/// `http://<listen>`; a wildcard listen address cannot be opened from a browser, so it becomes localhost.
fn default_public_url(listen: std::net::SocketAddr) -> String {
    if listen.ip().is_unspecified() {
        eprintln!(
            "warning: IBC_PUBLIC_URL is not set; device-login links will point at localhost:{}",
            listen.port()
        );
        format!("http://localhost:{}", listen.port())
    } else {
        format!("http://{listen}")
    }
}

/// Trimmed, lower-case email, or `None` if it does not look like one.
pub fn normalize_email(email: &str) -> Option<String> {
    let email = email.trim().to_lowercase();
    let (local, domain) = email.split_once('@')?;
    let ok = !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains('@')
        && email.len() <= 254
        && !email.chars().any(|c| c.is_whitespace() || c.is_control());
    ok.then_some(email)
}

pub fn check_password(password: &str) -> ApiResult<()> {
    let n = password.chars().count();
    if (8..=256).contains(&n) {
        Ok(())
    } else {
        Err(ApiError::bad_request("a password has 8 to 256 characters"))
    }
}

/// Trimmed display name of 1 to 40 printable characters.
pub fn check_display_name(name: &str) -> ApiResult<String> {
    let name = name.trim();
    let n = name.chars().count();
    if (1..=40).contains(&n) && !name.chars().any(char::is_control) {
        Ok(name.to_string())
    } else {
        Err(ApiError::bad_request(
            "a display name has 1 to 40 printable characters",
        ))
    }
}

fn argon2(memory_kib: u32) -> ApiResult<Argon2<'static>> {
    let params = Params::new(memory_kib, 2, 1, None).map_err(ApiError::internal)?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

/// A PHC string (`$argon2id$v=19$m=...`): the parameters and salt travel with the hash.
pub fn hash_password(password: &str, memory_kib: u32) -> ApiResult<String> {
    let mut salt = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut salt);
    let salt = SaltString::encode_b64(&salt).map_err(ApiError::internal)?;
    Ok(argon2(memory_kib)?
        .hash_password(password.as_bytes(), &salt)
        .map_err(ApiError::internal)?
        .to_string())
}

/// Check a password against a stored PHC string (constant time inside argon2). With no stored hash, a hash
/// is still computed so that an unknown email takes as long as a wrong password.
pub fn verify_password(stored: Option<&str>, password: &str, memory_kib: u32) -> bool {
    let Some(stored) = stored else {
        let _ = hash_password(password, memory_kib);
        return false;
    };
    let Ok(parsed) = PasswordHash::new(stored) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// Letters that cannot be misread for one another (RFC 8628 suggests consonants only).
const CODE_ALPHABET: &[u8] = b"BCDFGHJKLMNPQRSTVWXZ";

fn random_code(len: usize) -> String {
    let mut rng = rand::rngs::OsRng;
    (0..len)
        .map(|_| CODE_ALPHABET[rng.gen_range(0..CODE_ALPHABET.len())] as char)
        .collect()
}

/// `ABCD-EFGH`: what a person types on the device page.
pub fn new_user_code() -> String {
    let code = random_code(8);
    format!("{}-{}", &code[..4], &code[4..])
}

/// A user code as typed (any case, with or without the dash or spaces) in its stored form.
pub fn normalize_user_code(code: &str) -> String {
    let flat: String = code
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if flat.len() == 8 {
        format!("{}-{}", &flat[..4], &flat[4..])
    } else {
        flat
    }
}

/// A team join code: 10 letters, shared with teammates.
pub fn new_join_code() -> String {
    random_code(10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emails_and_codes() {
        assert_eq!(
            normalize_email("  Ada@Example.ORG "),
            Some("ada@example.org".into())
        );
        for bad in ["", "ada", "@x.org", "ada@x", "ada@.org", "a b@x.org"] {
            assert_eq!(normalize_email(bad), None, "{bad}");
        }
        let code = new_user_code();
        assert_eq!(code.len(), 9);
        assert_eq!(
            normalize_user_code(&code.to_lowercase().replace('-', " ")),
            code
        );
        assert_eq!(new_join_code().len(), 10);
        assert_ne!(new_join_code(), new_join_code());
    }

    #[test]
    fn passwords_round_trip() {
        let hash = hash_password("correct horse", 64).unwrap();
        assert!(hash.starts_with("$argon2id$v=19$m=64,t=2,p=1$"));
        assert!(verify_password(Some(&hash), "correct horse", 64));
        assert!(!verify_password(Some(&hash), "correct horsE", 64));
        assert!(!verify_password(None, "correct horse", 64));
        assert!(!verify_password(Some("garbage"), "correct horse", 64));
    }
}
