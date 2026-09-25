//! What the daily topic pass keeps: one small row per member per day, and one
//! row per night saying what that night's pass did.
//!
//! Its own file in the runtime directory, `topics.db`, so losing it never
//! touches the message log or the panel's settings — and losing it costs only
//! the history, since every night builds the next row.
//!
//! No message text is ever in here. A row is a handful of tags, one line the
//! model wrote, a count and some channel ids. That is deliberate: this table is
//! meant to build up over months, and the thing that makes that safe is that
//! there is almost nothing in each row.

use std::path::Path;
use std::sync::OnceLock;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

use super::topics::{DayEntry, Pass};

const SCHEMA: &str = "
    -- One member, one India day. Small on purpose: this is kept for months.
    CREATE TABLE IF NOT EXISTS member_days (
        user_id INTEGER NOT NULL, day TEXT NOT NULL,
        topics_json TEXT NOT NULL, line TEXT NOT NULL DEFAULT '',
        messages INTEGER NOT NULL DEFAULT 0, channels_json TEXT NOT NULL DEFAULT '[]',
        built_ts INTEGER NOT NULL,
        PRIMARY KEY (user_id, day)) WITHOUT ROWID;
    CREATE INDEX IF NOT EXISTS member_days_day ON member_days (day);
    -- One night's work, of one kind: the topic pass, or the scan that picks who
    -- is worth a look. The row is written when the night is claimed and filled
    -- in when it finishes, so a night that died half way is visible as one that
    -- started and never ended.
    CREATE TABLE IF NOT EXISTS runs (
        kind TEXT NOT NULL DEFAULT 'topics', day TEXT NOT NULL,
        started_ts INTEGER NOT NULL, finished_ts INTEGER,
        chunks INTEGER NOT NULL DEFAULT 0, failed INTEGER NOT NULL DEFAULT 0,
        members INTEGER NOT NULL DEFAULT 0, too_thin INTEGER NOT NULL DEFAULT 0,
        rejected INTEGER NOT NULL DEFAULT 0,
        input_tokens INTEGER NOT NULL DEFAULT 0, output_tokens INTEGER NOT NULL DEFAULT 0,
        model TEXT NOT NULL DEFAULT '', note TEXT NOT NULL DEFAULT '',
        -- What the night did not read, so a short night can never be mistaken
        -- for a quiet one: chunks the cap refused, the channels they were of,
        -- and the channels whose answers never came back readable.
        capped INTEGER NOT NULL DEFAULT 0,
        dropped_json TEXT NOT NULL DEFAULT '[]', lost_json TEXT NOT NULL DEFAULT '[]',
        PRIMARY KEY (kind, day));
    CREATE INDEX IF NOT EXISTS runs_started ON runs (kind, started_ts);
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
";

/// A night claimed this long ago that never finished is taken to have died with
/// the process, and may be claimed again.
pub const STALE_SECS: i64 = 6 * 3600;

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

/// The shared connection. Hold the lock only for a query, never across an `.await`.
pub fn db() -> Option<&'static Mutex<Connection>> {
    DB.get()
}

pub fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)?;
    // Added once a night had to say what it did not read. A store made before
    // this simply shows nothing dropped for the nights it already has.
    let have: Vec<String> = {
        let mut stmt = conn.prepare("PRAGMA table_info(runs)")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for (name, kind) in
        [("capped", "INTEGER NOT NULL DEFAULT 0"), ("dropped_json", "TEXT NOT NULL DEFAULT '[]'"), ("lost_json", "TEXT NOT NULL DEFAULT '[]'")]
    {
        if !have.iter().any(|h| h == name) {
            conn.execute_batch(&format!("ALTER TABLE runs ADD COLUMN {} {}", name, kind))?;
        }
    }
    Ok(())
}

/// Opens (or makes) `<workspace>/.runtime/topics.db`. Once per process.
pub fn open(workspace: &str) -> anyhow::Result<()> {
    if DB.get().is_some() {
        return Ok(());
    }
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    open_at(&dir.join("topics.db"))
}

pub fn open_at(path: &Path) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
    init(&conn)?;
    let _ = DB.set(Mutex::new(conn));
    Ok(())
}

