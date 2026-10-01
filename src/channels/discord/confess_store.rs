//! What the confessions feature keeps, in its own file in the runtime
//! directory, `confessions.db`, so losing it never touches anything else — and
//! so the one table holding people's worst moments is nowhere near the message
//! log the AI reads.
//!
//! Two tables. One row per submission: its number in the public series, what it
//! says, who sent it, whether a mod approved or rejected it and who that mod
//! was, and where the approved copy was posted. And one row per channel saying
//! which confession card is currently carrying the two buttons.
//!
//! That second table, with `posted_message` on the first, is the only way the
//! bot ever learns which message it may edit. It is matched by a message id the
//! bot wrote down itself — never by author, never by text — so the
//! third-party bot's own messages and the hundreds of confessions already in
//! that channel are invisible to it and cannot be touched.
//!
//! The record is the point: a mod has to be able to answer "who sent #457" a
//! month later, and a rejected confession has to be provably never posted. Both
//! are only true if the row is on disk before anything is sent to Discord.

use std::path::Path;
use std::sync::OnceLock;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

pub(crate) const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS confessions (
        number INTEGER PRIMARY KEY,
        kind TEXT NOT NULL,
        answers INTEGER,
        body TEXT NOT NULL,
        user_id INTEGER NOT NULL,
        user_name TEXT NOT NULL DEFAULT '',
        by_mod INTEGER NOT NULL DEFAULT 0,
        created_ts INTEGER NOT NULL,
        status TEXT NOT NULL DEFAULT 'pending',
        decided_by INTEGER NOT NULL DEFAULT 0,
        decided_ts INTEGER NOT NULL DEFAULT 0,
        reason TEXT NOT NULL DEFAULT '',
        review_message INTEGER NOT NULL DEFAULT 0,
        posted_channel INTEGER NOT NULL DEFAULT 0,
        posted_message INTEGER NOT NULL DEFAULT 0,
        thread_id INTEGER NOT NULL DEFAULT 0);
    CREATE INDEX IF NOT EXISTS confessions_by_user ON confessions (user_id, created_ts);
    CREATE INDEX IF NOT EXISTS confessions_by_status ON confessions (status, created_ts);
    CREATE TABLE IF NOT EXISTS buttons (
        channel_id INTEGER PRIMARY KEY, message_id INTEGER NOT NULL,
        number INTEGER NOT NULL DEFAULT 0, posted_ts INTEGER NOT NULL);
";

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

/// The shared connection. Hold the lock only for a query, never across an `.await`.
pub fn db() -> Option<&'static Mutex<Connection>> {
    DB.get()
}

/// A confession, or a reply to one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Confession,
    Reply,
}

impl Kind {
    pub fn key(self) -> &'static str {
        match self {
            Kind::Confession => "confession",
            Kind::Reply => "reply",
        }
    }

    pub fn from_key(raw: &str) -> Option<Self> {
        match raw {
            "confession" => Some(Kind::Confession),
            "reply" => Some(Kind::Reply),
            _ => None,
        }
    }
}

/// Where one submission stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Approved,
    Rejected,
}

impl Status {
    pub fn key(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Approved => "approved",
            Status::Rejected => "rejected",
        }
    }

    pub fn from_key(raw: &str) -> Option<Self> {
        match raw {
            "pending" => Some(Status::Pending),
            "approved" => Some(Status::Approved),
            "rejected" => Some(Status::Rejected),
            _ => None,
        }
    }
}

/// One submission, as it stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Confession {
    /// Its place in the public series. Handed out when it is submitted, so a
    /// rejection still has a number for the log — the public series therefore
    /// has gaps, and each gap is something a mod refused.
    pub number: i64,
    pub kind: Kind,
    /// The confession a reply answers.
    pub answers: Option<i64>,
    pub body: String,
    pub user_id: u64,
    pub user_name: String,
    /// Whether the submitter had moderator powers when they sent it. Shown
    /// plainly in review so a mod cannot approve their own quietly.
    pub by_mod: bool,
    pub created_ts: i64,
    pub status: Status,
    /// The mod who decided, 0 while pending.
    pub decided_by: u64,
    pub decided_ts: i64,
    /// Why it was rejected, when a reason was given.
    pub reason: String,
    /// The review embed's message, so a decision can edit its buttons away.
    pub review_message: u64,
    pub posted_channel: u64,
    pub posted_message: u64,
    /// The thread opened on the posted message, where the conversation about it
    /// happens. 0 until one is opened — and the number is how the bot knows not
    /// to open a second one.
    pub thread_id: u64,
}

