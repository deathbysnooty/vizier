//! What the Kalesh page keeps: every fight the live detector called, and every
//! summary a moderator asked for. Its own small file in the runtime directory,
//! `kalesh.db`, so losing it never touches the message log or the panel's
//! settings.
//!
//! Neither table holds message text. A detection is where and when, who, and
//! which message ids; the text stays in the message log, which already decides
//! what is kept and for how long. A summary holds the model's paraphrase and
//! what it cost, so asking for the same stretch twice never pays twice.

use std::path::Path;
use std::sync::OnceLock;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS detections (
        id INTEGER PRIMARY KEY AUTOINCREMENT, channel_id INTEGER NOT NULL,
        start_ms INTEGER NOT NULL, end_ms INTEGER NOT NULL,
        participants_json TEXT NOT NULL, message_ids_json TEXT NOT NULL, message_count INTEGER NOT NULL,
        line TEXT NOT NULL DEFAULT '', created_ts INTEGER NOT NULL);
    CREATE INDEX IF NOT EXISTS detections_created ON detections (created_ts);
    CREATE TABLE IF NOT EXISTS summaries (
        id INTEGER PRIMARY KEY AUTOINCREMENT, stretch_key TEXT NOT NULL, channel_id INTEGER NOT NULL,
        a_id INTEGER NOT NULL, b_id INTEGER NOT NULL, start_ms INTEGER NOT NULL, end_ms INTEGER NOT NULL,
        detection_id INTEGER, message_ids_json TEXT NOT NULL, message_count INTEGER NOT NULL,
        sent_count INTEGER NOT NULL, trimmed INTEGER NOT NULL, run_by INTEGER NOT NULL, run_ts INTEGER NOT NULL,
        model TEXT NOT NULL, input_tokens INTEGER NOT NULL, output_tokens INTEGER NOT NULL,
        summary_json TEXT, raw TEXT NOT NULL);
    CREATE INDEX IF NOT EXISTS summaries_key ON summaries (stretch_key);
    CREATE INDEX IF NOT EXISTS summaries_run ON summaries (run_ts);
    CREATE INDEX IF NOT EXISTS summaries_place ON summaries (channel_id, start_ms);
";

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

/// The shared connection. Hold the lock only for a query, never across an `.await`.
pub fn db() -> Option<&'static Mutex<Connection>> {
    DB.get()
}

fn prepare(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)?;
    // Added once summaries could be about more than two people, or a whole period.
    let have: Vec<String> = {
        let mut stmt = conn.prepare("PRAGMA table_info(summaries)")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for (name, kind) in [("people_json", "TEXT"), ("scope", "TEXT NOT NULL DEFAULT 'stretch'")] {
        if !have.iter().any(|h| h == name) {
            conn.execute_batch(&format!("ALTER TABLE summaries ADD COLUMN {} {}", name, kind))?;
        }
    }
    Ok(())
}

/// A summary of one stretch in one channel.
pub const SCOPE_STRETCH: &str = "stretch";
/// A summary of every stretch of a period, all channels together (`channel_id` 0).
pub const SCOPE_PERIOD: &str = "period";

/// Opens (or makes) `<workspace>/.runtime/kalesh.db`. Once per process.
pub fn open(workspace: &str) -> anyhow::Result<()> {
    if DB.get().is_some() {
        return Ok(());
    }
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    open_at(&dir.join("kalesh.db"))
}

pub fn open_at(path: &Path) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
    prepare(&conn)?;
    let _ = DB.set(Mutex::new(conn));
    Ok(())
}

/// An in-memory store, for tests that don't care where it lives.
pub fn open_memory() -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    prepare(&conn)?;
    Ok(conn)
}