/// Opens the real store on a throwaway file, once, so the tests that exercise a
/// whole night have somewhere to claim it. In-memory will not do: the job holds
/// the shared connection, not one of its own.
#[cfg(test)]
pub fn open_for_tests() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let dir = tempfile::tempdir().expect("a place for the test store");
        let path = dir.path().join("topics.db");
        std::mem::forget(dir);
        open_at(&path).expect("the test store opens");
    });
}

/// An in-memory store, for the tests.
pub fn memory() -> Connection {
    let conn = Connection::open_in_memory().expect("in-memory sqlite");
    init(&conn).expect("schema");
    conn
}

// --- the entries -------------------------------------------------------------------------------

fn entry_row(r: &rusqlite::Row) -> rusqlite::Result<DayEntry> {
    let channels: Vec<String> = serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default();
    Ok(DayEntry {
        user_id: r.get::<_, i64>(0)? as u64,
        day: r.get(1)?,
        topics: serde_json::from_str(&r.get::<_, String>(2)?).unwrap_or_default(),
        line: r.get(3)?,
        channels: channels.iter().filter_map(|c| c.parse().ok()).collect(),
        messages: r.get(5)?,
    })
}

const ENTRY_COLUMNS: &str = "user_id, day, topics_json, line, channels_json, messages";

/// One night's entries, all at once. Replacing rather than piling up, so a night
/// run twice leaves exactly what one run would have.
pub fn save(conn: &mut Connection, entries: &[DayEntry], now: i64) -> rusqlite::Result<usize> {
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO member_days (user_id, day, topics_json, line, messages, channels_json, built_ts)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(user_id, day) DO UPDATE SET topics_json = excluded.topics_json, line = excluded.line,
                 messages = excluded.messages, channels_json = excluded.channels_json, built_ts = excluded.built_ts",
        )?;
        for e in entries {
            stmt.execute(params![
                e.user_id as i64,
                e.day,
                serde_json::to_string(&e.topics).unwrap_or_else(|_| "[]".into()),
                e.line,
                e.messages,
                serde_json::to_string(&e.channels.iter().map(|c| c.to_string()).collect::<Vec<_>>()).unwrap_or_else(|_| "[]".into()),
                now,
            ])?;
        }
    }
    tx.commit()?;
    Ok(entries.len())
}

/// One member's days, newest first, no further back than `since_day`.
pub fn for_member(conn: &Connection, user: u64, since_day: &str, limit: usize) -> rusqlite::Result<Vec<DayEntry>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {} FROM member_days WHERE user_id = ?1 AND day >= ?2 ORDER BY day DESC LIMIT ?3",
        ENTRY_COLUMNS
    ))?;
    let rows = stmt.query_map(params![user as i64, since_day, limit as i64], entry_row)?;
    rows.collect()
}

/// One member's one day, if it was recorded. Nothing back means a day with
/// nothing much on it, never a day that was missed — `run_for` says which.
pub fn one(conn: &Connection, user: u64, day: &str) -> rusqlite::Result<Option<DayEntry>> {
    conn.query_row(&format!("SELECT {} FROM member_days WHERE user_id = ?1 AND day = ?2", ENTRY_COLUMNS), params![user as i64, day], entry_row)
        .optional()
}

/// Everybody's entries between two India days, both ends included: the
/// server-wide view of a week.
pub fn between(conn: &Connection, from_day: &str, to_day: &str, limit: usize) -> rusqlite::Result<Vec<DayEntry>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {} FROM member_days WHERE day >= ?1 AND day <= ?2 ORDER BY day DESC, messages DESC LIMIT ?3",
        ENTRY_COLUMNS
    ))?;
    let rows = stmt.query_map(params![from_day, to_day, limit as i64], entry_row)?;
    rows.collect()
}

pub fn count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM member_days", [], |r| r.get(0)).unwrap_or(0)
}

/// Drops entries older than `before_day`, so the store stays a few months
/// rather than for ever.
pub fn trim(conn: &Connection, before_day: &str) -> rusqlite::Result<usize> {
    Ok(conn.execute("DELETE FROM member_days WHERE day < ?1", params![before_day])?)
}

// --- the nights --------------------------------------------------------------------------------

