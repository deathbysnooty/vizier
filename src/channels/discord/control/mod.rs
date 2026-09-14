//! The control room: every MLCI setting in one place, changeable from the web
//! panel without a restart.
//!
//! Settings keep the names they had as environment variables
//! (`VIZIER_FIGHT_CHANNEL` and so on). A value saved from the panel sits over the
//! environment; clearing it falls back to the environment again, so the `.env`
//! file stays the starting point and nothing is lost by switching this on.
//! Features read through [`var`] and its helpers at the moment they need a value,
//! which is what makes a change take effect at once.
//!
//! control.db also holds the panel's audit trail, its sign-in links and sessions.

pub mod autoreplies;
pub mod catalog;
pub mod help;
pub mod insights;
pub mod members;
pub mod memos;
pub mod remind;
pub mod profiles;
pub mod reminders;
pub mod scheduler;
pub mod web;

use std::collections::HashMap;
use std::sync::{LazyLock, OnceLock};

use base64::Engine;
use chrono::Utc;
use parking_lot::{Mutex, RwLock};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();
/// Every saved value, by key, so a read never touches the database.
static VALUES: LazyLock<RwLock<HashMap<String, String>>> = LazyLock::new(|| RwLock::new(HashMap::new()));

/// How long a sign-in link from `/panel` stays usable.
pub const LINK_SECS: i64 = 10 * 60;
/// How long a panel session lasts before signing in again.
pub const SESSION_SECS: i64 = 7 * 24 * 3600;

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS settings (
        key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_by INTEGER NOT NULL, updated_ts INTEGER NOT NULL);
    CREATE TABLE IF NOT EXISTS audit (
        id INTEGER PRIMARY KEY AUTOINCREMENT, ts INTEGER NOT NULL, user_id INTEGER NOT NULL,
        key TEXT NOT NULL, old TEXT, new TEXT);
    CREATE INDEX IF NOT EXISTS audit_ts ON audit (ts);
    CREATE TABLE IF NOT EXISTS links (
        token_hash TEXT PRIMARY KEY, user_id INTEGER NOT NULL, expires INTEGER NOT NULL, used INTEGER NOT NULL DEFAULT 0);
    CREATE TABLE IF NOT EXISTS sessions (
        token_hash TEXT PRIMARY KEY, user_id INTEGER NOT NULL, expires INTEGER NOT NULL, created INTEGER NOT NULL);
";

pub fn open(workspace: &str) -> anyhow::Result<()> {
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    std::fs::create_dir_all(&dir)?;
    let conn = Connection::open(dir.join("control.db"))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
    conn.execute_batch(SCHEMA)?;
    reminders::migrate(&conn)?;
    autoreplies::migrate(&conn)?;
    members::migrate(&conn)?;
    memos::migrate(&conn)?;
    profiles::migrate(&conn)?;
    // Reply and mention counts keep their own file; losing it must not stop the panel.
    if let Err(err) = insights::open(workspace) {
        tracing::warn!("insights: store not opened: {}", err);
    }
    let loaded = {
        let mut stmt = conn.prepare("SELECT key, value FROM settings")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        rows.collect::<rusqlite::Result<HashMap<_, _>>>()?
    };
    *VALUES.write() = loaded;
    let _ = DB.set(Mutex::new(conn));
    Ok(())
}

// --- reading ----------------------------------------------------------------

/// A setting: the panel's value if one is saved, otherwise the environment's.
/// Blank counts as not set.
pub fn var(key: &str) -> Option<String> {
    let saved = VALUES.read().get(key).cloned();
    saved.or_else(|| std::env::var(key).ok()).map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// Where a setting's current value comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    Panel,
    Env,
    Default,
}

pub fn origin(key: &str) -> Origin {
    if VALUES.read().contains_key(key) {
        Origin::Panel
    } else if std::env::var(key).is_ok_and(|v| !v.trim().is_empty()) {
        Origin::Env
    } else {
        Origin::Default
    }
}

pub fn id(key: &str) -> Option<u64> {
    var(key)?.parse().ok()
}

/// A comma-separated list of ids. Anything that isn't a number is skipped.
pub fn ids(key: &str) -> Vec<u64> {
    var(key).map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect()).unwrap_or_default()
}

pub fn number(key: &str, default: u64) -> u64 {
    var(key).and_then(|v| v.parse().ok()).unwrap_or(default)
}

