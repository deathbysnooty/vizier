//! What Truth or Dare keeps in `.runtime/dare.db`: the question that is up, the
//! votes that put it there, and the votes to be rid of it.
//!
//! A round walks `voting` → `open` → (`skipped` | `expired`). It starts as a
//! row with no question at all: the channel votes 💬 or 🔥 on it, and the first
//! kind to reach the threshold turns that same row into the question. A member's
//! own question ([`open_asked`]) skips the vote and goes straight to `open`.
//!
//! Both moves are one `UPDATE … WHERE status = …`, so two people voting in the
//! same second can only put one question up, and three people pressing skip
//! together can only end the round once.
//!
//! Votes live in their own table keyed `(round, user, choice)`, which is what
//! makes a tally a count of DISTINCT people rather than of presses: pressing
//! 💬 twice is pressing it once. One person may hold a truth vote and a skip
//! vote at the same time — they are votes on different questions.
//!
//! The no-repeat window is global here rather than per person. Nobody is asked
//! anything individually any more, so "don't ask *them* that again" has nothing
//! to attach to; what matters is that the channel isn't served the same question
//! twice in a fortnight.

use std::collections::HashSet;
use std::sync::OnceLock;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

pub const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS rounds (
        id INTEGER PRIMARY KEY AUTOINCREMENT, topic TEXT, prompt_key TEXT, text TEXT, asked_by INTEGER,
        status TEXT NOT NULL DEFAULT 'voting', posted_ts INTEGER NOT NULL, opened_ts INTEGER,
        answers INTEGER NOT NULL DEFAULT 0, first_answer INTEGER, last_answer_ts INTEGER,
        message_id INTEGER, channel_id INTEGER NOT NULL, ended_ts INTEGER, ended_by INTEGER);
    CREATE INDEX IF NOT EXISTS rounds_status ON rounds (status);
    CREATE INDEX IF NOT EXISTS rounds_asker ON rounds (asked_by, posted_ts);
    CREATE TABLE IF NOT EXISTS votes (
        round_id INTEGER NOT NULL, user_id INTEGER NOT NULL, choice TEXT NOT NULL, ts INTEGER NOT NULL,
        PRIMARY KEY (round_id, user_id, choice));
    CREATE TABLE IF NOT EXISTS asked (prompt_key TEXT PRIMARY KEY, ts INTEGER NOT NULL);
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);";

pub fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)
}

pub fn open(workspace: &str) -> anyhow::Result<()> {
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    std::fs::create_dir_all(&dir)?;
    let conn = Connection::open(dir.join("dare.db"))?;
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

// --- what a round is -------------------------------------------------------------------------

/// What kind of question is up. `Asked` is a member's own, in their own words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Topic {
    Truth,
    Dare,
    Asked,
}

impl Topic {
    pub fn key(self) -> &'static str {
        match self {
            Topic::Truth => "truth",
            Topic::Dare => "dare",
            Topic::Asked => "asked",
        }
    }

    pub fn from_key(key: &str) -> Option<Topic> {
        match key {
            "truth" => Some(Topic::Truth),
            "dare" => Some(Topic::Dare),
            "asked" => Some(Topic::Asked),
            _ => None,
        }
    }
}

/// How a round is getting on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// No question yet: the channel is voting on what it wants.
    Voting,
    /// A question is up and the room is answering.
    Open,
    /// Voted away.
    Skipped,
    /// The room moved on.
    Expired,
}

impl Status {
    pub fn key(self) -> &'static str {
        match self {
            Status::Voting => "voting",
            Status::Open => "open",
            Status::Skipped => "skipped",
            Status::Expired => "expired",
        }
    }

    pub fn from_key(key: &str) -> Status {
        match key {
            "voting" => Status::Voting,
            "open" => Status::Open,
            "skipped" => Status::Skipped,
            _ => Status::Expired,
        }
    }

    /// Whether the card is still showing this round.
    pub fn live(self) -> bool {
        matches!(self, Status::Voting | Status::Open)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: i64,
    pub topic: Option<Topic>,
    /// The bank slug, for a truth or a dare.
    pub prompt_key: Option<String>,
    /// The words themselves, for a member's own question.
    pub text: Option<String>,
    pub asked_by: Option<u64>,
    pub status: Status,
    pub posted_ts: i64,
    pub opened_ts: Option<i64>,
    pub answers: i64,
    pub first_answer: Option<u64>,
    pub last_answer_ts: Option<i64>,
    pub message: Option<u64>,
    pub channel: u64,
    pub ended_ts: Option<i64>,
    pub ended_by: Option<u64>,
}

impl Row {
    /// When the room last did anything with this round: answered it, or failed
    /// to. What the idle clock runs from.
    pub fn touched_ts(&self) -> i64 {
        self.last_answer_ts.or(self.opened_ts).unwrap_or(self.posted_ts)
    }
}