impl Default for Confession {
    fn default() -> Self {
        Self {
            number: 0,
            kind: Kind::Confession,
            answers: None,
            body: String::new(),
            user_id: 0,
            user_name: String::new(),
            by_mod: false,
            created_ts: 0,
            status: Status::Pending,
            decided_by: 0,
            decided_ts: 0,
            reason: String::new(),
            review_message: 0,
            posted_channel: 0,
            posted_message: 0,
            thread_id: 0,
        }
    }
}

impl Confession {
    /// The link to the posted copy, when there is one.
    pub fn link(&self, guild: u64) -> Option<String> {
        (self.posted_message != 0)
            .then(|| format!("https://discord.com/channels/{}/{}/{}", guild, self.posted_channel, self.posted_message))
    }
}

/// What a new submission carries before it has a number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct New {
    pub kind: Kind,
    pub answers: Option<i64>,
    pub body: String,
    pub user_id: u64,
    pub user_name: String,
    pub by_mod: bool,
    pub created_ts: i64,
}

fn prepare(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)
}

/// Opens (or makes) `<workspace>/.runtime/confessions.db`. Once per process.
pub fn open(workspace: &str) -> anyhow::Result<()> {
    if DB.get().is_some() {
        return Ok(());
    }
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    open_at(&dir.join("confessions.db"))
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

// --- the submissions ---------------------------------------------------------

const COLUMNS: &str = "number, kind, answers, body, user_id, user_name, by_mod, created_ts, status, decided_by, \
                       decided_ts, reason, review_message, posted_channel, posted_message, thread_id";

pub fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Confession> {
    let kind: String = row.get(1)?;
    let status: String = row.get(8)?;
    Ok(Confession {
        number: row.get(0)?,
        kind: Kind::from_key(&kind).unwrap_or(Kind::Confession),
        answers: row.get(2)?,
        body: row.get(3)?,
        user_id: row.get::<_, i64>(4)? as u64,
        user_name: row.get(5)?,
        by_mod: row.get::<_, i64>(6)? != 0,
        created_ts: row.get(7)?,
        status: Status::from_key(&status).unwrap_or(Status::Pending),
        decided_by: row.get::<_, i64>(9)? as u64,
        decided_ts: row.get(10)?,
        reason: row.get(11)?,
        review_message: row.get::<_, i64>(12)? as u64,
        posted_channel: row.get::<_, i64>(13)? as u64,
        posted_message: row.get::<_, i64>(14)? as u64,
        thread_id: row.get::<_, i64>(15)? as u64,
    })
}

/// The number the next submission takes: one past the highest ever handed out,
/// or the seed from the setting when that is higher. The seed is how the series
/// was told where the old bot had got to; once the store is ahead of it, the
/// store wins, so lowering the setting can never hand out a number twice.
pub fn next_number(conn: &Connection, seed: i64) -> i64 {
    let highest: i64 = conn.query_row("SELECT COALESCE(MAX(number), 0) FROM confessions", [], |r| r.get(0)).unwrap_or(0);
    (highest + 1).max(seed.max(1))
}

/// Writes a submission down and gives it its number. The number is read and
/// claimed inside one transaction, so two submissions landing together cannot
/// take the same one.
pub fn add(conn: &mut Connection, new: &New, seed: i64) -> rusqlite::Result<Confession> {
    let tx = conn.transaction()?;
    let number = next_number(&tx, seed);
    tx.execute(
        "INSERT INTO confessions (number, kind, answers, body, user_id, user_name, by_mod, created_ts, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending')",
        params![
            number,
            new.kind.key(),
            new.answers,
            new.body,
            new.user_id as i64,
            new.user_name,
            i64::from(new.by_mod),
            new.created_ts,
        ],
    )?;
    tx.commit()?;
    Ok(Confession {
        number,
        kind: new.kind,
        answers: new.answers,
        body: new.body.clone(),
        user_id: new.user_id,
        user_name: new.user_name.clone(),
        by_mod: new.by_mod,
        created_ts: new.created_ts,
        ..Confession::default()
    })
}

/// One submission by its number.
pub fn get(conn: &Connection, number: i64) -> rusqlite::Result<Option<Confession>> {
    conn.query_row(&format!("SELECT {} FROM confessions WHERE number = ?1", COLUMNS), params![number], read_row).optional()
}

/// Notes the review embed's message, so a decision can take its buttons off.
pub fn set_review_message(conn: &Connection, number: i64, message: u64) -> rusqlite::Result<()> {
    conn.execute("UPDATE confessions SET review_message = ?2 WHERE number = ?1", params![number, message as i64])?;
    Ok(())
}

/// Records a decision, and refuses one that has already been made. The `false`
/// is what stops two mods pressing Approve at the same time posting it twice.
pub fn decide(conn: &Connection, number: i64, status: Status, by: u64, ts: i64, reason: &str) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE confessions SET status = ?2, decided_by = ?3, decided_ts = ?4, reason = ?5
         WHERE number = ?1 AND status = 'pending'",
        params![number, status.key(), by as i64, ts, reason],
    )?;
    Ok(n == 1)
}

