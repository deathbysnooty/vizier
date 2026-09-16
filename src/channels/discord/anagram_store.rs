//! What Anagrams keeps in `.runtime/anagram.db`: every round the bot has set,
//! and every answer that won one.
//!
//! A round moves `open` → `solved` when someone types a word that fits, or
//! `open` → `skipped` when a player skips it after a hint (or a mod skips it
//! outright), or `open` → `expired` when nobody got it all evening. Only the
//! FIRST correct answer wins: the move to `solved` is one
//! `UPDATE … WHERE status = 'open'`, so two answers landing in the same second
//! can only make one winner.
//!
//! Every round keeps its sorted letters, so the same letters aren't set twice
//! in a month, and who asked for the hint or pressed on, so everything the
//! channel did is attributable afterwards. A day's solvers are a table of their
//! own, ready for a `/today` summary or a panel page.

use std::collections::HashSet;
use std::sync::OnceLock;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

pub const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS rounds (
        id INTEGER PRIMARY KEY AUTOINCREMENT, word TEXT NOT NULL, letters TEXT NOT NULL,
        scramble TEXT NOT NULL, points INTEGER NOT NULL, posted_ts INTEGER NOT NULL,
        message_id INTEGER, channel_id INTEGER NOT NULL, status TEXT NOT NULL DEFAULT 'open',
        winner INTEGER, winning_word TEXT, solved_ts INTEGER, seconds INTEGER,
        hint_by INTEGER, hint_ts INTEGER, ended_by INTEGER, ended_ts INTEGER);
    CREATE INDEX IF NOT EXISTS rounds_status ON rounds (status);
    CREATE INDEX IF NOT EXISTS rounds_letters ON rounds (letters, posted_ts);
    CREATE TABLE IF NOT EXISTS solves (
        day TEXT NOT NULL, user_id INTEGER NOT NULL, round_id INTEGER NOT NULL,
        points INTEGER NOT NULL DEFAULT 0, word TEXT NOT NULL DEFAULT '',
        seconds INTEGER NOT NULL DEFAULT 0, ts INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (user_id, round_id));
    CREATE INDEX IF NOT EXISTS solves_day ON solves (day);
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);";

pub fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)
}

pub fn open(workspace: &str) -> anyhow::Result<()> {
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    std::fs::create_dir_all(&dir)?;
    let conn = Connection::open(dir.join("anagram.db"))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
    init(&conn)?;
    let _ = DB.set(Mutex::new(conn));
    Ok(())
}

pub fn db() -> Option<&'static Mutex<Connection>> {
    DB.get()
}

pub fn meta_get(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0)).optional().ok().flatten()
}

pub fn meta_set(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute("INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![key, value])
        .map(|_| ())
}

// --- rounds ----------------------------------------------------------------------------

/// How a round ended, or that it hasn't.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Up in the channel, nobody has answered it.
    Open,
    Solved,
    /// A player skipped it after a hint, or a mod skipped it outright.
    Skipped,
    /// Nobody touched it for long enough that the bot moved on by itself.
    Expired,
}