pub fn float(key: &str, default: f64) -> f64 {
    var(key).and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// A switch: on/off, true/false, yes/no or 1/0; anything else is `default`.
pub fn on(key: &str, default: bool) -> bool {
    match var(key).map(|v| v.to_ascii_lowercase()).as_deref() {
        Some("on" | "true" | "yes" | "1") => true,
        Some("off" | "false" | "no" | "0") => false,
        _ => default,
    }
}

// --- writing ----------------------------------------------------------------

/// Saves a setting from the panel, or with `None` removes the panel's value so
/// the environment applies again. Every change is written to the audit trail.
pub fn set(key: &str, value: Option<&str>, by: u64) -> anyhow::Result<()> {
    let db = DB.get().ok_or_else(|| anyhow::anyhow!("control store not open"))?;
    let conn = db.lock();
    let old = VALUES.read().get(key).cloned();
    let now = Utc::now().timestamp();
    match value {
        Some(v) => {
            conn.execute(
                "INSERT INTO settings (key, value, updated_by, updated_ts) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_by = excluded.updated_by,
                 updated_ts = excluded.updated_ts",
                params![key, v, by as i64, now],
            )?;
        }
        None => {
            conn.execute("DELETE FROM settings WHERE key = ?1", params![key])?;
        }
    }
    conn.execute(
        "INSERT INTO audit (ts, user_id, key, old, new) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![now, by as i64, key, old, value],
    )?;
    let mut values = VALUES.write();
    match value {
        Some(v) => values.insert(key.to_string(), v.to_string()),
        None => values.remove(key),
    };
    Ok(())
}

/// Writes one change to the audit trail without saving a setting - for things
/// kept elsewhere, like the bot's AI settings (`agent:<field>`).
pub fn log_change(key: &str, old: Option<&str>, new: Option<&str>, by: u64) -> anyhow::Result<()> {
    let db = DB.get().ok_or_else(|| anyhow::anyhow!("control store not open"))?;
    db.lock().execute(
        "INSERT INTO audit (ts, user_id, key, old, new) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![Utc::now().timestamp(), by as i64, key, old, new],
    )?;
    Ok(())
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct AuditEntry {
    pub ts: i64,
    pub user_id: String,
    pub key: String,
    pub old: Option<String>,
    pub new: Option<String>,
}

/// The newest changes first.
pub fn audit(limit: usize) -> Vec<AuditEntry> {
    let Some(db) = DB.get() else {
        return Vec::new();
    };
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare("SELECT ts, user_id, key, old, new FROM audit ORDER BY id DESC LIMIT ?1") else {
        return Vec::new();
    };
    stmt.query_map(params![limit as i64], |r| {
        Ok(AuditEntry {
            ts: r.get(0)?,
            user_id: r.get::<_, i64>(1)?.to_string(),
            key: r.get(2)?,
            old: r.get(3)?,
            new: r.get(4)?,
        })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

// --- signing in ---------------------------------------------------------------

fn token() -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}

fn hash(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|b| format!("{:02x}", b)).collect()
}

/// A one-use sign-in token for an admin, good for [`LINK_SECS`]. Only its hash
/// is stored.
pub fn create_link(user: u64) -> Option<String> {
    let db = DB.get()?;
    let t = token();
    let now = Utc::now().timestamp();
    let conn = db.lock();
    let _ = conn.execute("DELETE FROM links WHERE expires < ?1", params![now]);
    conn.execute(
        "INSERT INTO links (token_hash, user_id, expires) VALUES (?1, ?2, ?3)",
        params![hash(&t), user as i64, now + LINK_SECS],
    )
    .ok()?;
    Some(t)
}

/// Spends a sign-in token and opens a session: returns the session token and
/// who it belongs to. A used, expired or unknown token gives nothing.
pub fn redeem_link(link: &str) -> Option<(String, u64)> {
    let db = DB.get()?;
    let now = Utc::now().timestamp();
    let conn = db.lock();
    let user: i64 = conn
        .query_row(
            "UPDATE links SET used = 1 WHERE token_hash = ?1 AND used = 0 AND expires >= ?2 RETURNING user_id",
            params![hash(link), now],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten()?;
    let session = token();
    conn.execute(
        "INSERT INTO sessions (token_hash, user_id, expires, created) VALUES (?1, ?2, ?3, ?4)",
        params![hash(&session), user, now + SESSION_SECS, now],
    )
    .ok()?;
    Some((session, user as u64))
}

/// Who a session token belongs to, while it lasts.
pub fn session_user(session: &str) -> Option<u64> {
    let db = DB.get()?;
    let now = Utc::now().timestamp();
    db.lock()
        .query_row(
            "SELECT user_id FROM sessions WHERE token_hash = ?1 AND expires >= ?2",
            params![hash(session), now],
            |r| r.get::<_, i64>(0),
        )
        .optional()
        .ok()
        .flatten()
        .map(|u| u as u64)
}

/// When a session runs out, as a Unix timestamp.
pub fn session_expires(session: &str) -> Option<i64> {
    let db = DB.get()?;
    db.lock()
        .query_row("SELECT expires FROM sessions WHERE token_hash = ?1", params![hash(session)], |r| r.get(0))
        .optional()
        .ok()
        .flatten()
}

pub fn end_session(session: &str) {
    if let Some(db) = DB.get() {
        let _ = db.lock().execute("DELETE FROM sessions WHERE token_hash = ?1", params![hash(session)]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switches_read_the_usual_spellings() {
        VALUES.write().insert("TEST_SWITCH_A".into(), "Off".into());
        VALUES.write().insert("TEST_SWITCH_B".into(), "yes".into());
        VALUES.write().insert("TEST_SWITCH_C".into(), "maybe".into());
        assert!(!on("TEST_SWITCH_A", true));
        assert!(on("TEST_SWITCH_B", false));
        assert!(on("TEST_SWITCH_C", true));
        assert!(!on("TEST_SWITCH_MISSING", false));
    }

    #[test]
    fn lists_and_blanks() {
        VALUES.write().insert("TEST_IDS".into(), " 12, x ,34,".into());
        VALUES.write().insert("TEST_BLANK".into(), "   ".into());
        assert_eq!(ids("TEST_IDS"), vec![12, 34]);
        assert_eq!(var("TEST_BLANK"), None);
        assert_eq!(number("TEST_BLANK", 7), 7);
    }

    #[test]
    fn tokens_are_unique_and_hashes_are_stable() {
        let (a, b) = (token(), token());
        assert_ne!(a, b);
        assert_eq!(a.len(), 43);
        assert_eq!(hash("abc"), hash("abc"));
        assert_ne!(hash("abc"), hash("abd"));
    }
}