/// The thread opened on the posted message. Written down so a second pass
/// never opens a second thread on the same confession.
pub fn set_thread(conn: &Connection, number: i64, thread: u64) -> rusqlite::Result<()> {
    conn.execute("UPDATE confessions SET thread_id = ?2 WHERE number = ?1", params![number, thread as i64])?;
    Ok(())
}

/// Where the approved copy went. Only ever called for an approved one.
pub fn set_posted(conn: &Connection, number: i64, channel: u64, message: u64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE confessions SET posted_channel = ?2, posted_message = ?3 WHERE number = ?1",
        params![number, channel as i64, message as i64],
    )?;
    Ok(())
}

/// How many approved replies a confession already has. One more than this is
/// the next reply's letter inside its thread, so members can refer to one.
pub fn approved_replies_to(conn: &Connection, target: i64) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM confessions WHERE kind = 'reply' AND answers = ?1 AND status = 'approved'",
        params![target],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
}

/// How many of this member's submissions have been approved, and how many
/// rejected — the two numbers the review embed shows a mod.
pub fn tally(conn: &Connection, user: u64) -> (i64, i64) {
    let one = |status: Status| {
        conn.query_row(
            "SELECT COUNT(*) FROM confessions WHERE user_id = ?1 AND status = ?2",
            params![user as i64, status.key()],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
    };
    (one(Status::Approved), one(Status::Rejected))
}

/// When this member last submitted anything, for the cooldown.
pub fn last_from(conn: &Connection, user: u64) -> Option<i64> {
    conn.query_row(
        "SELECT MAX(created_ts) FROM confessions WHERE user_id = ?1",
        params![user as i64],
        |r| r.get::<_, Option<i64>>(0),
    )
    .ok()
    .flatten()
}

/// Pending, approved, rejected.
pub fn counts(conn: &Connection) -> (i64, i64, i64) {
    let one = |status: Status| {
        conn.query_row("SELECT COUNT(*) FROM confessions WHERE status = ?1", params![status.key()], |r| r.get::<_, i64>(0))
            .unwrap_or(0)
    };
    (one(Status::Pending), one(Status::Approved), one(Status::Rejected))
}

/// What the panel page asks for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    pub status: Option<Status>,
    /// Matched against the text, the submitter's name and the number.
    pub q: Option<String>,
    pub user: Option<u64>,
    pub limit: usize,
}