const COLUMNS: &str = "id, topic, prompt_key, text, asked_by, status, posted_ts, opened_ts, answers, \
                       first_answer, last_answer_ts, message_id, channel_id, ended_ts, ended_by";

fn read(row: &rusqlite::Row) -> rusqlite::Result<Row> {
    let topic: Option<String> = row.get(1)?;
    let status: String = row.get(5)?;
    Ok(Row {
        id: row.get(0)?,
        topic: topic.as_deref().and_then(Topic::from_key),
        prompt_key: row.get(2)?,
        text: row.get(3)?,
        asked_by: row.get::<_, Option<i64>>(4)?.map(|v| v as u64),
        status: Status::from_key(&status),
        posted_ts: row.get(6)?,
        opened_ts: row.get(7)?,
        answers: row.get(8)?,
        first_answer: row.get::<_, Option<i64>>(9)?.map(|v| v as u64),
        last_answer_ts: row.get(10)?,
        message: row.get::<_, Option<i64>>(11)?.map(|v| v as u64),
        channel: row.get::<_, i64>(12)? as u64,
        ended_ts: row.get(13)?,
        ended_by: row.get::<_, Option<i64>>(14)?.map(|v| v as u64),
    })
}

/// Opens the voting: a round with no question on it yet.
pub fn start_vote(conn: &Connection, channel: u64, now: i64) -> rusqlite::Result<Row> {
    conn.execute("INSERT INTO rounds (status, posted_ts, channel_id) VALUES ('voting', ?1, ?2)", params![now, channel as i64])?;
    get(conn, conn.last_insert_rowid()).ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn get(conn: &Connection, id: i64) -> Option<Row> {
    conn.query_row(&format!("SELECT {} FROM rounds WHERE id = ?1", COLUMNS), params![id], read).optional().ok().flatten()
}

/// The round the card is showing, if there is one.
pub fn live(conn: &Connection) -> Option<Row> {
    conn.query_row(&format!("SELECT {} FROM rounds WHERE status IN ('voting', 'open') ORDER BY id DESC LIMIT 1", COLUMNS), [], read)
        .optional()
        .ok()
        .flatten()
}

pub fn set_message(conn: &Connection, id: i64, message: u64) -> rusqlite::Result<()> {
    conn.execute("UPDATE rounds SET message_id = ?2 WHERE id = ?1", params![id, message as i64]).map(|_| ())
}

/// The vote carried: this round is now that question. Only a round still in
/// `voting` can be opened, so two votes landing together open it once.
pub fn open_with(conn: &Connection, id: i64, topic: Topic, prompt_key: &str, now: i64) -> rusqlite::Result<bool> {
    let done = conn.execute(
        "UPDATE rounds SET status = 'open', topic = ?2, prompt_key = ?3, opened_ts = ?4 WHERE id = ?1 AND status = 'voting'",
        params![id, topic.key(), prompt_key, now],
    )?;
    if done > 0 {
        note_asked(conn, prompt_key, now)?;
    }
    Ok(done > 0)
}

/// A member's own question, which needs no vote to go up.
pub fn open_asked(conn: &Connection, id: i64, text: &str, by: u64, now: i64) -> rusqlite::Result<bool> {
    conn.execute(
        "UPDATE rounds SET status = 'open', topic = 'asked', text = ?2, asked_by = ?3, opened_ts = ?4 \
         WHERE id = ?1 AND status = 'voting'",
        params![id, text, by as i64, now],
    )
    .map(|n| n > 0)
}

/// Somebody answered. The count is what the card shows; the first one is who
/// gets the ✅.
pub fn note_answer(conn: &Connection, id: i64, message: u64, now: i64) -> rusqlite::Result<bool> {
    conn.execute(
        "UPDATE rounds SET answers = answers + 1, last_answer_ts = ?3, \
         first_answer = COALESCE(first_answer, ?2) WHERE id = ?1 AND status = 'open'",
        params![id, message as i64, now],
    )
    .map(|n| n > 0)
}

/// Voted away, or dropped by a mod.
pub fn skip(conn: &Connection, id: i64, by: Option<u64>, now: i64) -> rusqlite::Result<bool> {
    conn.execute(
        "UPDATE rounds SET status = 'skipped', ended_ts = ?2, ended_by = ?3 WHERE id = ?1 AND status IN ('voting', 'open')",
        params![id, now, by.map(|v| v as i64)],
    )
    .map(|n| n > 0)
}

/// The room moved on.
pub fn expire(conn: &Connection, id: i64, now: i64) -> rusqlite::Result<bool> {
    conn.execute("UPDATE rounds SET status = 'expired', ended_ts = ?2 WHERE id = ?1 AND status IN ('voting', 'open')", params![id, now])
        .map(|n| n > 0)
}

pub fn recent(conn: &Connection, limit: usize) -> Vec<Row> {
    let sql = format!("SELECT {} FROM rounds WHERE status NOT IN ('voting') ORDER BY id DESC LIMIT ?1", COLUMNS);
    let Ok(mut stmt) = conn.prepare(&sql) else { return Vec::new() };
    let Ok(rows) = stmt.query_map(params![limit as i64], read) else { return Vec::new() };
    rows.filter_map(Result::ok).collect()
}

// --- votes ------------------------------------------------------------------------------------

/// How many DISTINCT people want each thing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub truth: i64,
    pub dare: i64,
    pub skip: i64,
}