/// The nightly topic pass over a day's chat.
pub const KIND_TOPICS: &str = "topics";
/// The nightly scan that picks who is worth a deep dive.
pub const KIND_SCAN: &str = "scan";

/// What one night's work did, as the page shows it. The same row serves both
/// nightly jobs; a scan simply leaves the token counts at nought.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Run {
    pub kind: String,
    pub day: String,
    pub started_ts: i64,
    pub finished_ts: Option<i64>,
    /// Chunks sent, for the topic pass; members looked at, for the scan.
    pub chunks: i64,
    pub failed: i64,
    /// Members written down, for the topic pass; members dived into, for the scan.
    pub members: i64,
    pub too_thin: i64,
    pub rejected: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub model: String,
    /// Why a night that finished badly finished badly, in words. This is what
    /// puts a failure on the page rather than only in the log.
    pub note: String,
    /// Chunks the cap refused to send. Nought means the night fitted.
    pub capped: i64,
    /// The channels those chunks were of, and how many messages went unread.
    pub dropped: Vec<(String, i64)>,
    /// The channels whose answers never came back readable, even after the retry.
    pub lost: Vec<String>,
}

impl Run {
    /// A night that started and never came back: the process went while it was
    /// working, or it is working now.
    pub fn unfinished(&self) -> bool {
        self.finished_ts.is_none()
    }
}

fn run_row(r: &rusqlite::Row) -> rusqlite::Result<Run> {
    Ok(Run {
        kind: r.get(0)?,
        day: r.get(1)?,
        started_ts: r.get(2)?,
        finished_ts: r.get(3)?,
        chunks: r.get(4)?,
        failed: r.get(5)?,
        members: r.get(6)?,
        too_thin: r.get(7)?,
        rejected: r.get(8)?,
        input_tokens: r.get(9)?,
        output_tokens: r.get(10)?,
        model: r.get(11)?,
        note: r.get(12)?,
        capped: r.get(13)?,
        dropped: serde_json::from_str(&r.get::<_, String>(14)?).unwrap_or_default(),
        lost: serde_json::from_str(&r.get::<_, String>(15)?).unwrap_or_default(),
    })
}

const RUN_COLUMNS: &str = "kind, day, started_ts, finished_ts, chunks, failed, members, too_thin, rejected, input_tokens, output_tokens, \
     model, note, capped, dropped_json, lost_json";

/// Takes tonight's day for one job, or says somebody already has it.
///
/// This is what makes a re-run a no-op: the row goes in before a single token is
/// spent, and a day that already has one is refused. A claim that was taken more
/// than [`STALE_SECS`] ago and never finished is taken to have died with the
/// process, and may be taken again — otherwise one crash would cost that day for
/// good.
pub fn claim(conn: &Connection, kind: &str, day: &str, now: i64) -> rusqlite::Result<bool> {
    let existing: Option<(i64, Option<i64>)> = conn
        .query_row("SELECT started_ts, finished_ts FROM runs WHERE kind = ?1 AND day = ?2", params![kind, day], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    match existing {
        Some((_, Some(_))) => Ok(false),
        Some((started, None)) if now - started < STALE_SECS => Ok(false),
        Some((_, None)) => {
            conn.execute("UPDATE runs SET started_ts = ?3, note = '' WHERE kind = ?1 AND day = ?2", params![kind, day, now])?;
            Ok(true)
        }
        None => {
            conn.execute("INSERT INTO runs (kind, day, started_ts) VALUES (?1, ?2, ?3)", params![kind, day, now])?;
            Ok(true)
        }
    }
}

/// What a finished night is written down as.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Done {
    pub chunks: i64,
    pub failed: i64,
    pub members: i64,
    pub too_thin: i64,
    pub rejected: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub model: String,
    pub note: String,
    pub capped: i64,
    pub dropped: Vec<(String, i64)>,
    pub lost: Vec<String>,
}

impl From<&Pass> for Done {
    fn from(p: &Pass) -> Self {
        Done {
            chunks: p.chunks as i64,
            failed: p.failed as i64,
            members: p.entries.len() as i64,
            too_thin: p.too_thin as i64,
            rejected: p.rejected as i64,
            input_tokens: p.input_tokens as i64,
            output_tokens: p.output_tokens as i64,
            model: p.model.clone(),
            // What the night lost, in the same words the log used.
            note: p.note(),
            capped: p.capped as i64,
            dropped: p.dropped.iter().map(|(name, lost)| (name.clone(), *lost as i64)).collect(),
            lost: p.lost.clone(),
        }
    }
}