// --- detections --------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Participant {
    pub id: u64,
    pub name: String,
    pub messages: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NewDetection {
    pub channel_id: u64,
    pub start_ms: i64,
    pub end_ms: i64,
    /// Busiest first.
    pub participants: Vec<Participant>,
    pub message_ids: Vec<u64>,
    pub line: String,
    pub created_ts: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Detection {
    pub id: i64,
    pub channel_id: u64,
    pub start_ms: i64,
    pub end_ms: i64,
    pub participants: Vec<Participant>,
    pub message_ids: Vec<u64>,
    pub message_count: usize,
    pub line: String,
    pub created_ts: i64,
}

pub fn add_detection(conn: &Connection, d: &NewDetection) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO detections (channel_id, start_ms, end_ms, participants_json, message_ids_json, message_count, line, created_ts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            d.channel_id as i64,
            d.start_ms,
            d.end_ms,
            serde_json::to_string(&d.participants).unwrap_or_else(|_| "[]".into()),
            serde_json::to_string(&d.message_ids.iter().map(|i| i.to_string()).collect::<Vec<_>>()).unwrap_or_else(|_| "[]".into()),
            d.message_ids.len() as i64,
            d.line,
            d.created_ts,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

fn detection_row(r: &rusqlite::Row) -> rusqlite::Result<Detection> {
    let ids: Vec<String> = serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or_default();
    Ok(Detection {
        id: r.get(0)?,
        channel_id: r.get::<_, i64>(1)? as u64,
        start_ms: r.get(2)?,
        end_ms: r.get(3)?,
        participants: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default(),
        message_ids: ids.iter().filter_map(|i| i.parse().ok()).collect(),
        message_count: r.get::<_, i64>(6)? as usize,
        line: r.get(7)?,
        created_ts: r.get(8)?,
    })
}

const DETECTION_COLUMNS: &str = "id, channel_id, start_ms, end_ms, participants_json, message_ids_json, message_count, line, created_ts";

/// Newest first.
pub fn detections(conn: &Connection, limit: usize) -> rusqlite::Result<Vec<Detection>> {
    let mut stmt = conn.prepare_cached(&format!("SELECT {} FROM detections ORDER BY start_ms DESC, id DESC LIMIT ?1", DETECTION_COLUMNS))?;
    let rows = stmt.query_map(params![limit as i64], detection_row)?;
    rows.collect()
}

pub fn detection(conn: &Connection, id: i64) -> rusqlite::Result<Option<Detection>> {
    conn.query_row(&format!("SELECT {} FROM detections WHERE id = ?1", DETECTION_COLUMNS), params![id], detection_row).optional()
}

// --- summaries ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct NewSummary {
    pub stretch_key: String,
    /// 0 for a whole period.
    pub channel_id: u64,
    /// The first two of `people`, kept for summaries written before there could be more.
    pub a_id: u64,
    pub b_id: u64,
    /// Everyone it is about, in the order they were chosen.
    pub people: Vec<u64>,
    /// [`SCOPE_STRETCH`] or [`SCOPE_PERIOD`].
    pub scope: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub detection_id: Option<i64>,
    /// The stretch's message ids in order: `[#n]` in the summary is `message_ids[n - 1]`.
    pub message_ids: Vec<u64>,
    pub sent_count: usize,
    pub trimmed: bool,
    pub run_by: u64,
    pub run_ts: i64,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// The model's answer, read into the page's shape; none when it wasn't JSON.
    pub summary: Option<serde_json::Value>,
    pub raw: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub id: i64,
    pub new: NewSummary,
}

pub fn add_summary(conn: &Connection, s: &NewSummary) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO summaries (stretch_key, channel_id, a_id, b_id, start_ms, end_ms, detection_id, message_ids_json, message_count,
             sent_count, trimmed, run_by, run_ts, model, input_tokens, output_tokens, summary_json, raw, people_json, scope)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
        params![
            s.stretch_key,
            s.channel_id as i64,
            s.a_id as i64,
            s.b_id as i64,
            s.start_ms,
            s.end_ms,
            s.detection_id,
            serde_json::to_string(&s.message_ids.iter().map(|i| i.to_string()).collect::<Vec<_>>()).unwrap_or_else(|_| "[]".into()),
            s.message_ids.len() as i64,
            s.sent_count as i64,
            s.trimmed as i64,
            s.run_by as i64,
            s.run_ts,
            s.model,
            s.input_tokens as i64,
            s.output_tokens as i64,
            s.summary.as_ref().map(|v| v.to_string()),
            s.raw,
            serde_json::to_string(&s.people.iter().map(|i| i.to_string()).collect::<Vec<_>>()).unwrap_or_else(|_| "[]".into()),
            s.scope,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

const SUMMARY_COLUMNS: &str = "id, stretch_key, channel_id, a_id, b_id, start_ms, end_ms, detection_id, message_ids_json, \
                               sent_count, trimmed, run_by, run_ts, model, input_tokens, output_tokens, summary_json, raw, people_json, scope";

fn summary_row(r: &rusqlite::Row) -> rusqlite::Result<Summary> {
    let ids: Vec<String> = serde_json::from_str(&r.get::<_, String>(8)?).unwrap_or_default();
    let (a, b) = (r.get::<_, i64>(3)? as u64, r.get::<_, i64>(4)? as u64);
    let people: Vec<u64> = r
        .get::<_, Option<String>>(18)?
        .and_then(|j| serde_json::from_str::<Vec<String>>(&j).ok())
        .map(|v| v.iter().filter_map(|i| i.parse().ok()).collect::<Vec<u64>>())
        .filter(|v| v.len() >= 2)
        .unwrap_or_else(|| vec![a, b]);
    Ok(Summary {
        id: r.get(0)?,
        new: NewSummary {
            stretch_key: r.get(1)?,
            channel_id: r.get::<_, i64>(2)? as u64,
            a_id: a,
            b_id: b,
            people,
            scope: r.get(19)?,
            start_ms: r.get(5)?,
            end_ms: r.get(6)?,
            detection_id: r.get(7)?,
            message_ids: ids.iter().filter_map(|i| i.parse().ok()).collect(),
            sent_count: r.get::<_, i64>(9)? as usize,
            trimmed: r.get::<_, i64>(10)? != 0,
            run_by: r.get::<_, i64>(11)? as u64,
            run_ts: r.get(12)?,
            model: r.get(13)?,
            input_tokens: r.get::<_, i64>(14)? as u64,
            output_tokens: r.get::<_, i64>(15)? as u64,
            summary: r.get::<_, Option<String>>(16)?.and_then(|s| serde_json::from_str(&s).ok()),
            raw: r.get(17)?,
        },
    })
}

/// The stored summary of exactly this stretch (same pair, channel and messages), if any.
pub fn summary_for(conn: &Connection, key: &str) -> rusqlite::Result<Option<Summary>> {
    conn.query_row(&format!("SELECT {} FROM summaries WHERE stretch_key = ?1 ORDER BY id DESC LIMIT 1", SUMMARY_COLUMNS), params![key], summary_row)
        .optional()
}

pub fn summary(conn: &Connection, id: i64) -> rusqlite::Result<Option<Summary>> {
    conn.query_row(&format!("SELECT {} FROM summaries WHERE id = ?1", SUMMARY_COLUMNS), params![id], summary_row).optional()
}

fn same_people(x: &[u64], y: &[u64]) -> bool {
    let (mut x, mut y) = (x.to_vec(), y.to_vec());
    x.sort_unstable();
    x.dedup();
    y.sort_unstable();
    y.dedup();
    x == y
}

/// Every stretch summary about exactly these people in this channel whose
/// stretch overlaps `start..=end`: the same fight summarised when it was
/// shorter, say. Newest first.
pub fn summaries_near(conn: &Connection, channel: u64, people: &[u64], start_ms: i64, end_ms: i64) -> rusqlite::Result<Vec<Summary>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {} FROM summaries WHERE channel_id = ?1 AND scope = 'stretch' AND start_ms <= ?3 AND end_ms >= ?2 ORDER BY id DESC LIMIT 200",
        SUMMARY_COLUMNS
    ))?;
    let rows = stmt.query_map(params![channel as i64, start_ms, end_ms], summary_row)?;
    let all: Vec<Summary> = rows.collect::<rusqlite::Result<_>>()?;
    Ok(all.into_iter().filter(|s| same_people(&s.new.people, people)).take(20).collect())
}