/// Records a vote. `false` means they had already cast that one — pressing a
/// button twice is pressing it once.
pub fn vote(conn: &Connection, round: i64, user: u64, choice: &str, now: i64) -> rusqlite::Result<bool> {
    conn.execute(
        "INSERT OR IGNORE INTO votes (round_id, user_id, choice, ts) VALUES (?1, ?2, ?3, ?4)",
        params![round, user as i64, choice, now],
    )
    .map(|n| n > 0)
}

/// Takes a vote back, for a member who changes their mind.
pub fn unvote(conn: &Connection, round: i64, user: u64, choice: &str) -> rusqlite::Result<bool> {
    conn.execute("DELETE FROM votes WHERE round_id = ?1 AND user_id = ?2 AND choice = ?3", params![round, user as i64, choice])
        .map(|n| n > 0)
}

pub fn voted(conn: &Connection, round: i64, user: u64, choice: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM votes WHERE round_id = ?1 AND user_id = ?2 AND choice = ?3",
        params![round, user as i64, choice],
        |_| Ok(()),
    )
    .optional()
    .ok()
    .flatten()
    .is_some()
}

pub fn tally(conn: &Connection, round: i64) -> Tally {
    let count = |choice: &str| {
        conn.query_row(
            "SELECT COUNT(DISTINCT user_id) FROM votes WHERE round_id = ?1 AND choice = ?2",
            params![round, choice],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
    };
    Tally { truth: count("truth"), dare: count("dare"), skip: count("skip") }
}

/// Who voted to skip, for the line that says the room binned it.
pub fn skippers(conn: &Connection, round: i64) -> Vec<u64> {
    let Ok(mut stmt) = conn.prepare("SELECT user_id FROM votes WHERE round_id = ?1 AND choice = 'skip' ORDER BY ts") else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map(params![round], |r| r.get::<_, i64>(0)) else { return Vec::new() };
    rows.filter_map(Result::ok).map(|v| v as u64).collect()
}

// --- the no-repeat window, and the asking cooldown ---------------------------------------------

pub fn note_asked(conn: &Connection, prompt_key: &str, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO asked (prompt_key, ts) VALUES (?1, ?2) ON CONFLICT(prompt_key) DO UPDATE SET ts = excluded.ts",
        params![prompt_key, now],
    )
    .map(|_| ())
}

/// What the channel has been asked since a moment.
pub fn asked_since(conn: &Connection, since: i64) -> HashSet<String> {
    let Ok(mut stmt) = conn.prepare("SELECT prompt_key FROM asked WHERE ts >= ?1") else { return HashSet::new() };
    let Ok(rows) = stmt.query_map(params![since], |r| r.get::<_, String>(0)) else { return HashSet::new() };
    rows.filter_map(Result::ok).collect()
}