/// Fills a claimed night in once the whole day has been read. Called once, after
/// whatever was to be written is written, so a night is only ever "done" when it
/// really is.
pub fn finish(conn: &Connection, kind: &str, day: &str, done: &Done, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE runs SET finished_ts = ?3, chunks = ?4, failed = ?5, members = ?6, too_thin = ?7, rejected = ?8,
             input_tokens = ?9, output_tokens = ?10, model = ?11, note = ?12, capped = ?13, dropped_json = ?14, lost_json = ?15
           WHERE kind = ?1 AND day = ?2",
        params![
            kind,
            day,
            now,
            done.chunks,
            done.failed,
            done.members,
            done.too_thin,
            done.rejected,
            done.input_tokens,
            done.output_tokens,
            done.model,
            done.note,
            done.capped,
            serde_json::to_string(&done.dropped).unwrap_or_else(|_| "[]".into()),
            serde_json::to_string(&done.lost).unwrap_or_else(|_| "[]".into()),
        ],
    )?;
    Ok(())
}

/// Gives a claim back without marking the night done, so the next run may try
/// again — and leaves the reason on the row, where the page can read it. What a
/// night that failed outright does.
pub fn release(conn: &Connection, kind: &str, day: &str, note: &str) -> rusqlite::Result<()> {
    conn.execute("UPDATE runs SET started_ts = 0, note = ?3 WHERE kind = ?1 AND day = ?2 AND finished_ts IS NULL", params![kind, day, note])?;
    Ok(())
}

pub fn run_for(conn: &Connection, kind: &str, day: &str) -> rusqlite::Result<Option<Run>> {
    conn.query_row(&format!("SELECT {} FROM runs WHERE kind = ?1 AND day = ?2", RUN_COLUMNS), params![kind, day], run_row).optional()
}

/// The last few nights of one job, newest first.
pub fn runs(conn: &Connection, kind: &str, limit: usize) -> rusqlite::Result<Vec<Run>> {
    let mut stmt = conn.prepare_cached(&format!("SELECT {} FROM runs WHERE kind = ?1 ORDER BY day DESC LIMIT ?2", RUN_COLUMNS))?;
    let rows = stmt.query_map(params![kind, limit as i64], run_row)?;
    rows.collect()
}

pub fn meta_get(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0)).optional().ok().flatten()
}