/// The submissions the filter asks for, newest first.
pub fn list(conn: &Connection, filter: &Filter) -> rusqlite::Result<Vec<Confession>> {
    let limit = filter.limit.clamp(1, 500) as i64;
    let like = filter.q.as_deref().map(|q| format!("%{}%", q.trim()));
    // The number is matched exactly when the search is a number, so typing 457
    // finds #457 rather than everything containing "457".
    let number = filter.q.as_deref().and_then(|q| q.trim().trim_start_matches('#').parse::<i64>().ok());
    let mut stmt = conn.prepare(&format!(
        "SELECT {} FROM confessions
         WHERE (?1 IS NULL OR status = ?1)
           AND (?2 IS NULL OR user_id = ?2)
           AND (?3 IS NULL OR body LIKE ?3 OR user_name LIKE ?3 OR number = ?4)
         ORDER BY number DESC LIMIT ?5",
        COLUMNS
    ))?;
    let rows = stmt.query_map(
        params![filter.status.map(|s| s.key()), filter.user.map(|u| u as i64), like, number, limit],
        read_row,
    )?;
    rows.collect()
}

// --- which card carries the buttons ------------------------------------------

/// The confession card currently carrying the two buttons in that channel, as
/// (message id, confession number).
///
/// This, and `posted_message` on a confession, are the only message ids this
/// bot will ever edit. Both are ids it wrote down itself when it posted the
/// message. Nothing is matched on author or on content, so the bot this
/// replaces — whose own cards and panels are still in that channel — is
/// invisible here.
pub fn buttons_holder(conn: &Connection, channel: u64) -> Option<(u64, i64)> {
    conn.query_row("SELECT message_id, number FROM buttons WHERE channel_id = ?1", params![channel as i64], |r| {
        Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)?))
    })
    .optional()
    .ok()
    .flatten()
    .filter(|(id, _)| *id != 0)
}

pub fn set_buttons_holder(conn: &Connection, channel: u64, message: u64, number: i64, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO buttons (channel_id, message_id, number, posted_ts) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(channel_id) DO UPDATE SET message_id = excluded.message_id, number = excluded.number,
         posted_ts = excluded.posted_ts",
        params![channel as i64, message as i64, number, now],
    )?;
    Ok(())
}

/// Forgets which card carries the buttons — after the card has gone, so the
/// next pass looks for the newest surviving one instead of editing a message
/// that is not there.
pub fn clear_buttons_holder(conn: &Connection, channel: u64) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM buttons WHERE channel_id = ?1", params![channel as i64])?;
    Ok(())
}

/// The newest confession card this bot still believes is in that channel, as
/// (number, message id). Replies are never cards: they live in threads.
pub fn newest_card(conn: &Connection, channel: u64) -> Option<(i64, u64)> {
    conn.query_row(
        "SELECT number, posted_message FROM confessions
         WHERE kind = 'confession' AND status = 'approved' AND posted_channel = ?1 AND posted_message != 0
         ORDER BY number DESC LIMIT 1",
        params![channel as i64],
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? as u64)),
    )
    .optional()
    .ok()
    .flatten()
}

/// One confession's card has gone from the channel: somebody deleted it. The
/// text and who sent it are kept — this only forgets where the copy was, so
/// the card is no longer offered as somewhere to put the buttons.
pub fn forget_posted(conn: &Connection, number: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE confessions SET posted_message = 0 WHERE number = ?1", params![number])?;
    Ok(())
}