/// Every whole-period summary about exactly these people whose period overlaps
/// `start..=end`. Newest first.
pub fn period_summaries(conn: &Connection, people: &[u64], start_ms: i64, end_ms: i64) -> rusqlite::Result<Vec<Summary>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {} FROM summaries WHERE scope = 'period' AND start_ms <= ?2 AND end_ms >= ?1 ORDER BY id DESC LIMIT 200",
        SUMMARY_COLUMNS
    ))?;
    let rows = stmt.query_map(params![start_ms, end_ms], summary_row)?;
    let all: Vec<Summary> = rows.collect::<rusqlite::Result<_>>()?;
    Ok(all.into_iter().filter(|s| same_people(&s.new.people, people)).take(20).collect())
}

/// The newest summaries, for the page's list.
pub fn recent_summaries(conn: &Connection, limit: usize) -> rusqlite::Result<Vec<Summary>> {
    let mut stmt = conn.prepare_cached(&format!("SELECT {} FROM summaries ORDER BY run_ts DESC, id DESC LIMIT ?1", SUMMARY_COLUMNS))?;
    let rows = stmt.query_map(params![limit as i64], summary_row)?;
    rows.collect()
}

/// Which detections have a summary, of those asked about.
pub fn summarised_detections(conn: &Connection, ids: &[i64]) -> rusqlite::Result<Vec<i64>> {
    let mut out = Vec::new();
    let mut stmt = conn.prepare_cached("SELECT 1 FROM summaries WHERE detection_id = ?1 LIMIT 1")?;
    for id in ids {
        if stmt.exists(params![id])? {
            out.push(*id);
        }
    }
    Ok(out)
}