pub fn meta_set(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute("INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![key, value])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(user: u64, day: &str, topics: &[&str]) -> DayEntry {
        DayEntry {
            user_id: user,
            day: day.into(),
            topics: topics.iter().map(|t| t.to_string()).collect(),
            line: "Was on about cricket most of the evening.".into(),
            messages: 42,
            channels: vec![5, 6],
        }
    }

    #[test]
    fn entries_round_trip_and_a_day_written_twice_stays_one_row() {
        let mut conn = memory();
        let day = "2026-09-21";
        save(&mut conn, &[entry(11, day, &["cricket", "the house cup"]), entry(22, day, &["anime"])], 900).unwrap();
        assert_eq!(count(&conn), 2);
        let back = one(&conn, 11, day).unwrap().unwrap();
        assert_eq!(back.topics, vec!["cricket", "the house cup"]);
        assert_eq!((back.messages, back.channels.clone()), (42, vec![5, 6]));
        assert!(one(&conn, 99, day).unwrap().is_none(), "somebody with nothing much that day");

        // The same night again replaces rather than piling up.
        save(&mut conn, &[entry(11, day, &["chess"])], 1_000).unwrap();
        assert_eq!(count(&conn), 2);
        assert_eq!(one(&conn, 11, day).unwrap().unwrap().topics, vec!["chess"]);

        // A member's own history, newest first, and the window holds.
        save(&mut conn, &[entry(11, "2026-09-20", &["chess"]), entry(11, "2026-08-01", &["old"])], 1_000).unwrap();
        let mine = for_member(&conn, 11, "2026-09-01", 50).unwrap();
        assert_eq!(mine.iter().map(|e| e.day.as_str()).collect::<Vec<_>>(), vec!["2026-09-21", "2026-09-20"], "August is outside the window");
        assert_eq!(for_member(&conn, 11, "2026-09-01", 1).unwrap().len(), 1, "the limit holds");

        // And a week across everybody.
        let week = between(&conn, "2026-09-20", "2026-09-21", 500).unwrap();
        assert_eq!(week.len(), 3);
        assert!(between(&conn, "2026-10-01", "2026-10-07", 500).unwrap().is_empty());

        assert_eq!(trim(&conn, "2026-09-01").unwrap(), 1, "August went");
        assert_eq!(count(&conn), 3);
    }

    #[test]
    fn a_night_is_claimed_before_it_is_worked_and_only_once() {
        let conn = memory();
        let (day, now) = ("2026-09-21", 1_700_000_000);
        assert!(claim(&conn, KIND_TOPICS, day, now).unwrap(), "nobody has tonight yet");
        assert!(!claim(&conn, KIND_TOPICS, day, now).unwrap(), "a second process gets nothing");
        assert!(!claim(&conn, KIND_TOPICS, day, now + 60).unwrap(), "and so does a restart a minute later");
        assert!(claim(&conn, KIND_TOPICS, "2026-09-22", now).unwrap(), "another night is another night");
        assert!(claim(&conn, KIND_SCAN, day, now).unwrap(), "and the scan's night is its own");

        // A claim that died with the process is taken again once it is stale.
        assert!(!claim(&conn, KIND_TOPICS, day, now + STALE_SECS - 1).unwrap());
        assert!(claim(&conn, KIND_TOPICS, day, now + STALE_SECS).unwrap(), "six hours in, nobody is working on it");

        // Once it is finished it is finished, stale or not.
        let pass = Pass { chunks: 7, failed: 1, too_thin: 30, rejected: 4, input_tokens: 110_000, output_tokens: 3_000, model: "cheap".into(), ..Pass::default() };
        finish(&conn, KIND_TOPICS, day, &Done::from(&pass), now + 300).unwrap();
        assert!(!claim(&conn, KIND_TOPICS, day, now + 10 * STALE_SECS).unwrap(), "a finished night is never done again");
        let r = run_for(&conn, KIND_TOPICS, day).unwrap().unwrap();
        assert_eq!((r.chunks, r.failed, r.too_thin, r.input_tokens, r.model.as_str()), (7, 1, 30, 110_000, "cheap"));
        assert!(!r.unfinished());
        assert!(run_for(&conn, KIND_TOPICS, "2026-09-22").unwrap().unwrap().unfinished(), "claimed and still going");
        assert_eq!(runs(&conn, KIND_TOPICS, 10).unwrap().first().map(|r| r.day.clone()), Some("2026-09-22".to_string()), "newest first");
        assert_eq!(runs(&conn, KIND_SCAN, 10).unwrap().len(), 1, "one job's nights are not the other's");

        // A night that failed outright gives its claim back at once, and says why
        // on the row rather than only in the log.
        release(&conn, KIND_TOPICS, "2026-09-22", "the model never answered").unwrap();
        assert_eq!(run_for(&conn, KIND_TOPICS, "2026-09-22").unwrap().unwrap().note, "the model never answered");
        assert!(claim(&conn, KIND_TOPICS, "2026-09-22", now).unwrap(), "a released night can be tried again");
        assert_eq!(meta_get(&conn, "nothing"), None);
        meta_set(&conn, "last_look", "17").unwrap();
        assert_eq!(meta_get(&conn, "last_look").as_deref(), Some("17"));
    }

    /// A row is tags, a line and counts — never anything anybody said.
    #[test]
    fn a_row_keeps_no_message_text() {
        let mut conn = memory();
        save(&mut conn, &[entry(11, "2026-09-21", &["cricket"])], 900).unwrap();
        let columns: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(member_days)").unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, String>(1)).unwrap();
            rows.flatten().collect()
        };
        assert_eq!(columns, vec!["user_id", "day", "topics_json", "line", "messages", "channels_json", "built_ts"]);
        assert!(!columns.iter().any(|c| c == "content" || c == "text" || c == "message_ids_json"), "{columns:?}");
    }
}
