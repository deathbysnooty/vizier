//! Reminders: messages the bot posts on a schedule, made and edited from the
//! panel. This file is the store; the scheduler that posts them lives beside it.

use chrono::Utc;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::DB;

/// When a reminder goes out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Schedule {
    /// Every so many minutes, lined up on the clock (every 60 = on the hour).
    Every { minutes: u32 },
    /// At these India times each day, "HH:MM".
    Daily { times: Vec<String> },
}

/// Which line goes out next.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    /// Each line in turn.
    #[default]
    Rotate,
    Random,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reminder {
    /// 0 for one not saved yet.
    #[serde(default)]
    pub id: i64,
    pub name: String,
    pub enabled: bool,
    /// Discord ids travel as strings: they are too big for JavaScript numbers.
    pub channel_id: String,
    /// The messages. Placeholders: `{name}`, `{mention}` (the member in
    /// `user_id`), `{hours}` and `{days}` (since `since`).
    pub lines: Vec<String>,
    #[serde(default)]
    pub order: Order,
    pub schedule: Schedule,
    /// Only post between these India times, "HH:MM"; both empty means any time.
    #[serde(default)]
    pub active_from: String,
    #[serde(default)]
    pub active_to: String,
    /// The member the reminder is about, if any.
    #[serde(default)]
    pub user_id: String,
    /// Shown as `{name}`.
    #[serde(default)]
    pub user_name: String,
    /// RFC 3339 moment `{hours}` and `{days}` count from.
    #[serde(default)]
    pub since: String,
    /// Stop (switch off) once `user_id` is back in the server, posting `welcome_line`.
    #[serde(default)]
    pub stop_when_back: bool,
    #[serde(default)]
    pub welcome_line: String,
    /// RFC 3339 end, after which it stops by itself; empty for never.
    #[serde(default)]
    pub ends: String,
    /// Kept by the scheduler.
    #[serde(default)]
    pub last_sent: i64,
    #[serde(default)]
    pub sent_count: i64,
}

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS reminders (
        id INTEGER PRIMARY KEY AUTOINCREMENT, body TEXT NOT NULL, updated_by INTEGER NOT NULL, updated_ts INTEGER NOT NULL);
";

pub(super) fn migrate(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)
}

pub fn list() -> Vec<Reminder> {
    let Some(db) = DB.get() else {
        return Vec::new();
    };
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare("SELECT id, body FROM reminders ORDER BY id") else {
        return Vec::new();
    };
    stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .map(|rows| {
            rows.flatten()
                .filter_map(|(id, body)| serde_json::from_str::<Reminder>(&body).ok().map(|r| Reminder { id, ..r }))
                .collect()
        })
        .unwrap_or_default()
}

pub fn get(id: i64) -> Option<Reminder> {
    let db = DB.get()?;
    let body: String = db
        .lock()
        .query_row("SELECT body FROM reminders WHERE id = ?1", params![id], |r| r.get(0))
        .optional()
        .ok()
        .flatten()?;
    serde_json::from_str::<Reminder>(&body).ok().map(|r| Reminder { id, ..r })
}

/// Saves a reminder, new when its id is 0, and returns its id. The change goes
/// to the audit trail under `reminder:<id>`.
pub fn save(reminder: &Reminder, by: u64) -> anyhow::Result<i64> {
    let db = DB.get().ok_or_else(|| anyhow::anyhow!("control store not open"))?;
    let conn = db.lock();
    let now = Utc::now().timestamp();
    let body = serde_json::to_string(reminder)?;
    let (id, old) = if reminder.id == 0 {
        conn.execute(
            "INSERT INTO reminders (body, updated_by, updated_ts) VALUES (?1, ?2, ?3)",
            params![body, by as i64, now],
        )?;
        (conn.last_insert_rowid(), None)
    } else {
        let old: Option<String> = conn
            .query_row("SELECT body FROM reminders WHERE id = ?1", params![reminder.id], |r| r.get(0))
            .optional()?;
        if old.is_none() {
            anyhow::bail!("no reminder {}", reminder.id);
        }
        conn.execute(
            "UPDATE reminders SET body = ?1, updated_by = ?2, updated_ts = ?3 WHERE id = ?4",
            params![body, by as i64, now, reminder.id],
        )?;
        (reminder.id, old)
    };
    if by != 0 {
        conn.execute(
            "INSERT INTO audit (ts, user_id, key, old, new) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![now, by as i64, format!("reminder:{}", id), old, body],
        )?;
    }
    Ok(id)
}

pub fn delete(id: i64, by: u64) -> anyhow::Result<()> {
    let db = DB.get().ok_or_else(|| anyhow::anyhow!("control store not open"))?;
    let conn = db.lock();
    let old: Option<String> =
        conn.query_row("SELECT body FROM reminders WHERE id = ?1", params![id], |r| r.get(0)).optional()?;
    conn.execute("DELETE FROM reminders WHERE id = ?1", params![id])?;
    conn.execute(
        "INSERT INTO audit (ts, user_id, key, old, new) VALUES (?1, ?2, ?3, ?4, NULL)",
        params![Utc::now().timestamp(), by as i64, format!("reminder:{}", id), old],
    )?;
    Ok(())
}
