//! What Guess the Word keeps in `.runtime/guess.db`: every doodle the bot has
//! put up, and every guess that named one.
//!
//! A round moves `open` → `solved` when somebody types what it is, or
//! `open` → `skipped` when a player skips it after a hint (or a mod skips it
//! outright), or `open` → `expired` when nobody got it all evening. Only the
//! FIRST correct guess wins: the move to `solved` is one
//! `UPDATE … WHERE status = 'open'`, so two guesses landing in the same second
//! can only make one winner.
//!
//! Every round keeps the word in its plain form and which drawings of it were
//! shown - the one on the card, and the second one a hint puts beside it - so
//! neither the word nor any single picture comes round again inside the
//! no-repeat window. Who asked for the hint and who pressed on are written down
//! too: a hint costs the round a point, so it has to be attributable. A day's
//! winners are a table of their own, ready for a `/today` summary or a panel
//! page.

use std::collections::HashSet;
use std::sync::OnceLock;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

pub const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS rounds (
        id INTEGER PRIMARY KEY AUTOINCREMENT, word TEXT NOT NULL, word_key TEXT NOT NULL,
        doodle INTEGER NOT NULL, hint_doodle INTEGER, points INTEGER NOT NULL, posted_ts INTEGER NOT NULL,
        message_id INTEGER, channel_id INTEGER NOT NULL, status TEXT NOT NULL DEFAULT 'open',
        winner INTEGER, winning_guess TEXT, solved_ts INTEGER, seconds INTEGER,
        hint_by INTEGER, hint_ts INTEGER, ended_by INTEGER, ended_ts INTEGER);
    CREATE INDEX IF NOT EXISTS rounds_status ON rounds (status);
    CREATE INDEX IF NOT EXISTS rounds_word ON rounds (word_key, posted_ts);
    CREATE TABLE IF NOT EXISTS solves (
        day TEXT NOT NULL, user_id INTEGER NOT NULL, round_id INTEGER NOT NULL,
        points INTEGER NOT NULL DEFAULT 0, worth INTEGER NOT NULL DEFAULT 0, guess TEXT NOT NULL DEFAULT '',
        seconds INTEGER NOT NULL DEFAULT 0, ts INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (user_id, round_id));
    CREATE INDEX IF NOT EXISTS solves_day ON solves (day);
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);";

/// Columns added after the first release, for a database made before them. The
/// game is live, so they go on with `ALTER TABLE`: nothing is ever rewritten or
/// dropped under a running round.
const ADDED_COLUMNS: &[(&str, &str, &str)] = &[("solves", "worth", "INTEGER NOT NULL DEFAULT 0")];

/// What the solves written before `worth` existed were worth, worked out from
/// the round each was for: the round's value, a point off it if the hint had
/// gone, exactly as `worth_after_hint` figures it. Rows whose round has gone
/// keep the zero, which is all that can honestly be said about them. Run once,
/// the moment the column is added.
const BACKFILL_WORTH: &str = "
    UPDATE solves SET worth = (
        SELECT CASE WHEN r.hint_by IS NULL THEN MAX(r.points, 0) ELSE MAX(r.points - 1, 1) END
        FROM rounds r WHERE r.id = solves.round_id)
    WHERE worth = 0 AND round_id IN (SELECT id FROM rounds);";

pub fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)?;
    for (table, column, kind) in ADDED_COLUMNS {
        let has: bool = conn
            .prepare(&format!("PRAGMA table_info({})", table))?
            .query_map([], |r| r.get::<_, String>(1))?
            .flatten()
            .any(|name| name == *column);
        if has {
            continue;
        }
        conn.execute_batch(&format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, kind))?;
        if *column == "worth" {
            conn.execute_batch(BACKFILL_WORTH)?;
        }
    }
    Ok(())
}

