//! Member notes' own store, `.runtime/notes.db`: the stored "what they're like"
//! bullets, who has opted out, the audit of mods looking people up, and a log
//! of every build run with the tokens it spent.
//!
//! Nothing here calls a model or Discord; `notes.rs` and `notes_build.rs` do.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

pub const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS notes (
        user_id INTEGER PRIMARY KEY, bullets_json TEXT NOT NULL, built_ts INTEGER NOT NULL,
        input_tokens INTEGER NOT NULL DEFAULT 0, output_tokens INTEGER NOT NULL DEFAULT 0,
        model TEXT NOT NULL DEFAULT '', messages_used INTEGER NOT NULL DEFAULT 0,
        rejected INTEGER NOT NULL DEFAULT 0, built_by INTEGER NOT NULL DEFAULT 0);
    CREATE TABLE IF NOT EXISTS optouts (user_id INTEGER PRIMARY KEY, ts INTEGER NOT NULL);
    -- The last time a member was looked at by a build, whatever came of it, so
    -- someone with too little to go on is not re-read every run.
    CREATE TABLE IF NOT EXISTS attempts (
        user_id INTEGER PRIMARY KEY, ts INTEGER NOT NULL, outcome TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS lookups (
        id INTEGER PRIMARY KEY AUTOINCREMENT, ts INTEGER NOT NULL, asker INTEGER NOT NULL,
        target INTEGER NOT NULL, via TEXT NOT NULL);
    CREATE INDEX IF NOT EXISTS lookups_target ON lookups (target, ts);
    CREATE TABLE IF NOT EXISTS runs (
        id INTEGER PRIMARY KEY AUTOINCREMENT, ts INTEGER NOT NULL, kind TEXT NOT NULL,
        asked INTEGER NOT NULL, built INTEGER NOT NULL, skipped INTEGER NOT NULL, failed INTEGER NOT NULL,
        rejected INTEGER NOT NULL, input_tokens INTEGER NOT NULL, output_tokens INTEGER NOT NULL, waiting INTEGER NOT NULL);
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);";

pub fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)
}

pub fn open(workspace: &str) -> anyhow::Result<()> {
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    std::fs::create_dir_all(&dir)?;
    let conn = Connection::open(dir.join("notes.db"))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
    init(&conn)?;
    let _ = DB.set(Mutex::new(conn));
    Ok(())
}

pub fn db() -> Option<&'static Mutex<Connection>> {
    DB.get()
}

// --- notes ---------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Note {
    pub user_id: u64,
    pub bullets: Vec<String>,
    pub built_ts: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub model: String,
    pub messages_used: i64,
    /// Bullets the model wrote that the filter threw away.
    pub rejected: i64,
    /// 0 for the scheduled build, otherwise the mod who pressed rebuild.
    pub built_by: u64,
}

fn note_row(r: &rusqlite::Row) -> rusqlite::Result<Note> {
    let bullets: String = r.get(1)?;
    Ok(Note {
        user_id: r.get::<_, i64>(0)? as u64,
        bullets: serde_json::from_str(&bullets).unwrap_or_default(),
        built_ts: r.get(2)?,
        input_tokens: r.get(3)?,
        output_tokens: r.get(4)?,
        model: r.get(5)?,
        messages_used: r.get(6)?,
        rejected: r.get(7)?,
        built_by: r.get::<_, i64>(8)? as u64,
    })
}

const NOTE_COLUMNS: &str = "user_id, bullets_json, built_ts, input_tokens, output_tokens, model, messages_used, rejected, built_by";

pub fn get(conn: &Connection, user: u64) -> Option<Note> {
    conn.query_row(&format!("SELECT {} FROM notes WHERE user_id = ?1", NOTE_COLUMNS), params![user as i64], note_row)
        .optional()
        .ok()
        .flatten()
}

/// When each member's notes were last built.
pub fn built_at(conn: &Connection) -> HashMap<u64, i64> {
    let Ok(mut stmt) = conn.prepare("SELECT user_id, built_ts FROM notes") else { return HashMap::new() };
    stmt.query_map([], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)?)))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

pub fn count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM notes", [], |r| r.get(0)).unwrap_or(0)
}

