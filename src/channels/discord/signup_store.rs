//! What the sign-up buttons keep, in their own small file in the runtime
//! directory, `signups.db`, so losing it never touches anything else.
//!
//! Two things. One row per member who has pressed a button: which way they
//! answered, the name they had at the time, when they first answered and when
//! they last changed their mind, and whether the role is actually on them. And
//! one row per sign-up message the bot has posted: where it is, who posted it,
//! and the exact words it was posted with — which is what makes the live count
//! survive a restart, because the message can be rewritten from the store
//! alone with no memory of having posted it.
//!
//! The record is the point of the whole feature: next month's eggs are handed
//! out from this list, so it is written before anything else is attempted and
//! kept whether or not the role was ever given.

use std::path::Path;
use std::sync::OnceLock;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS answers (
        user_id INTEGER PRIMARY KEY, answer TEXT NOT NULL, name TEXT NOT NULL DEFAULT '',
        first_ts INTEGER NOT NULL, ts INTEGER NOT NULL, worn INTEGER NOT NULL DEFAULT 0);
    CREATE INDEX IF NOT EXISTS answers_by_answer ON answers (answer, ts);
    CREATE TABLE IF NOT EXISTS posts (
        message_id INTEGER PRIMARY KEY, channel_id INTEGER NOT NULL, posted_by INTEGER NOT NULL,
        posted_ts INTEGER NOT NULL, title TEXT NOT NULL DEFAULT '', body TEXT NOT NULL DEFAULT '',
        images INTEGER NOT NULL DEFAULT 0);
";

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

/// The shared connection. Hold the lock only for a query, never across an `.await`.
pub fn db() -> Option<&'static Mutex<Connection>> {
    DB.get()
}

/// Which way somebody answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    In,
    Out,
}

impl Answer {
    pub fn key(self) -> &'static str {
        match self {
            Answer::In => "in",
            Answer::Out => "out",
        }
    }

    pub fn from_key(raw: &str) -> Option<Self> {
        match raw {
            "in" => Some(Answer::In),
            "out" => Some(Answer::Out),
            _ => None,
        }
    }
}

/// One member's answer as it stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub user_id: u64,
    pub name: String,
    pub answer: Answer,
    /// The first time they answered at all, whichever way.
    pub first_ts: i64,
    /// The last time they changed anything.
    pub ts: i64,
    /// Whether the role is on them. False for everyone recorded while the
    /// setting was empty or the role was out of the bot's reach.
    pub worn: bool,
}

/// One sign-up message the bot has posted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Post {
    pub message_id: u64,
    pub channel_id: u64,
    pub posted_by: u64,
    pub posted_ts: i64,
    pub title: String,
    pub body: String,
    pub images: i64,
}

fn prepare(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)
}

/// Opens (or makes) `<workspace>/.runtime/signups.db`. Once per process.
pub fn open(workspace: &str) -> anyhow::Result<()> {
    if DB.get().is_some() {
        return Ok(());
    }
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    open_at(&dir.join("signups.db"))
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

/// An in-memory store, for tests.
pub fn open_memory() -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    prepare(&conn)?;
    Ok(conn)
}

// --- the answers -------------------------------------------------------------

fn read_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<Entry> {
    let answer: String = row.get(2)?;
    Ok(Entry {
        user_id: row.get::<_, i64>(0)? as u64,
        name: row.get(1)?,
        answer: Answer::from_key(&answer).unwrap_or(Answer::Out),
        first_ts: row.get(3)?,
        ts: row.get(4)?,
        worn: row.get::<_, i64>(5)? != 0,
    })
}

const COLUMNS: &str = "user_id, name, answer, first_ts, ts, worn";

/// How one member stands, if they have ever pressed anything.
pub fn answer_of(conn: &Connection, user: u64) -> rusqlite::Result<Option<Entry>> {
    conn.query_row(&format!("SELECT {} FROM answers WHERE user_id = ?1", COLUMNS), params![user as i64], read_entry).optional()
}

/// Writes one answer. The first time is remembered separately from the last, so
/// a member who changes their mind keeps the day they first spoke up. A name
/// that comes through empty never overwrites one already on record.
pub fn record(conn: &Connection, user: u64, name: &str, answer: Answer, worn: bool, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO answers (user_id, name, answer, first_ts, ts, worn) VALUES (?1, ?2, ?3, ?4, ?4, ?5)
         ON CONFLICT(user_id) DO UPDATE SET
            answer = excluded.answer,
            name = CASE WHEN excluded.name = '' THEN answers.name ELSE excluded.name END,
            ts = excluded.ts,
            worn = excluded.worn",
        params![user as i64, name, answer.key(), now, i64::from(worn)],
    )?;
    Ok(())
}

/// How many are in, and how many said no.
pub fn counts(conn: &Connection) -> (i64, i64) {
    let one = |answer: Answer| {
        conn.query_row("SELECT COUNT(*) FROM answers WHERE answer = ?1", params![answer.key()], |r| r.get::<_, i64>(0)).unwrap_or(0)
    };
    (one(Answer::In), one(Answer::Out))
}

/// Everyone who answered one way, in the order they last answered — oldest
/// first, so the list reads as the sign-up sheet it is.
pub fn listed(conn: &Connection, answer: Answer) -> rusqlite::Result<Vec<Entry>> {
    let mut stmt = conn.prepare(&format!("SELECT {} FROM answers WHERE answer = ?1 ORDER BY ts, user_id", COLUMNS))?;
    let rows = stmt.query_map(params![answer.key()], read_entry)?;
    rows.collect()
}