/// Which of these stretch keys have a summary.
pub fn summarised_keys(conn: &Connection, keys: &[String]) -> rusqlite::Result<Vec<String>> {
    let mut out = Vec::new();
    let mut stmt = conn.prepare_cached("SELECT 1 FROM summaries WHERE stretch_key = ?1 LIMIT 1")?;
    for key in keys {
        if stmt.exists(params![key])? {
            out.push(key.clone());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detections_and_summaries_round_trip() {
        let conn = open_memory().unwrap();
        let d = NewDetection {
            channel_id: 23,
            start_ms: 1_000,
            end_ms: 91_000,
            participants: vec![Participant { id: 1, name: "A".into(), messages: 6 }, Participant { id: 2, name: "B".into(), messages: 5 }],
            message_ids: vec![11, 12, 13],
            line: "popcorn".into(),
            created_ts: 92,
        };
        let id = add_detection(&conn, &d).unwrap();
        let back = detection(&conn, id).unwrap().unwrap();
        assert_eq!((back.channel_id, back.message_ids.clone(), back.message_count, back.participants.len()), (23, vec![11, 12, 13], 3, 2));
        assert_eq!(detections(&conn, 10).unwrap().len(), 1);

        let s = NewSummary {
            stretch_key: "k".into(),
            channel_id: 23,
            a_id: 2,
            b_id: 1,
            people: vec![2, 1],
            scope: SCOPE_STRETCH.into(),
            start_ms: 0,
            end_ms: 100_000,
            detection_id: Some(id),
            message_ids: vec![11, 12, 13],
            sent_count: 3,
            trimmed: false,
            run_by: 9,
            run_ts: 5,
            model: "m".into(),
            input_tokens: 10,
            output_tokens: 2,
            summary: Some(serde_json::json!({ "overview": "x" })),
            raw: "{}".into(),
        };
        let sid = add_summary(&conn, &s).unwrap();
        assert_eq!(summary_for(&conn, "k").unwrap().unwrap().id, sid);
        assert!(summary_for(&conn, "other").unwrap().is_none());
        // Found whichever way round the pair is asked for, if the time overlaps.
        assert_eq!(summaries_near(&conn, 23, &[1, 2], 50_000, 200_000).unwrap().len(), 1);
        assert!(summaries_near(&conn, 23, &[1, 2], 200_000, 300_000).unwrap().is_empty());
        assert!(summaries_near(&conn, 23, &[1, 2, 3], 50_000, 200_000).unwrap().is_empty(), "a different group");
        assert!(period_summaries(&conn, &[1, 2], 0, 200_000).unwrap().is_empty(), "a stretch summary is not a period one");
        let p = NewSummary { stretch_key: "period:1-2-3".into(), channel_id: 0, people: vec![3, 1, 2], scope: SCOPE_PERIOD.into(), ..s.clone() };
        let pid = add_summary(&conn, &p).unwrap();
        assert_eq!(period_summaries(&conn, &[1, 2, 3], 10, 20).unwrap().iter().map(|x| x.id).collect::<Vec<_>>(), vec![pid]);
        assert_eq!(summary(&conn, pid).unwrap().unwrap().new.people, vec![3, 1, 2]);
        assert_eq!(summarised_detections(&conn, &[id, id + 1]).unwrap(), vec![id]);
        assert_eq!(summary(&conn, sid).unwrap().unwrap().new, s);
    }
}