/// Saves a build. Refuses someone who has opted out, whatever the caller thought.
pub fn save(conn: &Connection, note: &Note) -> rusqlite::Result<bool> {
    if opted_out(conn, note.user_id) {
        return Ok(false);
    }
    conn.execute(
        &format!(
            "INSERT INTO notes ({}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(user_id) DO UPDATE SET bullets_json = excluded.bullets_json, built_ts = excluded.built_ts,
             input_tokens = excluded.input_tokens, output_tokens = excluded.output_tokens, model = excluded.model,
             messages_used = excluded.messages_used, rejected = excluded.rejected, built_by = excluded.built_by",
            NOTE_COLUMNS
        ),
        params![
            note.user_id as i64,
            serde_json::to_string(&note.bullets).unwrap_or_else(|_| "[]".into()),
            note.built_ts,
            note.input_tokens,
            note.output_tokens,
            note.model,
            note.messages_used,
            note.rejected,
            note.built_by as i64
        ],
    )?;
    Ok(true)
}

/// Clears a member's notes. True when there were some.
pub fn delete(conn: &Connection, user: u64) -> rusqlite::Result<bool> {
    Ok(conn.execute("DELETE FROM notes WHERE user_id = ?1", params![user as i64])? > 0)
}

// --- opting out ----------------------------------------------------------------------------

pub fn opted_out(conn: &Connection, user: u64) -> bool {
    conn.query_row("SELECT 1 FROM optouts WHERE user_id = ?1", params![user as i64], |_| Ok(()))
        .optional()
        .ok()
        .flatten()
        .is_some()
}

pub fn optouts(conn: &Connection) -> HashSet<u64> {
    let Ok(mut stmt) = conn.prepare("SELECT user_id FROM optouts") else { return HashSet::new() };
    stmt.query_map([], |r| r.get::<_, i64>(0)).map(|rows| rows.flatten().map(|u| u as u64).collect()).unwrap_or_default()
}

/// Opts a member out: their notes are deleted and the opt-out recorded, in one
/// transaction so a build can't slip a note in between.
pub fn opt_out(conn: &mut Connection, user: u64, now: i64) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM notes WHERE user_id = ?1", params![user as i64])?;
    tx.execute("DELETE FROM attempts WHERE user_id = ?1", params![user as i64])?;
    tx.execute("INSERT OR REPLACE INTO optouts (user_id, ts) VALUES (?1, ?2)", params![user as i64, now])?;
    tx.commit()
}

pub fn opt_in(conn: &Connection, user: u64) -> rusqlite::Result<bool> {
    Ok(conn.execute("DELETE FROM optouts WHERE user_id = ?1", params![user as i64])? > 0)
}

// --- attempts ------------------------------------------------------------------------------

pub fn note_attempt(conn: &Connection, user: u64, now: i64, outcome: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO attempts (user_id, ts, outcome) VALUES (?1, ?2, ?3)
         ON CONFLICT(user_id) DO UPDATE SET ts = excluded.ts, outcome = excluded.outcome",
        params![user as i64, now, outcome],
    )?;
    Ok(())
}

/// The last attempt at each member that did not end in a note: (when, outcome).
pub fn attempts(conn: &Connection) -> HashMap<u64, (i64, String)> {
    let Ok(mut stmt) = conn.prepare("SELECT user_id, ts, outcome FROM attempts") else { return HashMap::new() };
    stmt.query_map([], |r| Ok((r.get::<_, i64>(0)? as u64, (r.get::<_, i64>(1)?, r.get::<_, String>(2)?))))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

// --- lookups -------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Lookup {
    pub ts: i64,
    pub asker: u64,
    pub target: u64,
    pub via: String,
}

pub fn record_lookup(conn: &Connection, asker: u64, target: u64, via: &str, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO lookups (ts, asker, target, via) VALUES (?1, ?2, ?3, ?4)",
        params![now, asker as i64, target as i64, via],
    )?;
    Ok(())
}