pub fn open(workspace: &str) -> anyhow::Result<()> {
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    std::fs::create_dir_all(&dir)?;
    let conn = Connection::open(dir.join("guess.db"))?;
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
    /// Up in the channel, nobody has named it.
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
    /// The word as it is announced: "police car".
    pub word: String,
    /// Its plain form, which is what the bank is looked up by and what the
    /// no-repeat window is made of.
    pub word_key: String,
    /// Which drawing of it is on the card.
    pub doodle: i64,
    /// And the second one a hint put beside it.
    pub hint_doodle: Option<i64>,
    /// What the round was worth when it went up, before any hint.
    pub points: i64,
    pub posted_ts: i64,
    pub message: Option<u64>,
    pub channel: u64,
    pub status: Status,
    pub winner: Option<u64>,
    /// What the winner typed, which need not be the word the bot announces.
    pub winning_guess: Option<String>,
    pub solved_ts: Option<i64>,
    pub seconds: Option<i64>,
    /// Who asked for the hint, and when.
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

    /// The drawings on the card now: one, or two once a hint is out. A word
    /// with only the one drawing to its name still gets its hint - the letter
    /// and the point off - but the card shows what it has.
    pub fn shown(&self) -> Vec<i64> {
        match self.hint_doodle.filter(|second| *second != self.doodle) {
            Some(second) => vec![self.doodle, second],
            None => vec![self.doodle],
        }
    }
}

const COLS: &str = "id, word, word_key, doodle, hint_doodle, points, posted_ts, message_id, channel_id, status, winner, winning_guess, \
                    solved_ts, seconds, hint_by, hint_ts, ended_by, ended_ts";

fn row(r: &rusqlite::Row) -> rusqlite::Result<Row> {
    let user = |i: usize| -> rusqlite::Result<Option<u64>> { Ok(r.get::<_, Option<i64>>(i)?.map(|v| v as u64)) };
    Ok(Row {
        id: r.get(0)?,
        word: r.get(1)?,
        word_key: r.get(2)?,
        doodle: r.get(3)?,
        hint_doodle: r.get(4)?,
        points: r.get(5)?,
        posted_ts: r.get(6)?,
        message: user(7)?,
        channel: r.get::<_, i64>(8)? as u64,
        status: Status::from_key(&r.get::<_, String>(9)?),
        winner: user(10)?,
        winning_guess: r.get(11)?,
        solved_ts: r.get(12)?,
        seconds: r.get(13)?,
        hint_by: user(14)?,
        hint_ts: r.get(15)?,
        ended_by: user(16)?,
        ended_ts: r.get(17)?,
    })
}

