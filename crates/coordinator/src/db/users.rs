//! Users, sessions, user tokens, device codes, team membership and sign-in throttling (docs/api/auth.md).
//! Every secret (session token, `ibu` token, device code) is stored as its SHA-256 only.

use super::{is_unique_violation, TeamRow};
use crate::error::{ApiError, ApiResult};
use crate::util::random_hex;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS users(
  id TEXT PRIMARY KEY, email TEXT NOT NULL UNIQUE COLLATE NOCASE, display_name TEXT NOT NULL,
  password_hash TEXT NOT NULL, is_admin INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL, banned_at INTEGER,
  teams_created INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS sessions(
  token_hash TEXT PRIMARY KEY, user_id TEXT NOT NULL REFERENCES users(id), created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS sessions_user ON sessions(user_id);
CREATE TABLE IF NOT EXISTS user_tokens(
  token_hash TEXT PRIMARY KEY, user_id TEXT NOT NULL REFERENCES users(id), label TEXT NOT NULL,
  created_at INTEGER NOT NULL, last_used INTEGER);
CREATE INDEX IF NOT EXISTS user_tokens_user ON user_tokens(user_id);
CREATE TABLE IF NOT EXISTS device_codes(
  device_code_hash TEXT PRIMARY KEY, user_code TEXT NOT NULL UNIQUE, user_id TEXT REFERENCES users(id),
  created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL, last_poll INTEGER);
CREATE TABLE IF NOT EXISTS team_members(
  team_id TEXT NOT NULL REFERENCES teams(id), user_id TEXT NOT NULL UNIQUE REFERENCES users(id),
  role TEXT NOT NULL, joined_at INTEGER NOT NULL, PRIMARY KEY(team_id, user_id));
CREATE TABLE IF NOT EXISTS login_failures(email TEXT NOT NULL, at INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS login_failures_email ON login_failures(email, at);
CREATE UNIQUE INDEX IF NOT EXISTS teams_join_code ON teams(join_code);
";

/// Create the tables (after `teams.join_code` was added by `ADDED_COLUMNS`).
pub fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)
}

#[derive(Clone, Debug)]
pub struct UserRow {
    pub id: String,
    pub email: String,
    pub display_name: String,
    pub is_admin: bool,
    pub banned: bool,
}

impl UserRow {
    pub fn json(&self) -> Value {
        json!({"id": self.id, "email": self.email, "display_name": self.display_name, "is_admin": self.is_admin})
    }
}

const USER_COLS: &str = "u.id, u.email, u.display_name, u.is_admin, u.banned_at IS NOT NULL";

fn user_row(r: &rusqlite::Row) -> rusqlite::Result<UserRow> {
    Ok(UserRow {
        id: r.get(0)?,
        email: r.get(1)?,
        display_name: r.get(2)?,
        is_admin: r.get(3)?,
        banned: r.get(4)?,
    })
}

// ---- users ----

pub fn create_user(
    tx: &Transaction,
    email: &str,
    display_name: &str,
    password_hash: &str,
    is_admin: bool,
    now: i64,
) -> ApiResult<UserRow> {
    let id = format!("u{}", random_hex(6));
    match tx.execute(
        "INSERT INTO users(id, email, display_name, password_hash, is_admin, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, email, display_name, password_hash, is_admin, now],
    ) {
        Ok(_) => Ok(UserRow {
            id,
            email: email.to_string(),
            display_name: display_name.to_string(),
            is_admin,
            banned: false,
        }),
        Err(e) if is_unique_violation(&e) => Err(ApiError::conflict(
            "email_taken",
            "an account with this email already exists",
        )),
        Err(e) => Err(e.into()),
    }
}

/// The user with this email and their password hash.
pub fn user_for_login(tx: &Transaction, email: &str) -> ApiResult<Option<(UserRow, String)>> {
    Ok(tx
        .query_row(
            &format!("SELECT {USER_COLS}, u.password_hash FROM users u WHERE u.email = ?1"),
            [email],
            |r| Ok((user_row(r)?, r.get(5)?)),
        )
        .optional()?)
}

pub fn promote_admin(tx: &Transaction, user_id: &str) -> ApiResult<()> {
    tx.execute("UPDATE users SET is_admin = 1 WHERE id = ?1", [user_id])?;
    Ok(())
}

// ---- sessions and user tokens ----

pub fn create_session(
    tx: &Transaction,
    token_hash: &str,
    user_id: &str,
    now: i64,
    expires_at: i64,
) -> ApiResult<()> {
    tx.execute(
        "DELETE FROM sessions WHERE user_id = ?1 AND expires_at <= ?2",
        params![user_id, now],
    )?;
    tx.execute(
        "INSERT INTO sessions(token_hash, user_id, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
        params![token_hash, user_id, now, expires_at],
    )?;
    Ok(())
}

pub fn user_by_session(tx: &Transaction, token_hash: &str, now: i64) -> ApiResult<Option<UserRow>> {
    Ok(tx
        .query_row(
            &format!(
                "SELECT {USER_COLS} FROM sessions s JOIN users u ON u.id = s.user_id
                 WHERE s.token_hash = ?1 AND s.expires_at > ?2"
            ),
            params![token_hash, now],
            user_row,
        )
        .optional()?)
}

pub fn delete_session(tx: &Transaction, token_hash: &str) -> ApiResult<()> {
    tx.execute("DELETE FROM sessions WHERE token_hash = ?1", [token_hash])?;
    Ok(())
}

pub fn create_user_token(
    tx: &Transaction,
    token_hash: &str,
    user_id: &str,
    label: &str,
    now: i64,
) -> ApiResult<()> {
    tx.execute(
        "INSERT INTO user_tokens(token_hash, user_id, label, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![token_hash, user_id, label, now],
    )?;
    Ok(())
}

/// The user behind an `ibu` token; records that the token was used.
pub fn user_by_token(tx: &Transaction, token_hash: &str, now: i64) -> ApiResult<Option<UserRow>> {
    let user = tx
        .query_row(
            &format!(
                "SELECT {USER_COLS} FROM user_tokens t JOIN users u ON u.id = t.user_id WHERE t.token_hash = ?1"
            ),
            [token_hash],
            user_row,
        )
        .optional()?;
    if user.is_some() {
        tx.execute(
            "UPDATE user_tokens SET last_used = ?2 WHERE token_hash = ?1",
            params![token_hash, now],
        )?;
    }
    Ok(user)
}

pub fn delete_user_token(tx: &Transaction, token_hash: &str) -> ApiResult<()> {
    tx.execute(
        "DELETE FROM user_tokens WHERE token_hash = ?1",
        [token_hash],
    )?;
    Ok(())
}

// ---- sign-in throttling ----

/// Failed sign-ins for `email` since `since`: how many, and when the oldest was.
pub fn recent_failures(tx: &Transaction, email: &str, since: i64) -> ApiResult<(i64, Option<i64>)> {
    Ok(tx.query_row(
        "SELECT COUNT(*), MIN(at) FROM login_failures WHERE email = ?1 AND at > ?2",
        params![email, since],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?)
}

/// Record a failed sign-in and forget the ones older than `since`.
pub fn record_failure(tx: &Transaction, email: &str, now: i64, since: i64) -> ApiResult<()> {
    tx.execute(
        "DELETE FROM login_failures WHERE email = ?1 AND at <= ?2",
        params![email, since],
    )?;
    tx.execute(
        "INSERT INTO login_failures(email, at) VALUES (?1, ?2)",
        params![email, now],
    )?;
    Ok(())
}

pub fn clear_failures(tx: &Transaction, email: &str) -> ApiResult<()> {
    tx.execute("DELETE FROM login_failures WHERE email = ?1", [email])?;
    Ok(())
}

// ---- device codes ----

/// Store a new device code. `Ok(false)`: the user code is taken, draw another.
pub fn create_device_code(
    tx: &Transaction,
    device_code_hash: &str,
    user_code: &str,
    now: i64,
    expires_at: i64,
) -> ApiResult<bool> {
    tx.execute("DELETE FROM device_codes WHERE expires_at <= ?1", [now])?;
    match tx.execute(
        "INSERT INTO device_codes(device_code_hash, user_code, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
        params![device_code_hash, user_code, now, expires_at],
    ) {
        Ok(_) => Ok(true),
        Err(e) if is_unique_violation(&e) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// Approve a pending, unexpired code for `user_id`. `false`: no such code.
pub fn approve_device_code(
    tx: &Transaction,
    user_code: &str,
    user_id: &str,
    now: i64,
) -> ApiResult<bool> {
    Ok(tx.execute(
        "UPDATE device_codes SET user_id = ?2 WHERE user_code = ?1 AND user_id IS NULL AND expires_at > ?3",
        params![user_code, user_id, now],
    )? == 1)
}

pub enum DevicePoll {
    Unknown,
    Expired,
    /// Not approved yet. `too_soon`: the previous poll was less than the interval ago.
    Pending {
        too_soon: bool,
    },
    /// Approved by this user; the code is now spent.
    Approved(String),
}

/// One poll of a device code. An approved or expired code is deleted, so it yields one token at most.
pub fn poll_device_code(
    tx: &Transaction,
    device_code_hash: &str,
    now: i64,
    min_gap_ms: i64,
) -> ApiResult<DevicePoll> {
    let row: Option<(Option<String>, i64, Option<i64>)> = tx
        .query_row(
            "SELECT user_id, expires_at, last_poll FROM device_codes WHERE device_code_hash = ?1",
            [device_code_hash],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((user_id, expires_at, last_poll)) = row else {
        return Ok(DevicePoll::Unknown);
    };
    if expires_at <= now || user_id.is_some() {
        tx.execute(
            "DELETE FROM device_codes WHERE device_code_hash = ?1",
            [device_code_hash],
        )?;
    }
    if let Some(user_id) = user_id {
        return Ok(DevicePoll::Approved(user_id));
    }
    if expires_at <= now {
        return Ok(DevicePoll::Expired);
    }
    tx.execute(
        "UPDATE device_codes SET last_poll = ?2 WHERE device_code_hash = ?1",
        params![device_code_hash, now],
    )?;
    Ok(DevicePoll::Pending {
        too_soon: last_poll.is_some_and(|t| now - t < min_gap_ms),
    })
}

pub fn user_by_id(tx: &Transaction, user_id: &str) -> ApiResult<UserRow> {
    tx.query_row(
        &format!("SELECT {USER_COLS} FROM users u WHERE u.id = ?1"),
        [user_id],
        user_row,
    )
    .optional()?
    .ok_or_else(|| ApiError::not_found("no such user"))
}

/// Ban (`Some(now)`) or unban (`None`) a user, found by id or email. A ban also ends the user's sessions
/// and revokes their tokens; the auth layer refuses a banned account in any case.
pub fn set_banned(tx: &Transaction, id_or_email: &str, at: Option<i64>) -> ApiResult<UserRow> {
    let id: String = tx
        .query_row(
            "SELECT id FROM users WHERE id = ?1 OR email = ?1 ORDER BY id = ?1 DESC LIMIT 1",
            [id_or_email],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| ApiError::not_found(format!("no user {id_or_email:?}")))?;
    tx.execute("UPDATE users SET banned_at = ?2 WHERE id = ?1", params![id, at])?;
    if at.is_some() {
        tx.execute("DELETE FROM sessions WHERE user_id = ?1", [&id])?;
        tx.execute("DELETE FROM user_tokens WHERE user_id = ?1", [&id])?;
    }
    user_by_id(tx, &id)
}

// ---- team membership ----

pub fn team_of_user(tx: &Transaction, user_id: &str) -> ApiResult<Option<TeamRow>> {
    Ok(tx
        .query_row(
            "SELECT t.id, t.name FROM team_members m JOIN teams t ON t.id = m.team_id WHERE m.user_id = ?1",
            [user_id],
            |r| {
                Ok(TeamRow {
                    id: r.get(0)?,
                    name: r.get(1)?,
                })
            },
        )
        .optional()?)
}

pub fn team_by_join_code(tx: &Transaction, join_code: &str) -> ApiResult<Option<TeamRow>> {
    Ok(tx
        .query_row(
            "SELECT id, name FROM teams WHERE join_code = ?1",
            [join_code],
            |r| {
                Ok(TeamRow {
                    id: r.get(0)?,
                    name: r.get(1)?,
                })
            },
        )
        .optional()?)
}

/// The team's join code, drawing one if the team has none yet (teams created by the organisers).
pub fn join_code(tx: &Transaction, team_id: &str) -> ApiResult<String> {
    let code: Option<String> = tx.query_row(
        "SELECT join_code FROM teams WHERE id = ?1",
        [team_id],
        |r| r.get(0),
    )?;
    match code {
        Some(code) => Ok(code),
        None => rotate_join_code(tx, team_id),
    }
}

/// Replace the team's join code; the old one stops working.
pub fn rotate_join_code(tx: &Transaction, team_id: &str) -> ApiResult<String> {
    loop {
        let code = crate::accounts::new_join_code();
        match tx.execute(
            "UPDATE teams SET join_code = ?2 WHERE id = ?1",
            params![team_id, code],
        ) {
            Ok(_) => return Ok(code),
            Err(e) if is_unique_violation(&e) => continue,
            Err(e) => return Err(e.into()),
        }
    }
}

pub fn set_team_token(tx: &Transaction, team_id: &str, token_hash: &str) -> ApiResult<()> {
    tx.execute(
        "UPDATE teams SET token_hash = ?2 WHERE id = ?1",
        params![team_id, token_hash],
    )?;
    Ok(())
}

pub fn member_count(tx: &Transaction, team_id: &str) -> ApiResult<i64> {
    Ok(tx.query_row(
        "SELECT COUNT(*) FROM team_members WHERE team_id = ?1",
        [team_id],
        |r| r.get(0),
    )?)
}

pub fn add_member(
    tx: &Transaction,
    team_id: &str,
    user_id: &str,
    role: &str,
    now: i64,
) -> ApiResult<()> {
    match tx.execute(
        "INSERT INTO team_members(team_id, user_id, role, joined_at) VALUES (?1, ?2, ?3, ?4)",
        params![team_id, user_id, role, now],
    ) {
        Ok(_) => Ok(()),
        Err(e) if is_unique_violation(&e) => Err(ApiError::conflict(
            "already_in_team",
            "you are already in a team; leave it first",
        )),
        Err(e) => Err(e.into()),
    }
}

pub fn remove_member(tx: &Transaction, user_id: &str) -> ApiResult<()> {
    tx.execute("DELETE FROM team_members WHERE user_id = ?1", [user_id])?;
    Ok(())
}

/// Count one more team created by this user; returns how many they had created before.
pub fn note_team_created(tx: &Transaction, user_id: &str) -> ApiResult<i64> {
    let before: i64 = tx.query_row(
        "SELECT teams_created FROM users WHERE id = ?1",
        [user_id],
        |r| r.get(0),
    )?;
    tx.execute(
        "UPDATE users SET teams_created = teams_created + 1 WHERE id = ?1",
        [user_id],
    )?;
    Ok(before)
}

pub fn members(tx: &Transaction, team_id: &str) -> ApiResult<Vec<Value>> {
    let mut st = tx.prepare(
        "SELECT u.id, u.display_name, m.role FROM team_members m JOIN users u ON u.id = m.user_id
         WHERE m.team_id = ?1 ORDER BY m.joined_at, u.id",
    )?;
    let rows = st.query_map([team_id], |r| {
        Ok(
            json!({"user_id": r.get::<_, String>(0)?, "display_name": r.get::<_, String>(1)?,
                  "role": r.get::<_, String>(2)?}),
        )
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}