/// Which confession, if any, was posted as this message in this channel. How a
/// deleted message is recognised as one of ours — by an id out of this table
/// and nothing else.
pub fn card_at(conn: &Connection, channel: u64, message: u64) -> Option<i64> {
    conn.query_row(
        "SELECT number FROM confessions WHERE posted_channel = ?1 AND posted_message = ?2",
        params![channel as i64, message as i64],
        |r| r.get::<_, i64>(0),
    )
    .optional()
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new(user: u64, body: &str, ts: i64) -> New {
        New {
            kind: Kind::Confession,
            answers: None,
            body: body.into(),
            user_id: user,
            user_name: "Zoya".into(),
            by_mod: false,
            created_ts: ts,
        }
    }

    #[test]
    fn the_series_starts_at_the_seed_and_climbs_from_there() {
        let mut conn = open_memory().unwrap();
        assert_eq!(next_number(&conn, 459), 459, "an empty store takes the number the setting seeded");
        let first = add(&mut conn, &new(11, "I cheated at Wordle", 100), 459).unwrap();
        assert_eq!(first.number, 459);
        let second = add(&mut conn, &new(12, "me too", 200), 459).unwrap();
        assert_eq!(second.number, 460, "the next one is the next number");
        // Lowering the seed afterwards can never hand a number out twice.
        assert_eq!(next_number(&conn, 1), 461);
        assert_eq!(next_number(&conn, 10_000), 10_000, "but raising it skips the series forward");
    }

    #[test]
    fn a_submission_comes_back_whole() {
        let mut conn = open_memory().unwrap();
        let c = add(&mut conn, &new(11, "the text", 100), 459).unwrap();
        let back = get(&conn, 459).unwrap().unwrap();
        assert_eq!(back, c);
        assert_eq!((back.status, back.decided_by, back.posted_message), (Status::Pending, 0, 0));
        assert_eq!(get(&conn, 1).unwrap(), None);
    }

    /// Two mods pressing Approve together must not post the same confession
    /// twice: the second write is refused.
    #[test]
    fn a_decision_is_made_once_and_only_once() {
        let mut conn = open_memory().unwrap();
        add(&mut conn, &new(11, "the text", 100), 459).unwrap();
        assert!(decide(&conn, 459, Status::Approved, 7, 500, "").unwrap());
        assert!(!decide(&conn, 459, Status::Approved, 8, 600, "").unwrap(), "the second press does nothing");
        assert!(!decide(&conn, 459, Status::Rejected, 8, 600, "late").unwrap());
        let c = get(&conn, 459).unwrap().unwrap();
        assert_eq!((c.status, c.decided_by, c.decided_ts), (Status::Approved, 7, 500));
        assert_eq!(c.reason, "", "nothing from the losing press got in");
    }

    #[test]
    fn a_rejection_keeps_its_reason_and_is_never_posted() {
        let mut conn = open_memory().unwrap();
        add(&mut conn, &new(11, "a name and a phone number", 100), 459).unwrap();
        assert!(decide(&conn, 459, Status::Rejected, 7, 500, "doxxing").unwrap());
        let c = get(&conn, 459).unwrap().unwrap();
        assert_eq!((c.status, c.reason.as_str()), (Status::Rejected, "doxxing"));
        assert_eq!((c.posted_channel, c.posted_message), (0, 0), "nothing public was ever written down");
        assert_eq!(c.link(900), None);
    }

    #[test]
    fn the_counts_and_the_tally_split_the_three_states() {
        let mut conn = open_memory().unwrap();
        for (i, user) in [(0, 11), (1, 11), (2, 12), (3, 11)] {
            add(&mut conn, &new(user, "x", 100 + i), 459).unwrap();
        }
        decide(&conn, 459, Status::Approved, 7, 500, "").unwrap();
        decide(&conn, 460, Status::Rejected, 7, 500, "no").unwrap();
        decide(&conn, 461, Status::Approved, 7, 500, "").unwrap();
        assert_eq!(counts(&conn), (1, 2, 1));
        assert_eq!(tally(&conn, 11), (1, 1), "one approved, one rejected, one still waiting");
        assert_eq!(tally(&conn, 12), (1, 0));
        assert_eq!(tally(&conn, 99), (0, 0));
        assert_eq!(last_from(&conn, 11), Some(103));
        assert_eq!(last_from(&conn, 99), None);
    }

    /// A reply's letter inside the thread comes from how many are already
    /// there, so two replies never answer to the same letter.
    #[test]
    fn approved_replies_are_counted_per_confession() {
        let mut conn = open_memory().unwrap();
        add(&mut conn, &new(11, "the confession", 100), 457).unwrap();
        let reply = |user: u64, target: i64, ts: i64| New {
            kind: Kind::Reply,
            answers: Some(target),
            body: "an answer".into(),
            user_id: user,
            user_name: "Kabir".into(),
            by_mod: false,
            created_ts: ts,
        };
        add(&mut conn, &reply(12, 457, 200), 457).unwrap();
        add(&mut conn, &reply(13, 457, 300), 457).unwrap();
        add(&mut conn, &reply(14, 999, 400), 457).unwrap();
        assert_eq!(approved_replies_to(&conn, 457), 0, "nothing counts until it is approved");
        decide(&conn, 458, Status::Approved, 7, 500, "").unwrap();
        assert_eq!(approved_replies_to(&conn, 457), 1);
        decide(&conn, 459, Status::Rejected, 7, 500, "no").unwrap();
        assert_eq!(approved_replies_to(&conn, 457), 1, "a rejected reply never takes a letter");
        decide(&conn, 460, Status::Approved, 7, 500, "").unwrap();
        assert_eq!(approved_replies_to(&conn, 457), 1, "and another confession's replies are its own");
        assert_eq!(approved_replies_to(&conn, 999), 1);
    }

    #[test]
    fn the_list_searches_the_text_the_name_and_the_number() {
        let mut conn = open_memory().unwrap();
        add(&mut conn, &new(11, "I cheated at Wordle", 100), 459).unwrap();
        let mut second = new(12, "I have never seen Star Wars", 200);
        second.user_name = "Kabir".into();
        add(&mut conn, &second, 459).unwrap();
        decide(&conn, 459, Status::Approved, 7, 500, "").unwrap();

        let all = |f: Filter| list(&conn, &f).unwrap().iter().map(|c| c.number).collect::<Vec<_>>();
        assert_eq!(all(Filter { limit: 50, ..Filter::default() }), vec![460, 459], "newest first");
        assert_eq!(all(Filter { status: Some(Status::Approved), limit: 50, ..Filter::default() }), vec![459]);
        assert_eq!(all(Filter { status: Some(Status::Pending), limit: 50, ..Filter::default() }), vec![460]);
        assert_eq!(all(Filter { q: Some("wordle".into()), limit: 50, ..Filter::default() }), vec![459], "case does not matter");
        assert_eq!(all(Filter { q: Some("Kabir".into()), limit: 50, ..Filter::default() }), vec![460]);
        assert_eq!(all(Filter { q: Some("#460".into()), limit: 50, ..Filter::default() }), vec![460], "a number finds that one");
        assert_eq!(all(Filter { user: Some(12), limit: 50, ..Filter::default() }), vec![460]);
        assert!(all(Filter { q: Some("nothing like this".into()), limit: 50, ..Filter::default() }).is_empty());
    }

    /// The thread is written down so the conversation can be found again, and
    /// so a second pass never opens a second one.
    #[test]
    fn the_thread_on_a_posted_confession_is_remembered_once() {
        let mut conn = open_memory().unwrap();
        add(&mut conn, &new(11, "the text", 100), 459).unwrap();
        assert_eq!(get(&conn, 459).unwrap().unwrap().thread_id, 0, "no thread until one is opened");
        decide(&conn, 459, Status::Approved, 7, 500, "").unwrap();
        set_posted(&conn, 459, 21, 9001).unwrap();
        set_thread(&conn, 459, 7777).unwrap();
        assert_eq!(get(&conn, 459).unwrap().unwrap().thread_id, 7777);
    }

    /// The safety rail: the bot can only ever learn about a message it wrote
    /// down itself.
    #[test]
    fn only_a_card_this_bot_posted_is_ever_remembered() {
        let conn = open_memory().unwrap();
        assert_eq!(buttons_holder(&conn, 77), None, "nothing to edit in a channel we have never posted in");
        set_buttons_holder(&conn, 77, 4242, 459, 100).unwrap();
        assert_eq!(buttons_holder(&conn, 77), Some((4242, 459)));
        assert_eq!(buttons_holder(&conn, 78), None, "and nothing in any other channel");
        set_buttons_holder(&conn, 77, 4243, 460, 200).unwrap();
        assert_eq!(buttons_holder(&conn, 77), Some((4243, 460)), "the newest card takes them over");
        clear_buttons_holder(&conn, 77).unwrap();
        assert_eq!(buttons_holder(&conn, 77), None);
        // A message id nothing wrote down is not one of ours, whoever posted it.
        assert_eq!(card_at(&conn, 77, 4242), None, "a card has to be in the confessions table to be ours");
    }

    /// Where the buttons go when the newest card is lost: the newest surviving
    /// card this bot posted, and never a reply or anything still pending.
    #[test]
    fn the_newest_surviving_card_is_the_one_the_buttons_can_move_to() {
        let mut conn = open_memory().unwrap();
        assert_eq!(newest_card(&conn, 21), None);
        for (i, body) in ["one", "two", "three"].iter().enumerate() {
            add(&mut conn, &new(11, body, 100 + i as i64), 459).unwrap();
            let n = 459 + i as i64;
            decide(&conn, n, Status::Approved, 7, 200, "").unwrap();
            set_posted(&conn, n, 21, 9000 + n as u64);
        }
        assert_eq!(newest_card(&conn, 21), Some((461, 9461)), "the newest of the three");
        assert_eq!(card_at(&conn, 21, 9461), Some(461));
        assert_eq!(newest_card(&conn, 99), None, "and only in the channel asked about");

        // A reply, approved and posted inside a thread, is not a card.
        add(&mut conn, &New { kind: Kind::Reply, answers: Some(461), ..new(12, "an answer", 400) }, 459).unwrap();
        decide(&conn, 462, Status::Approved, 7, 500, "").unwrap();
        set_posted(&conn, 462, 7777, 9462);
        assert_eq!(newest_card(&conn, 21), Some((461, 9461)), "a reply never carries the buttons");

        // Somebody deletes the newest card: the buttons move down one.
        forget_posted(&conn, 461).unwrap();
        assert_eq!(newest_card(&conn, 21), Some((460, 9460)));
        assert_eq!(card_at(&conn, 21, 9461), None, "the deleted card is not ours to edit any more");
        // The text and who sent it are kept: only the copy is forgotten.
        let c = get(&conn, 461).unwrap().unwrap();
        assert_eq!((c.user_id, c.status, c.body.as_str()), (11, Status::Approved, "three"));
        // All of them gone: nowhere for the buttons, and that is not an error.
        forget_posted(&conn, 460).unwrap();
        forget_posted(&conn, 459).unwrap();
        assert_eq!(newest_card(&conn, 21), None);
    }

    /// A restart must lose neither the series nor the record of who sent what.
    #[test]
    fn the_record_is_on_disk_and_comes_back_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/confessions.db");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        {
            let mut conn = Connection::open(&path).unwrap();
            prepare(&conn).unwrap();
            add(&mut conn, &new(11, "the first one", 100), 459).unwrap();
            add(&mut conn, &new(12, "the second one", 200), 459).unwrap();
            decide(&conn, 459, Status::Approved, 7, 300, "").unwrap();
            set_posted(&conn, 459, 21, 9001).unwrap();
            set_thread(&conn, 459, 7777).unwrap();
            set_buttons_holder(&conn, 21, 9001, 459, 300).unwrap();
        }
        // A second process opening the same file.
        let conn = Connection::open(&path).unwrap();
        prepare(&conn).unwrap();
        assert_eq!(next_number(&conn, 459), 461, "the series carries on where it stopped, not back at the seed");
        let c = get(&conn, 459).unwrap().unwrap();
        assert_eq!((c.user_id, c.status, c.posted_message), (11, Status::Approved, 9001));
        assert_eq!(c.link(900).as_deref(), Some("https://discord.com/channels/900/21/9001"));
        assert_eq!(c.thread_id, 7777, "and the thread its conversation is in");
        assert_eq!(buttons_holder(&conn, 21), Some((9001, 459)), "and the card carrying the buttons is still known");
    }
}