/// Writes a fresh round and hands it back with its number.
pub fn add_round(conn: &Connection, word: &str, word_key: &str, doodle: i64, points: i64, channel: u64, now: i64) -> rusqlite::Result<Row> {
    conn.execute(
        "INSERT INTO rounds (word, word_key, doodle, points, posted_ts, channel_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![word, word_key, doodle, points, now, channel as i64],
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

/// Every word drawn since `since`: what the no-repeat window is made of.
pub fn words_since(conn: &Connection, since: i64) -> HashSet<String> {
    let Ok(mut stmt) = conn.prepare("SELECT DISTINCT word_key FROM rounds WHERE posted_ts >= ?1") else {
        return HashSet::new();
    };
    stmt.query_map(params![since], |r| r.get::<_, String>(0)).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

/// Every drawing of one word that has been shown since `since` - the cards' own
/// and the ones hints put beside them - so no picture is ever seen twice inside
/// the window.
pub fn doodles_since(conn: &Connection, word_key: &str, since: i64) -> HashSet<i64> {
    let sql = "SELECT doodle, hint_doodle FROM rounds WHERE word_key = ?1 AND posted_ts >= ?2";
    let Ok(mut stmt) = conn.prepare(sql) else { return HashSet::new() };
    let rows = stmt.query_map(params![word_key, since], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?)));
    let Ok(rows) = rows else { return HashSet::new() };
    let mut seen = HashSet::new();
    for (doodle, hint) in rows.flatten() {
        seen.insert(doodle);
        seen.extend(hint);
    }
    seen
}

/// The one move that decides a race: `open` → `solved`, only the first wins.
/// True for the winner, false for everyone who typed it a moment later.
pub fn claim(conn: &Connection, id: i64, user: u64, guess: &str, now: i64) -> rusqlite::Result<bool> {
    conn.execute(
        "UPDATE rounds SET status = 'solved', winner = ?2, winning_guess = ?3, solved_ts = ?4,
         seconds = MAX(?4 - posted_ts, 0) WHERE id = ?1 AND status = 'open'",
        params![id, user as i64, guess, now],
    )
    .map(|n| n > 0)
}

/// The hint, which one round has exactly one of: a second drawing of the same
/// thing, and the word's first letter. True for whoever asked first.
pub fn take_hint(conn: &Connection, id: i64, user: u64, doodle: i64, now: i64) -> rusqlite::Result<bool> {
    conn.execute(
        "UPDATE rounds SET hint_by = ?2, hint_ts = ?3, hint_doodle = ?4 WHERE id = ?1 AND status = 'open' AND hint_by IS NULL",
        params![id, user as i64, now, doodle],
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

/// A round nobody named and nobody skipped, closed by the bot itself.
pub fn expire(conn: &Connection, id: i64, now: i64) -> rusqlite::Result<bool> {
    conn.execute("UPDATE rounds SET status = 'expired', ended_ts = ?2 WHERE id = ?1 AND status = 'open'", params![id, now])
        .map(|n| n > 0)
}

// --- solves ----------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Solve {
    pub user: u64,
    pub round: i64,
    /// The HOUSE points the ledger actually paid: nothing once the day's cap is
    /// full, and nothing at all for someone with no house.
    pub points: i64,
    /// The GUESS points the solve was worth: the round's value with the hint
    /// already taken off, before the cap touched it. Every solver gets these,
    /// mods and the unsorted included — they are the game's own score, not the
    /// house cup's.
    pub worth: i64,
    /// What they typed.
    pub guess: String,
    pub seconds: i64,
    pub ts: i64,
}

/// Writes a win. A player counts once per round.
#[allow(clippy::too_many_arguments)]
pub fn add_solve(
    conn: &Connection,
    day: &str,
    user: u64,
    round: i64,
    points: i64,
    worth: i64,
    guess: &str,
    seconds: i64,
    ts: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO solves (day, user_id, round_id, points, worth, guess, seconds, ts) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(user_id, round_id) DO UPDATE SET points = MAX(solves.points, excluded.points),
             worth = MAX(solves.worth, excluded.worth)",
        params![day, user as i64, round, points, worth, guess, seconds, ts],
    )
    .map(|_| ())
}

fn solve_row(r: &rusqlite::Row) -> rusqlite::Result<Solve> {
    Ok(Solve {
        user: r.get::<_, i64>(0)? as u64,
        round: r.get(1)?,
        points: r.get(2)?,
        worth: r.get(3)?,
        guess: r.get(4)?,
        seconds: r.get(5)?,
        ts: r.get(6)?,
    })
}

/// A day's wins, first one first. What a `/today` summary - or a panel page -
/// reads.
pub fn day_solves(conn: &Connection, day: &str) -> Vec<Solve> {
    let Ok(mut stmt) =
        conn.prepare("SELECT user_id, round_id, points, worth, guess, seconds, ts FROM solves WHERE day = ?1 ORDER BY ts, round_id")
    else {
        return Vec::new();
    };
    stmt.query_map(params![day], solve_row).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

// --- guess points ----------------------------------------------------------------------

/// One player's guess points over a stretch of days.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tally {
    pub user: u64,
    /// Guess points: every solve at its full value, so the house-points cap
    /// never hides one.
    pub points: i64,
    /// How many rounds they won.
    pub solves: i64,
    /// When they got to that total — their last solve of the stretch.
    pub reached: i64,
}

/// The order a board reads in, and the order the day's card is decided in: most
/// guess points, then most rounds won, then whoever got there first.
pub fn rank(rows: &mut [Tally]) {
    rows.sort_by(|a, b| b.points.cmp(&a.points).then(b.solves.cmp(&a.solves)).then(a.reached.cmp(&b.reached)).then(a.user.cmp(&b.user)));
}

/// Everyone's guess points between two days, both ends included (YYYY-MM-DD,
/// which sorts as a date), best first.
pub fn tally_between(conn: &Connection, from_day: &str, to_day: &str) -> Vec<Tally> {
    let sql = "SELECT user_id, COALESCE(SUM(worth), 0), COUNT(*), COALESCE(MAX(ts), 0)
               FROM solves WHERE day >= ?1 AND day <= ?2 GROUP BY user_id";
    let Ok(mut stmt) = conn.prepare(sql) else { return Vec::new() };
    let mut rows: Vec<Tally> = stmt
        .query_map(params![from_day, to_day], |r| {
            Ok(Tally { user: r.get::<_, i64>(0)? as u64, points: r.get(1)?, solves: r.get(2)?, reached: r.get(3)? })
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();
    rank(&mut rows);
    rows
}

/// One day's guess points, best first. What the day's frog card is decided on.
pub fn day_tally(conn: &Connection, day: &str) -> Vec<Tally> {
    tally_between(conn, day, day)
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
    use crate::channels::discord::guess_bank::plain;

    pub fn memory() -> Connection {
        let conn = Connection::open_in_memory().expect("memory db");
        init(&conn).expect("schema");
        conn
    }

    pub fn put(conn: &Connection, word: &str, doodle: i64, points: i64, now: i64) -> Row {
        add_round(conn, word, &plain(word), doodle, points, 77, now).expect("round")
    }

    #[test]
    fn only_the_first_correct_guess_claims_a_round() {
        let conn = memory();
        let r = put(&conn, "guitar", 3, 2, 1_000);
        assert_eq!(live(&conn).map(|l| l.id), Some(r.id));
        assert_eq!((r.word_key.as_str(), r.doodle, r.hint_doodle), ("guitar", 3, None));
        assert!(claim(&conn, r.id, 11, "gitar", 1_040).unwrap(), "the first gets it");
        assert!(!claim(&conn, r.id, 22, "guitar", 1_041).unwrap(), "a second later is too late");
        let after = get(&conn, r.id).unwrap();
        assert_eq!(after.status, Status::Solved);
        assert_eq!((after.winner, after.winning_guess.as_deref(), after.seconds), (Some(11), Some("gitar"), Some(40)));
        assert!(live(&conn).is_none(), "a solved round is no longer the live one");
        // And nothing can move it afterwards.
        assert!(!skip(&conn, r.id, 5, 1_100).unwrap());
        assert!(!expire(&conn, r.id, 9_000).unwrap());
        assert!(!take_hint(&conn, r.id, 5, 4, 1_100).unwrap());
    }

    #[test]
    fn a_round_has_one_hint_and_it_says_who_asked_and_what_it_showed() {
        let conn = memory();
        let r = put(&conn, "police car", 1, 2, 500);
        assert!(!get(&conn, r.id).unwrap().hinted());
        assert_eq!(r.shown(), vec![1], "one drawing until the hint");
        assert!(take_hint(&conn, r.id, 42, 7, 540).unwrap(), "the first ask gets it");
        assert!(!take_hint(&conn, r.id, 43, 9, 560).unwrap(), "and it is the only one");
        let after = get(&conn, r.id).unwrap();
        assert_eq!((after.hint_by, after.hint_ts, after.hint_doodle), (Some(42), Some(540), Some(7)));
        assert_eq!(after.first_letter(), 'P');
        assert_eq!(after.shown(), vec![1, 7], "both drawings are on the card now");
        assert!(after.hinted());
    }

    #[test]
    fn a_skipped_round_and_a_stale_one_are_told_apart_and_both_name_the_time() {
        let conn = memory();
        let one = put(&conn, "guitar", 0, 2, 100);
        assert!(skip(&conn, one.id, 7, 200).unwrap());
        let after = get(&conn, one.id).unwrap();
        assert_eq!((after.status, after.ended_by, after.ended_ts), (Status::Skipped, Some(7), Some(200)));
        assert!(!claim(&conn, one.id, 9, "guitar", 300).unwrap(), "a skipped round can't be won");
        let two = put(&conn, "ice cream", 2, 2, 400);
        assert!(expire(&conn, two.id, 3_000).unwrap());
        let after = get(&conn, two.id).unwrap();
        assert_eq!((after.status, after.ended_by, after.ended_ts), (Status::Expired, None, Some(3_000)));
        assert!(live(&conn).is_none());
        assert_eq!(recent(&conn, 5).len(), 2);
    }

    #[test]
    fn neither_a_word_nor_a_single_drawing_comes_round_again_inside_the_window() {
        let conn = memory();
        let day = 86_400;
        put(&conn, "guitar", 3, 2, 1_000);
        put(&conn, "ice cream", 5, 2, 1_000 + 10 * day);
        let two = put(&conn, "guitar", 8, 2, 1_000 + 12 * day);
        take_hint(&conn, two.id, 1, 12, 1_000 + 12 * day + 5).unwrap();
        let now = 1_000 + 20 * day;
        // A fortnight's words, and a longer window that reaches the older round.
        assert_eq!(words_since(&conn, now - 14 * day), HashSet::from(["icecream".to_string(), "guitar".to_string()]));
        assert!(words_since(&conn, now - 1).is_empty());
        // The drawings of one word, the hint's second picture among them.
        assert_eq!(doodles_since(&conn, "guitar", now - 14 * day), HashSet::from([8, 12]));
        assert_eq!(doodles_since(&conn, "guitar", 0), HashSet::from([3, 8, 12]));
        assert!(doodles_since(&conn, "icecream", now - 14 * day).contains(&5));
        assert!(doodles_since(&conn, "aeroplane", 0).is_empty(), "a word never drawn has nothing held back");
    }

    #[test]
    fn a_day_of_wins_is_kept_for_the_summaries() {
        let conn = memory();
        let one = put(&conn, "guitar", 1, 2, 10);
        let two = put(&conn, "ice cream", 2, 2, 20);
        add_solve(&conn, "2026-09-16", 1, one.id, 2, 2, "gitar", 40, 100).unwrap();
        add_solve(&conn, "2026-09-16", 2, two.id, 1, 1, "icecream", 90, 200).unwrap();
        // The same person on the same round doesn't count twice.
        add_solve(&conn, "2026-09-16", 1, one.id, 0, 2, "guitar", 40, 300).unwrap();
        let day = day_solves(&conn, "2026-09-16");
        assert_eq!(day.len(), 2);
        assert_eq!((day[0].user, day[0].points, day[0].guess.as_str()), (1, 2, "gitar"));
        assert!(solved_by(&conn, one.id, 1) && !solved_by(&conn, one.id, 2));
        assert!(day_solves(&conn, "2026-09-17").is_empty());
    }

    #[test]
    fn what_a_round_was_worth_is_kept_whatever_the_ledger_paid() {
        let conn = memory();
        let one = put(&conn, "guitar", 1, 2, 10);
        let two = put(&conn, "ice cream", 2, 3, 20);
        // A capped day: the ledger pays nothing, the game still scores it.
        add_solve(&conn, "2026-09-16", 1, one.id, 0, 2, "gitar", 40, 100).unwrap();
        // A mod, who has no house and so no ledger row at all.
        add_solve(&conn, "2026-09-16", 9, two.id, 0, 3, "icecream", 20, 120).unwrap();
        let day = day_solves(&conn, "2026-09-16");
        assert_eq!(day.iter().map(|s| (s.user, s.points, s.worth)).collect::<Vec<_>>(), vec![(1, 0, 2), (9, 0, 3)]);
        let tally = day_tally(&conn, "2026-09-16");
        assert_eq!(tally.iter().map(|t| (t.user, t.points, t.solves)).collect::<Vec<_>>(), vec![(9, 3, 1), (1, 2, 1)]);
        assert_eq!(tally[0].reached, 120);
    }

    #[test]
    fn the_board_is_points_then_rounds_then_whoever_got_there_first() {
        let conn = memory();
        let round = |n: i64, points: i64| put(&conn, "guitar", n, points, n).id;
        let (a, b, c, d, e) = (round(1, 1), round(2, 1), round(3, 3), round(4, 1), round(5, 1));
        // 1: two cheap rounds. 2: one dear one — fewer rounds, more points.
        add_solve(&conn, "2026-09-16", 1, a, 1, 1, "gitar", 5, 100).unwrap();
        add_solve(&conn, "2026-09-16", 1, b, 1, 1, "guitar", 5, 110).unwrap();
        add_solve(&conn, "2026-09-16", 2, c, 3, 3, "icecream", 5, 120).unwrap();
        // 3 ties 1 on points with fewer rounds; 4 ties both and finished later.
        add_solve(&conn, "2026-09-16", 3, d, 2, 2, "cop car", 5, 130).unwrap();
        add_solve(&conn, "2026-09-16", 4, e, 2, 2, "police car", 5, 140).unwrap();
        let tally = day_tally(&conn, "2026-09-16");
        assert_eq!(tally.iter().map(|t| (t.user, t.points, t.solves)).collect::<Vec<_>>(), vec![(2, 3, 1), (1, 2, 2), (3, 2, 1), (4, 2, 1)]);
        // A month is the same reading over a stretch of days, and a stretch with
        // nothing in it is empty rather than wrong.
        add_solve(&conn, "2026-09-02", 4, round(6, 3), 3, 3, "icecream", 5, 90).unwrap();
        let month = tally_between(&conn, "2026-09-01", "2026-09-31");
        assert_eq!(month.first().map(|t| (t.user, t.points, t.solves)), Some((4, 5, 2)));
        assert!(tally_between(&conn, "2026-08-01", "2026-08-31").is_empty());
    }

    #[test]
    fn the_worth_column_goes_onto_a_database_that_is_already_being_played_in() {
        // Exactly the schema as it shipped, rows and all: the live database.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE rounds (
                id INTEGER PRIMARY KEY AUTOINCREMENT, word TEXT NOT NULL, word_key TEXT NOT NULL,
                doodle INTEGER NOT NULL, hint_doodle INTEGER, points INTEGER NOT NULL, posted_ts INTEGER NOT NULL,
                message_id INTEGER, channel_id INTEGER NOT NULL, status TEXT NOT NULL DEFAULT 'open',
                winner INTEGER, winning_guess TEXT, solved_ts INTEGER, seconds INTEGER,
                hint_by INTEGER, hint_ts INTEGER, ended_by INTEGER, ended_ts INTEGER);
             CREATE TABLE solves (
                day TEXT NOT NULL, user_id INTEGER NOT NULL, round_id INTEGER NOT NULL,
                points INTEGER NOT NULL DEFAULT 0, guess TEXT NOT NULL DEFAULT '',
                seconds INTEGER NOT NULL DEFAULT 0, ts INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (user_id, round_id));
             CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO rounds (id, word, word_key, doodle, points, posted_ts, channel_id, status, winner, hint_by)
                VALUES (1, 'guitar', 'guitar', 3, 2, 10, 77, 'solved', 5, NULL),
                       (2, 'ice cream', 'icecream', 5, 2, 20, 77, 'solved', 6, 9),
                       (3, 'police car', 'policecar', 1, 2, 30, 77, 'solved', 7, NULL);
             INSERT INTO solves (day, user_id, round_id, points, guess, seconds, ts)
                VALUES ('2026-09-15', 5, 1, 2, 'gitar', 40, 100),
                       ('2026-09-15', 6, 2, 0, 'icecream', 20, 200),
                       ('2026-09-15', 7, 9, 2, 'cop car', 15, 300);",
        )
        .unwrap();
        init(&conn).unwrap();
        // Nothing was dropped or rewritten.
        let kept: i64 = conn.query_row("SELECT COUNT(*) FROM solves", [], |r| r.get(0)).unwrap();
        assert_eq!(kept, 3, "every solve survived the migration");
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM rounds", [], |r| r.get::<_, i64>(0)).unwrap(), 3);
        let day = day_solves(&conn, "2026-09-15");
        // Worked out from the round: full value, a point off the hinted one, and
        // nothing claimed for a solve whose round has gone.
        assert_eq!(day.iter().map(|s| (s.user, s.points, s.worth)).collect::<Vec<_>>(), vec![(5, 2, 2), (6, 0, 1), (7, 2, 0)]);
        // And running it again changes nothing.
        init(&conn).unwrap();
        assert_eq!(day_solves(&conn, "2026-09-15"), day);
    }

    #[test]
    fn meta_remembers_the_card() {
        let conn = memory();
        assert_eq!(meta_get(&conn, "card"), None);
        meta_set(&conn, "card", "5:6").unwrap();
        meta_set(&conn, "card", "7:8").unwrap();
        assert_eq!(meta_get(&conn, "card").as_deref(), Some("7:8"));
        let r = put(&conn, "guitar", 0, 2, 1);
        set_message(&conn, r.id, 99).unwrap();
        assert_eq!(get(&conn, r.id).unwrap().message, Some(99));
    }
}