/// How many are in without the role actually on them: the number the panel has
/// to warn about, because those members are on the list and nowhere else.
pub fn unworn(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM answers WHERE answer = 'in' AND worn = 0", [], |r| r.get::<_, i64>(0)).unwrap_or(0)
}

// --- the messages ------------------------------------------------------------

/// Notes a message the bot has just posted, so its count can be rewritten later
/// from the store alone.
pub fn add_post(conn: &Connection, post: &Post) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO posts (message_id, channel_id, posted_by, posted_ts, title, body, images)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(message_id) DO UPDATE SET title = excluded.title, body = excluded.body, images = excluded.images",
        params![
            post.message_id as i64,
            post.channel_id as i64,
            post.posted_by as i64,
            post.posted_ts,
            post.title,
            post.body,
            post.images
        ],
    )?;
    Ok(())
}

fn read_post(row: &rusqlite::Row<'_>) -> rusqlite::Result<Post> {
    Ok(Post {
        message_id: row.get::<_, i64>(0)? as u64,
        channel_id: row.get::<_, i64>(1)? as u64,
        posted_by: row.get::<_, i64>(2)? as u64,
        posted_ts: row.get(3)?,
        title: row.get(4)?,
        body: row.get(5)?,
        images: row.get(6)?,
    })
}

const POST_COLUMNS: &str = "message_id, channel_id, posted_by, posted_ts, title, body, images";

/// One posted message, by its Discord id. This is what makes the buttons
/// outlive a restart: a click carries the message id, and the words come back
/// from here rather than from anything the process remembered.
pub fn post(conn: &Connection, message: u64) -> rusqlite::Result<Option<Post>> {
    conn.query_row(&format!("SELECT {} FROM posts WHERE message_id = ?1", POST_COLUMNS), params![message as i64], read_post)
        .optional()
}

/// Every sign-up message, newest first.
pub fn posts(conn: &Connection) -> rusqlite::Result<Vec<Post>> {
    let mut stmt = conn.prepare(&format!("SELECT {} FROM posts ORDER BY posted_ts DESC, message_id DESC", POST_COLUMNS))?;
    let rows = stmt.query_map([], read_post)?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_keeps_the_day_it_was_first_given() {
        let conn = open_memory().unwrap();
        record(&conn, 7, "Zoya", Answer::In, true, 100).unwrap();
        record(&conn, 7, "Zoya", Answer::Out, false, 500).unwrap();
        let e = answer_of(&conn, 7).unwrap().unwrap();
        assert_eq!((e.answer, e.first_ts, e.ts), (Answer::Out, 100, 500), "the first time stands, the last time moves");
        assert!(!e.worn);
        // An empty name never wipes the one on record.
        record(&conn, 7, "", Answer::In, true, 900).unwrap();
        assert_eq!(answer_of(&conn, 7).unwrap().unwrap().name, "Zoya");
    }

    #[test]
    fn the_lists_and_the_counts_split_the_two_answers() {
        let conn = open_memory().unwrap();
        record(&conn, 1, "Kabir", Answer::In, true, 10).unwrap();
        record(&conn, 2, "Meera", Answer::In, false, 20).unwrap();
        record(&conn, 3, "Rohan", Answer::Out, false, 30).unwrap();
        assert_eq!(counts(&conn), (2, 1));
        assert_eq!(listed(&conn, Answer::In).unwrap().iter().map(|e| e.user_id).collect::<Vec<_>>(), vec![1, 2], "oldest first");
        assert_eq!(listed(&conn, Answer::Out).unwrap().len(), 1);
        // Meera is on the list without the role: exactly what the panel warns about.
        assert_eq!(unworn(&conn), 1);
        assert_eq!(answer_of(&conn, 99).unwrap(), None);
    }

    #[test]
    fn a_posted_message_is_found_again_by_its_id() {
        let conn = open_memory().unwrap();
        let p = Post {
            message_id: 4242,
            channel_id: 77,
            posted_by: 1,
            posted_ts: 1_700_000_000,
            title: "Next month".into(),
            body: "the mods have an idea".into(),
            images: 2,
        };
        add_post(&conn, &p).unwrap();
        assert_eq!(post(&conn, 4242).unwrap(), Some(p.clone()));
        assert_eq!(post(&conn, 1).unwrap(), None);
        add_post(&conn, &Post { title: "Reworded".into(), ..p.clone() }).unwrap();
        assert_eq!(posts(&conn).unwrap().len(), 1, "the same message is not listed twice");
        assert_eq!(posts(&conn).unwrap()[0].title, "Reworded");
    }

    /// The whole point of the store: a restart must not lose the sheet, nor the
    /// words the message was posted with.
    #[test]
    fn the_record_is_on_disk_and_comes_back_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/signups.db");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        {
            let conn = Connection::open(&path).unwrap();
            prepare(&conn).unwrap();
            record(&conn, 11, "Ira", Answer::In, true, 400).unwrap();
            add_post(&conn, &Post { message_id: 9, channel_id: 8, title: "T".into(), body: "B".into(), ..Post::default() }).unwrap();
        }
        // A second process opening the same file.
        let conn = Connection::open(&path).unwrap();
        prepare(&conn).unwrap();
        assert_eq!(counts(&conn), (1, 0));
        let e = answer_of(&conn, 11).unwrap().unwrap();
        assert_eq!((e.name.as_str(), e.answer, e.worn), ("Ira", Answer::In, true));
        assert_eq!(post(&conn, 9).unwrap().map(|p| (p.title, p.body)), Some(("T".into(), "B".into())));
    }
}