/// The newest lookups, of one member or of anyone.
pub fn lookups(conn: &Connection, target: Option<u64>, limit: usize) -> Vec<Lookup> {
    let sql = "SELECT ts, asker, target, via FROM lookups WHERE (?1 IS NULL OR target = ?1) ORDER BY id DESC LIMIT ?2";
    let Ok(mut stmt) = conn.prepare(sql) else { return Vec::new() };
    stmt.query_map(params![target.map(|t| t as i64), limit as i64], |r| {
        Ok(Lookup { ts: r.get(0)?, asker: r.get::<_, i64>(1)? as u64, target: r.get::<_, i64>(2)? as u64, via: r.get(3)? })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

// --- runs ----------------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Run {
    pub ts: i64,
    /// "first", "weekly" or "manual".
    pub kind: String,
    /// Model calls made.
    pub asked: i64,
    pub built: i64,
    /// Looked at but not sent to the model (too little to go on).
    pub skipped: i64,
    pub failed: i64,
    /// Bullets thrown away by the filter.
    pub rejected: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    /// Members who qualified but were left for a later run by the cap.
    pub waiting: i64,
}

pub fn record_run(conn: &Connection, run: &Run) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO runs (ts, kind, asked, built, skipped, failed, rejected, input_tokens, output_tokens, waiting)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![run.ts, run.kind, run.asked, run.built, run.skipped, run.failed, run.rejected, run.input_tokens, run.output_tokens, run.waiting],
    )?;
    Ok(())
}

pub fn runs(conn: &Connection, limit: usize) -> Vec<Run> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT ts, kind, asked, built, skipped, failed, rejected, input_tokens, output_tokens, waiting FROM runs ORDER BY id DESC LIMIT ?1",
    ) else {
        return Vec::new();
    };
    stmt.query_map(params![limit as i64], |r| {
        Ok(Run {
            ts: r.get(0)?,
            kind: r.get(1)?,
            asked: r.get(2)?,
            built: r.get(3)?,
            skipped: r.get(4)?,
            failed: r.get(5)?,
            rejected: r.get(6)?,
            input_tokens: r.get(7)?,
            output_tokens: r.get(8)?,
            waiting: r.get(9)?,
        })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

/// Every token the builds have spent, in and out.
pub fn tokens_spent(conn: &Connection) -> (i64, i64) {
    conn.query_row("SELECT COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0) FROM runs", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap_or((0, 0))
}

pub fn meta_get(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0)).optional().ok().flatten()
}

pub fn meta_set(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

#[cfg(test)]
pub fn memory() -> Connection {
    let conn = Connection::open_in_memory().expect("memory db");
    init(&conn).expect("schema");
    conn
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(user: u64) -> Note {
        Note {
            user_id: user,
            bullets: vec!["Loves cricket banter".into()],
            built_ts: 100,
            input_tokens: 900,
            output_tokens: 80,
            model: "m".into(),
            messages_used: 120,
            rejected: 0,
            built_by: 0,
        }
    }

    #[test]
    fn opting_out_deletes_the_notes_and_blocks_new_ones() {
        let mut conn = memory();
        assert!(save(&conn, &note(7)).unwrap());
        assert!(get(&conn, 7).is_some());
        opt_out(&mut conn, 7, 200).unwrap();
        assert!(get(&conn, 7).is_none(), "opting out deletes what was there");
        assert!(opted_out(&conn, 7));
        assert!(!save(&conn, &note(7)).unwrap(), "a build can't write for someone who opted out");
        assert!(get(&conn, 7).is_none());
        assert!(opt_in(&conn, 7).unwrap());
        assert!(!opted_out(&conn, 7));
        assert!(save(&conn, &note(7)).unwrap());
    }

    #[test]
    fn lookups_and_runs_are_kept_newest_first() {
        let conn = memory();
        record_lookup(&conn, 1, 2, "command", 10).unwrap();
        record_lookup(&conn, 1, 3, "chat", 11).unwrap();
        assert_eq!(lookups(&conn, None, 10).len(), 2);
        let of_two = lookups(&conn, Some(2), 10);
        assert_eq!(of_two, vec![Lookup { ts: 10, asker: 1, target: 2, via: "command".into() }]);
        record_run(&conn, &Run { ts: 5, kind: "first".into(), asked: 2, input_tokens: 3000, output_tokens: 200, ..Default::default() }).unwrap();
        record_run(&conn, &Run { ts: 6, kind: "weekly".into(), asked: 1, input_tokens: 1000, output_tokens: 100, ..Default::default() }).unwrap();
        assert_eq!(runs(&conn, 10)[0].kind, "weekly");
        assert_eq!(tokens_spent(&conn), (4000, 300));
    }
}