/// When this member last put a question of their own up, for the cooldown.
pub fn last_asked_by(conn: &Connection, user: u64) -> Option<i64> {
    conn.query_row("SELECT MAX(opened_ts) FROM rounds WHERE asked_by = ?1", params![user as i64], |r| r.get::<_, Option<i64>>(0))
        .optional()
        .ok()
        .flatten()
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().expect("memory db");
        init(&conn).expect("schema");
        conn
    }

    #[test]
    fn a_round_votes_then_opens_then_ends() {
        let c = conn();
        let r = start_vote(&c, 5, 100).expect("vote");
        assert_eq!(r.status, Status::Voting);
        assert!(r.topic.is_none());
        assert!(open_with(&c, r.id, Topic::Truth, "three_am", 110).expect("open"));
        let r = get(&c, r.id).expect("row");
        assert_eq!((r.status, r.topic), (Status::Open, Some(Topic::Truth)));
        assert_eq!(r.opened_ts, Some(110));
        assert!(skip(&c, r.id, None, 200).expect("skip"));
        assert_eq!(get(&c, r.id).expect("row").status, Status::Skipped);
        assert!(live(&c).is_none());
    }

    /// Two votes landing together still put ONE question up.
    #[test]
    fn a_round_can_only_be_opened_once() {
        let c = conn();
        let r = start_vote(&c, 5, 100).expect("vote");
        assert!(open_with(&c, r.id, Topic::Truth, "three_am", 110).expect("first"));
        assert!(!open_with(&c, r.id, Topic::Dare, "all_caps", 110).expect("second"));
        assert_eq!(get(&c, r.id).expect("row").prompt_key.as_deref(), Some("three_am"));
    }

    /// And three skips landing together end it once.
    #[test]
    fn a_round_can_only_be_ended_once() {
        let c = conn();
        let r = start_vote(&c, 5, 100).expect("vote");
        open_with(&c, r.id, Topic::Truth, "three_am", 110).expect("open");
        assert!(skip(&c, r.id, None, 200).expect("first"));
        assert!(!skip(&c, r.id, None, 200).expect("second"));
        assert!(!expire(&c, r.id, 300).expect("expire after skip"));
        assert_eq!(get(&c, r.id).expect("row").status, Status::Skipped);
    }

    #[test]
    fn a_tally_counts_people_not_presses() {
        let c = conn();
        let r = start_vote(&c, 5, 100).expect("vote");
        assert!(vote(&c, r.id, 7, "truth", 101).expect("vote"));
        assert!(!vote(&c, r.id, 7, "truth", 102).expect("again"), "pressing twice is pressing once");
        vote(&c, r.id, 8, "truth", 103).expect("vote");
        vote(&c, r.id, 9, "dare", 104).expect("vote");
        assert_eq!(tally(&c, r.id), Tally { truth: 2, dare: 1, skip: 0 });
        assert!(voted(&c, r.id, 7, "truth") && !voted(&c, r.id, 7, "dare"));
        assert!(unvote(&c, r.id, 7, "truth").expect("unvote"));
        assert_eq!(tally(&c, r.id).truth, 1);
    }

    /// A truth vote and a skip vote are votes on different questions, so one
    /// person may hold both at once.
    #[test]
    fn one_person_may_want_a_truth_and_then_want_rid_of_it() {
        let c = conn();
        let r = start_vote(&c, 5, 100).expect("vote");
        vote(&c, r.id, 7, "truth", 101).expect("vote");
        vote(&c, r.id, 7, "skip", 150).expect("vote");
        assert_eq!(tally(&c, r.id), Tally { truth: 1, dare: 0, skip: 1 });
        assert_eq!(skippers(&c, r.id), vec![7]);
    }

    #[test]
    fn a_members_own_question_needs_no_vote() {
        let c = conn();
        let r = start_vote(&c, 5, 100).expect("vote");
        assert!(open_asked(&c, r.id, "what did you have for breakfast", 7, 110).expect("asked"));
        let r = get(&c, r.id).expect("row");
        assert_eq!(r.topic, Some(Topic::Asked));
        assert_eq!(r.asked_by, Some(7));
        assert_eq!(r.text.as_deref(), Some("what did you have for breakfast"));
        assert!(r.prompt_key.is_none(), "a member's question is not a bank one");
        assert_eq!(last_asked_by(&c, 7), Some(110));
        assert_eq!(last_asked_by(&c, 8), None);
    }

    #[test]
    fn answers_are_counted_and_the_first_one_is_remembered() {
        let c = conn();
        let r = start_vote(&c, 5, 100).expect("vote");
        open_with(&c, r.id, Topic::Truth, "three_am", 110).expect("open");
        assert!(note_answer(&c, r.id, 900, 120).expect("first"));
        assert!(note_answer(&c, r.id, 901, 130).expect("second"));
        let r = get(&c, r.id).expect("row");
        assert_eq!((r.answers, r.first_answer), (2, Some(900)));
        assert_eq!(r.touched_ts(), 130, "the idle clock runs from the last answer");
        skip(&c, r.id, None, 200).expect("skip");
        assert!(!note_answer(&c, r.id, 902, 210).expect("after the end"), "a closed round takes no more");
    }

    /// The window is the channel's, not any one person's.
    #[test]
    fn the_no_repeat_window_is_the_whole_channels() {
        let c = conn();
        let r = start_vote(&c, 5, 100).expect("vote");
        open_with(&c, r.id, Topic::Dare, "all_caps", 110).expect("open");
        assert!(asked_since(&c, 0).contains("all_caps"));
        assert!(asked_since(&c, 200).is_empty(), "the window is a window");
    }

    #[test]
    fn voting_rounds_are_not_history() {
        let c = conn();
        let a = start_vote(&c, 5, 100).expect("vote");
        open_with(&c, a.id, Topic::Truth, "three_am", 110).expect("open");
        skip(&c, a.id, None, 120).expect("skip");
        start_vote(&c, 5, 130).expect("vote");
        let rows = recent(&c, 10);
        assert_eq!(rows.len(), 1, "the round still being voted on is not a past one");
        assert_eq!(rows[0].id, a.id);
    }
}