impl Status {
    pub fn key(self) -> &'static str {
        match self {
            Status::Open => "open",
            Status::Solved => "solved",
            Status::Skipped => "skipped",
            Status::Expired => "expired",
        }
    }

    fn from_key(key: &str) -> Status {
        match key {
            "solved" => Status::Solved,
            "skipped" => Status::Skipped,
            "expired" => Status::Expired,
            _ => Status::Open,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: i64,
    /// The word the bot scrambled.
    pub word: String,
    /// Its letters in order: what an answer has to match.
    pub letters: String,
    /// The arrangement on the card.
    pub scramble: String,
    /// What the round was worth when it went up, before any hint.
    pub points: i64,
    pub posted_ts: i64,
    pub message: Option<u64>,
    pub channel: u64,
    pub status: Status,
    pub winner: Option<u64>,
    /// The word the winner actually made, which need not be the bot's.
    pub winning_word: Option<String>,
    pub solved_ts: Option<i64>,
    pub seconds: Option<i64>,
    /// Who asked for the first letter, and when.
    pub hint_by: Option<u64>,
    pub hint_ts: Option<i64>,
    /// Who skipped it, and when it ended.
    pub ended_by: Option<u64>,
    pub ended_ts: Option<i64>,
}

impl Row {
    pub fn hinted(&self) -> bool {
        self.hint_by.is_some()
    }

    /// The letter a hint gives away.
    pub fn first_letter(&self) -> char {
        self.word.chars().next().unwrap_or('?').to_ascii_uppercase()
    }
}

const COLS: &str = "id, word, letters, scramble, points, posted_ts, message_id, channel_id, status, winner, winning_word, \
                    solved_ts, seconds, hint_by, hint_ts, ended_by, ended_ts";

fn row(r: &rusqlite::Row) -> rusqlite::Result<Row> {
    let user = |i: usize| -> rusqlite::Result<Option<u64>> { Ok(r.get::<_, Option<i64>>(i)?.map(|v| v as u64)) };
    Ok(Row {
        id: r.get(0)?,
        word: r.get(1)?,
        letters: r.get(2)?,
        scramble: r.get(3)?,
        points: r.get(4)?,
        posted_ts: r.get(5)?,
        message: user(6)?,
        channel: r.get::<_, i64>(7)? as u64,
        status: Status::from_key(&r.get::<_, String>(8)?),
        winner: user(9)?,
        winning_word: r.get(10)?,
        solved_ts: r.get(11)?,
        seconds: r.get(12)?,
        hint_by: user(13)?,
        hint_ts: r.get(14)?,
        ended_by: user(15)?,
        ended_ts: r.get(16)?,
    })
}

/// Writes a fresh round and hands it back with its number.
pub fn add_round(conn: &Connection, word: &str, letters: &str, scramble: &str, points: i64, channel: u64, now: i64) -> rusqlite::Result<Row> {
    conn.execute(
        "INSERT INTO rounds (word, letters, scramble, points, posted_ts, channel_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![word, letters, scramble, points, now, channel as i64],
    )?;
    get(conn, conn.last_insert_rowid()).ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn get(conn: &Connection, id: i64) -> Option<Row> {
    conn.query_row(&format!("SELECT {} FROM rounds WHERE id = ?1", COLS), params![id], row).optional().ok().flatten()
}

/// The round that is up now, if any.
pub fn live(conn: &Connection) -> Option<Row> {
    conn.query_row(&format!("SELECT {} FROM rounds WHERE status = 'open' ORDER BY id DESC LIMIT 1", COLS), [], row).optional().ok().flatten()
}

/// The newest rounds, however they ended.
pub fn recent(conn: &Connection, limit: usize) -> Vec<Row> {
    let sql = format!("SELECT {} FROM rounds ORDER BY id DESC LIMIT ?1", COLS);
    let Ok(mut stmt) = conn.prepare(&sql) else { return Vec::new() };
    stmt.query_map(params![limit as i64], row).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

pub fn set_message(conn: &Connection, id: i64, message: u64) -> rusqlite::Result<()> {
    conn.execute("UPDATE rounds SET message_id = ?2 WHERE id = ?1", params![id, message as i64]).map(|_| ())
}

/// Every set of letters used since `since`: what the no-repeat window is made
/// of.
pub fn letters_since(conn: &Connection, since: i64) -> HashSet<String> {
    let Ok(mut stmt) = conn.prepare("SELECT DISTINCT letters FROM rounds WHERE posted_ts >= ?1") else {
        return HashSet::new();
    };
    stmt.query_map(params![since], |r| r.get::<_, String>(0)).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

/// The one move that decides a race: `open` → `solved`, only the first wins.
/// True for the winner, false for everyone who typed it a moment later.
pub fn claim(conn: &Connection, id: i64, user: u64, word: &str, now: i64) -> rusqlite::Result<bool> {
    conn.execute(
        "UPDATE rounds SET status = 'solved', winner = ?2, winning_word = ?3, solved_ts = ?4,
         seconds = MAX(?4 - posted_ts, 0) WHERE id = ?1 AND status = 'open'",
        params![id, user as i64, word, now],
    )
    .map(|n| n > 0)
}

/// The hint, which one round has exactly one of. True for whoever asked first.
pub fn take_hint(conn: &Connection, id: i64, user: u64, now: i64) -> rusqlite::Result<bool> {
    conn.execute(
        "UPDATE rounds SET hint_by = ?2, hint_ts = ?3 WHERE id = ?1 AND status = 'open' AND hint_by IS NULL",
        params![id, user as i64, now],
    )
    .map(|n| n > 0)
}

/// Passing on a round: by a player after a hint, or by a mod. True when there
/// was one to skip.
pub fn skip(conn: &Connection, id: i64, user: u64, now: i64) -> rusqlite::Result<bool> {
    conn.execute(
        "UPDATE rounds SET status = 'skipped', ended_by = ?2, ended_ts = ?3 WHERE id = ?1 AND status = 'open'",
        params![id, user as i64, now],
    )
    .map(|n| n > 0)
}

/// A round nobody answered and nobody skipped, closed by the bot itself.
pub fn expire(conn: &Connection, id: i64, now: i64) -> rusqlite::Result<bool> {
    conn.execute("UPDATE rounds SET status = 'expired', ended_ts = ?2 WHERE id = ?1 AND status = 'open'", params![id, now])
        .map(|n| n > 0)
}

// --- solves ----------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Solve {
    pub user: u64,
    pub round: i64,
    pub points: i64,
    /// The word they made.
    pub word: String,
    pub seconds: i64,
    pub ts: i64,
}

/// Writes a win. A player counts once per round.
pub fn add_solve(conn: &Connection, day: &str, user: u64, round: i64, points: i64, word: &str, seconds: i64, ts: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO solves (day, user_id, round_id, points, word, seconds, ts) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(user_id, round_id) DO UPDATE SET points = MAX(solves.points, excluded.points)",
        params![day, user as i64, round, points, word, seconds, ts],
    )
    .map(|_| ())
}

fn solve_row(r: &rusqlite::Row) -> rusqlite::Result<Solve> {
    Ok(Solve {
        user: r.get::<_, i64>(0)? as u64,
        round: r.get(1)?,
        points: r.get(2)?,
        word: r.get(3)?,
        seconds: r.get(4)?,
        ts: r.get(5)?,
    })
}

/// A day's wins, first one first. What a `/today` summary - or a panel page -
/// reads.
pub fn day_solves(conn: &Connection, day: &str) -> Vec<Solve> {
    let Ok(mut stmt) = conn.prepare("SELECT user_id, round_id, points, word, seconds, ts FROM solves WHERE day = ?1 ORDER BY ts, round_id") else {
        return Vec::new();
    };
    stmt.query_map(params![day], solve_row).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

/// Whether this player has already won this round.
pub fn solved_by(conn: &Connection, round: i64, user: u64) -> bool {
    conn.query_row("SELECT 1 FROM solves WHERE round_id = ?1 AND user_id = ?2", params![round, user as i64], |r| r.get::<_, i64>(0))
        .optional()
        .ok()
        .flatten()
        .is_some()
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::channels::discord::anagram_words::key;

    pub fn memory() -> Connection {
        let conn = Connection::open_in_memory().expect("memory db");
        init(&conn).expect("schema");
        conn
    }

    pub fn put(conn: &Connection, word: &str, scramble: &str, points: i64, now: i64) -> Row {
        add_round(conn, word, &key(word), scramble, points, 77, now).expect("round")
    }

    #[test]
    fn only_the_first_correct_answer_claims_a_round() {
        let conn = memory();
        let r = put(&conn, "beast", "tsabe", 1, 1_000);
        assert_eq!(live(&conn).map(|l| l.id), Some(r.id));
        assert!(claim(&conn, r.id, 11, "bates", 1_040).unwrap(), "the first gets it");
        assert!(!claim(&conn, r.id, 22, "beast", 1_041).unwrap(), "a second later is too late");
        let after = get(&conn, r.id).unwrap();
        assert_eq!(after.status, Status::Solved);
        assert_eq!((after.winner, after.winning_word.as_deref(), after.seconds), (Some(11), Some("bates"), Some(40)));
        assert!(live(&conn).is_none(), "a solved round is no longer the live one");
        // And nothing can move it afterwards.
        assert!(!skip(&conn, r.id, 5, 1_100).unwrap());
        assert!(!expire(&conn, r.id, 9_000).unwrap());
        assert!(!take_hint(&conn, r.id, 5, 1_100).unwrap());
    }

    #[test]
    fn a_round_has_one_hint_and_it_says_who_asked() {
        let conn = memory();
        let r = put(&conn, "listen", "netsil", 2, 500);
        assert!(!get(&conn, r.id).unwrap().hinted());
        assert!(take_hint(&conn, r.id, 42, 540).unwrap(), "the first ask gets it");
        assert!(!take_hint(&conn, r.id, 43, 560).unwrap(), "and it is the only one");
        let after = get(&conn, r.id).unwrap();
        assert_eq!((after.hint_by, after.hint_ts), (Some(42), Some(540)));
        assert_eq!(after.first_letter(), 'L');
        assert!(after.hinted());
    }

    #[test]
    fn a_skipped_round_and_a_stale_one_are_told_apart_and_both_name_the_time() {
        let conn = memory();
        let one = put(&conn, "beast", "tsabe", 1, 100);
        assert!(skip(&conn, one.id, 7, 200).unwrap());
        let after = get(&conn, one.id).unwrap();
        assert_eq!((after.status, after.ended_by, after.ended_ts), (Status::Skipped, Some(7), Some(200)));
        assert!(!claim(&conn, one.id, 9, "bates", 300).unwrap(), "a skipped round can't be won");
        let two = put(&conn, "planet", "tenalp", 2, 400);
        assert!(expire(&conn, two.id, 3_000).unwrap());
        let after = get(&conn, two.id).unwrap();
        assert_eq!((after.status, after.ended_by, after.ended_ts), (Status::Expired, None, Some(3_000)));
        assert!(live(&conn).is_none());
        assert_eq!(recent(&conn, 5).len(), 2);
    }

    #[test]
    fn the_letters_of_every_round_are_kept_so_a_month_never_repeats_itself() {
        let conn = memory();
        let day = 86_400;
        put(&conn, "beast", "tsabe", 1, 1_000);
        put(&conn, "listen", "netsil", 2, 1_000 + 20 * day);
        let now = 1_000 + 40 * day;
        // A thirty day window leaves the older one out; a fifty day one keeps both.
        let month = letters_since(&conn, now - 30 * day);
        assert_eq!(month, HashSet::from([key("listen")]));
        let longer = letters_since(&conn, now - 50 * day);
        assert!(longer.contains(&key("beast")) && longer.contains(&key("silent")), "the letters, not the word: {:?}", longer);
        assert!(letters_since(&conn, now).is_empty());
    }

    #[test]
    fn a_day_of_wins_is_kept_for_the_summaries() {
        let conn = memory();
        let one = put(&conn, "beast", "tsabe", 1, 10);
        let two = put(&conn, "planet", "tenalp", 2, 20);
        add_solve(&conn, "2026-09-16", 1, one.id, 1, "bates", 40, 100).unwrap();
        add_solve(&conn, "2026-09-16", 2, two.id, 2, "platen", 90, 200).unwrap();
        // The same person on the same round doesn't count twice.
        add_solve(&conn, "2026-09-16", 1, one.id, 0, "beast", 40, 300).unwrap();
        let day = day_solves(&conn, "2026-09-16");
        assert_eq!(day.len(), 2);
        assert_eq!((day[0].user, day[0].points, day[0].word.as_str()), (1, 1, "bates"));
        assert!(solved_by(&conn, one.id, 1) && !solved_by(&conn, one.id, 2));
        assert!(day_solves(&conn, "2026-09-17").is_empty());
    }

    #[test]
    fn meta_remembers_the_card() {
        let conn = memory();
        assert_eq!(meta_get(&conn, "card"), None);
        meta_set(&conn, "card", "5:6").unwrap();
        meta_set(&conn, "card", "7:8").unwrap();
        assert_eq!(meta_get(&conn, "card").as_deref(), Some("7:8"));
        let r = put(&conn, "beast", "tsabe", 1, 1);
        set_message(&conn, r.id, 99).unwrap();
        assert_eq!(get(&conn, r.id).unwrap().message, Some(99));
    }
}
