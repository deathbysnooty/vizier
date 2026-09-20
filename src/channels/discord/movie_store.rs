//! What Guess the Movie keeps in `.runtime/movie.db`: every film the bot has
//! asked about, which clue it asked with, and every guess that named one.
//!
//! A round moves `open` → `solved` when somebody types the title, or
//! `open` → `skipped` when a player skips it after a hint (or a mod skips it
//! outright), or `open` → `expired` when nobody got it all evening. Only the
//! FIRST correct guess wins: the move to `solved` is one
//! `UPDATE … WHERE status = 'open'`, so two guesses landing in the same second
//! can only make one winner.
//!
//! Every round keeps the film's key AND the clue it was asked with, which is
//! what the no-repeat window is made of: a film is held back for a while, and
//! inside that each particular still and each particular line is held back
//! separately, so nobody is shown the same picture twice. A hint writes down
//! who asked and which tag it gave away — a hint costs the round a point, so it
//! has to be attributable.

use std::collections::HashSet;
use std::sync::OnceLock;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

use super::movie_bank::{Clue, clue_key};

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

pub const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS rounds (
        id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT NOT NULL, movie_key TEXT NOT NULL,
        clue TEXT NOT NULL, clue_index INTEGER NOT NULL DEFAULT 0, tags_shown INTEGER NOT NULL DEFAULT 5,
        points INTEGER NOT NULL, posted_ts INTEGER NOT NULL,
        message_id INTEGER, channel_id INTEGER NOT NULL, status TEXT NOT NULL DEFAULT 'open',
        winner INTEGER, winning_guess TEXT, solved_ts INTEGER, seconds INTEGER,
        hint_by INTEGER, hint_ts INTEGER, hint_clue TEXT, hint_shot INTEGER, ended_by INTEGER, ended_ts INTEGER);
    CREATE INDEX IF NOT EXISTS rounds_status ON rounds (status);
    CREATE INDEX IF NOT EXISTS rounds_movie ON rounds (movie_key, posted_ts);
    CREATE TABLE IF NOT EXISTS solves (
        day TEXT NOT NULL, user_id INTEGER NOT NULL, round_id INTEGER NOT NULL,
        points INTEGER NOT NULL DEFAULT 0, worth INTEGER NOT NULL DEFAULT 0, guess TEXT NOT NULL DEFAULT '',
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
    let conn = Connection::open(dir.join("movie.db"))?;
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
    /// The film as it is announced: "Gangs of Wasseypur".
    pub title: String,
    /// Its key, which is what the bank is looked up by and what the no-repeat
    /// window is made of.
    pub movie_key: String,
    /// Which kind of clue is on the card, and which one of that kind.
    pub clue: Clue,
    pub clue_index: i64,
    /// How many tags the card showed. A hint reveals the next one after these,
    /// so this is what decides which.
    pub tags_shown: i64,
    /// What the round was worth when it went up, before any hint.
    pub points: i64,
    pub posted_ts: i64,
    pub message: Option<u64>,
    pub channel: u64,
    pub status: Status,
    pub winner: Option<u64>,
    /// What the winner typed, which need not be the title the bot announces.
    pub winning_guess: Option<String>,
    pub solved_ts: Option<i64>,
    pub seconds: Option<i64>,
    /// Who asked for the hint, when, and what it gave away.
    ///
    /// `hint_clue` is whatever words came with it - a sharper tag on most
    /// rounds, the line written for the second still on a stills round - and is
    /// `None` when the film had nothing left to reveal. The hint still gave the
    /// letter and still cost the round either way. `hint_shot` is the second
    /// still it put up, when it put one up at all.
    pub hint_by: Option<u64>,
    pub hint_ts: Option<i64>,
    pub hint_clue: Option<String>,
    pub hint_shot: Option<i64>,
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
        self.title.chars().next().unwrap_or('?').to_ascii_uppercase()
    }

    /// How the used-clues window names the clue this round used.
    pub fn clue_key(&self) -> String {
        clue_key(self.clue, self.clue_index as usize)
    }
}

const COLS: &str = "id, title, movie_key, clue, clue_index, tags_shown, points, posted_ts, message_id, channel_id, status, winner, \
                    winning_guess, solved_ts, seconds, hint_by, hint_ts, hint_clue, hint_shot, ended_by, ended_ts";

fn row(r: &rusqlite::Row) -> rusqlite::Result<Row> {
    let user = |i: usize| -> rusqlite::Result<Option<u64>> { Ok(r.get::<_, Option<i64>>(i)?.map(|v| v as u64)) };
    Ok(Row {
        id: r.get(0)?,
        title: r.get(1)?,
        movie_key: r.get(2)?,
        clue: Clue::from_key(&r.get::<_, String>(3)?),
        clue_index: r.get(4)?,
        tags_shown: r.get(5)?,
        points: r.get(6)?,
        posted_ts: r.get(7)?,
        message: user(8)?,
        channel: r.get::<_, i64>(9)? as u64,
        status: Status::from_key(&r.get::<_, String>(10)?),
        winner: user(11)?,
        winning_guess: r.get(12)?,
        solved_ts: r.get(13)?,
        seconds: r.get(14)?,
        hint_by: user(15)?,
        hint_ts: r.get(16)?,
        hint_clue: r.get(17)?,
        hint_shot: r.get(18)?,
        ended_by: user(19)?,
        ended_ts: r.get(20)?,
    })
}

/// Writes a fresh round and hands it back with its number.
#[allow(clippy::too_many_arguments)]
pub fn add_round(
    conn: &Connection,
    title: &str,
    movie_key: &str,
    clue: Clue,
    clue_index: i64,
    tags_shown: i64,
    points: i64,
    channel: u64,
    now: i64,
) -> rusqlite::Result<Row> {
    conn.execute(
        "INSERT INTO rounds (title, movie_key, clue, clue_index, tags_shown, points, posted_ts, channel_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![title, movie_key, clue.key(), clue_index, tags_shown, points, now, channel as i64],
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

/// Every film asked about since `since`: what the no-repeat window is made of.
pub fn movies_since(conn: &Connection, since: i64) -> HashSet<String> {
    let Ok(mut stmt) = conn.prepare("SELECT DISTINCT movie_key FROM rounds WHERE posted_ts >= ?1") else {
        return HashSet::new();
    };
    stmt.query_map(params![since], |r| r.get::<_, String>(0)).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

/// Every clue of one film that has been up since `since` - "shot:1", "tags:0" -
/// so the same still is never shown twice inside the window.
pub fn clues_since(conn: &Connection, movie_key: &str, since: i64) -> HashSet<String> {
    let sql = "SELECT clue, clue_index FROM rounds WHERE movie_key = ?1 AND posted_ts >= ?2";
    let Ok(mut stmt) = conn.prepare(sql) else { return HashSet::new() };
    let rows = stmt.query_map(params![movie_key, since], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)));
    let Ok(rows) = rows else { return HashSet::new() };
    rows.flatten().map(|(clue, which)| clue_key(Clue::from_key(&clue), which as usize)).collect()
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

/// The hint, which one round has exactly one of: some sharper words, sometimes
/// a second still, and the title's first letter. True for whoever asked first.
pub fn take_hint(conn: &Connection, id: i64, user: u64, clue: Option<&str>, shot: Option<i64>, now: i64) -> rusqlite::Result<bool> {
    conn.execute(
        "UPDATE rounds SET hint_by = ?2, hint_ts = ?3, hint_clue = ?4, hint_shot = ?5
         WHERE id = ?1 AND status = 'open' AND hint_by IS NULL",
        params![id, user as i64, now, clue, shot],
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
    /// The MOVIE points the solve was worth: the round's value with the hint
    /// already taken off, before the cap touched it. Every solver gets these,
    /// mods and the unsorted included.
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

/// A day's wins, first one first.
pub fn day_solves(conn: &Connection, day: &str) -> Vec<Solve> {
    let Ok(mut stmt) =
        conn.prepare("SELECT user_id, round_id, points, worth, guess, seconds, ts FROM solves WHERE day = ?1 ORDER BY ts, round_id")
    else {
        return Vec::new();
    };
    stmt.query_map(params![day], solve_row).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

// --- movie points ----------------------------------------------------------------------

/// One player's movie points over a stretch of days.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tally {
    pub user: u64,
    /// Movie points: every solve at its full value, so the house-points cap
    /// never hides one.
    pub points: i64,
    /// How many rounds they won.
    pub solves: i64,
    /// When they got to that total — their last solve of the stretch.
    pub reached: i64,
}

/// The order a board reads in: most movie points, then most rounds won, then
/// whoever got there first.
pub fn rank(rows: &mut [Tally]) {
    rows.sort_by(|a, b| b.points.cmp(&a.points).then(b.solves.cmp(&a.solves)).then(a.reached.cmp(&b.reached)).then(a.user.cmp(&b.user)));
}

/// Everyone's movie points between two days, both ends included (YYYY-MM-DD,
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

/// One day's movie points, best first.
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
    use crate::channels::discord::movie_bank::key;

    pub fn memory() -> Connection {
        let conn = Connection::open_in_memory().expect("memory db");
        init(&conn).expect("schema");
        conn
    }

    pub fn put(conn: &Connection, title: &str, clue: Clue, which: i64, points: i64, now: i64) -> Row {
        add_round(conn, title, &key(title), clue, which, 5, points, 77, now).expect("round")
    }

    #[test]
    fn only_the_first_correct_guess_claims_a_round() {
        let conn = memory();
        let r = put(&conn, "Gangs of Wasseypur", Clue::Tags, 0, 3, 1_000);
        assert_eq!(live(&conn).map(|l| l.id), Some(r.id));
        assert_eq!((r.movie_key.as_str(), r.clue, r.clue_index), ("gangsofvaseipur", Clue::Tags, 0));
        assert!(claim(&conn, r.id, 11, "gangs of wasypur", 1_040).unwrap(), "the first gets it");
        assert!(!claim(&conn, r.id, 22, "gangs of wasseypur", 1_041).unwrap(), "a second later is too late");
        let after = get(&conn, r.id).unwrap();
        assert_eq!(after.status, Status::Solved);
        assert_eq!((after.winner, after.winning_guess.as_deref(), after.seconds), (Some(11), Some("gangs of wasypur"), Some(40)));
        assert!(live(&conn).is_none(), "a solved round is no longer the live one");
        // And nothing can move it afterwards.
        assert!(!skip(&conn, r.id, 5, 1_100).unwrap());
        assert!(!expire(&conn, r.id, 9_000).unwrap());
        assert!(!take_hint(&conn, r.id, 5, Some("a tag"), None, 1_100).unwrap());
    }

    #[test]
    fn a_round_has_one_hint_and_it_says_who_asked_and_what_it_revealed() {
        let conn = memory();
        let r = put(&conn, "Andhadhun", Clue::Shot, 2, 3, 500);
        assert!(!get(&conn, r.id).unwrap().hinted());
        assert_eq!(r.clue_key(), "shot:2");
        assert!(take_hint(&conn, r.id, 42, Some("a rabbit"), Some(1), 540).unwrap(), "the first ask gets it");
        assert!(!take_hint(&conn, r.id, 43, Some("a liver"), None, 560).unwrap(), "and it is the only one");
        let after = get(&conn, r.id).unwrap();
        assert_eq!((after.hint_by, after.hint_ts, after.hint_clue.as_deref()), (Some(42), Some(540), Some("a rabbit")));
        assert_eq!(after.hint_shot, Some(1), "and the second still it put up");
        assert_eq!(after.first_letter(), 'A');
        assert!(after.hinted());
        // A film with no tag left to reveal still gets its hint, and it is
        // written down as one.
        let two = put(&conn, "Dangal", Clue::Dialogue, 0, 3, 600);
        assert!(take_hint(&conn, two.id, 7, None, None, 640).unwrap());
        let after = get(&conn, two.id).unwrap();
        assert!(after.hinted() && after.hint_clue.is_none() && after.hint_shot.is_none());
    }

    #[test]
    fn a_skipped_round_and_a_stale_one_are_told_apart_and_both_name_the_time() {
        let conn = memory();
        let one = put(&conn, "Sholay", Clue::Tags, 0, 3, 100);
        assert!(skip(&conn, one.id, 7, 200).unwrap());
        let after = get(&conn, one.id).unwrap();
        assert_eq!((after.status, after.ended_by, after.ended_ts), (Status::Skipped, Some(7), Some(200)));
        assert!(!claim(&conn, one.id, 9, "sholay", 300).unwrap(), "a skipped round can't be won");
        let two = put(&conn, "Titanic", Clue::Shot, 1, 3, 400);
        assert!(expire(&conn, two.id, 3_000).unwrap());
        let after = get(&conn, two.id).unwrap();
        assert_eq!((after.status, after.ended_by, after.ended_ts), (Status::Expired, None, Some(3_000)));
        assert!(live(&conn).is_none());
        assert_eq!(recent(&conn, 5).len(), 2);
    }

    #[test]
    fn neither_a_film_nor_a_single_still_comes_round_again_inside_the_window() {
        let conn = memory();
        let day = 86_400;
        put(&conn, "Sholay", Clue::Shot, 0, 3, 1_000);
        put(&conn, "Titanic", Clue::Tags, 0, 3, 1_000 + 10 * day);
        put(&conn, "Sholay", Clue::Shot, 2, 3, 1_000 + 12 * day);
        put(&conn, "Sholay", Clue::Dialogue, 1, 3, 1_000 + 13 * day);
        let now = 1_000 + 20 * day;
        // A fortnight's films, and a longer window that reaches the older round.
        assert_eq!(movies_since(&conn, now - 14 * day), HashSet::from(["titanic".to_string(), "sholai".to_string()]));
        assert!(movies_since(&conn, now - 1).is_empty());
        // The clues of one film, each held back on its own.
        assert_eq!(clues_since(&conn, "sholai", now - 14 * day), HashSet::from(["shot:2".to_string(), "dialogue:1".to_string()]));
        assert!(clues_since(&conn, "sholai", 0).contains("shot:0"));
        assert!(clues_since(&conn, "inception", 0).is_empty(), "a film never asked about has nothing held back");
    }

    #[test]
    fn a_day_of_wins_is_kept_and_what_it_was_worth_survives_the_cap() {
        let conn = memory();
        let one = put(&conn, "Inception", Clue::Tags, 0, 3, 10);
        let two = put(&conn, "Interstellar", Clue::Shot, 0, 3, 20);
        // A capped day: the ledger pays nothing, the game still scores it.
        add_solve(&conn, "2026-09-16", 1, one.id, 0, 3, "inception", 40, 100).unwrap();
        // A mod, who has no house and so no ledger row at all.
        add_solve(&conn, "2026-09-16", 9, two.id, 0, 2, "interstellar", 20, 120).unwrap();
        // The same person on the same round doesn't count twice.
        add_solve(&conn, "2026-09-16", 1, one.id, 0, 3, "inception", 40, 300).unwrap();
        let day = day_solves(&conn, "2026-09-16");
        assert_eq!(day.iter().map(|s| (s.user, s.points, s.worth)).collect::<Vec<_>>(), vec![(1, 0, 3), (9, 0, 2)]);
        assert!(solved_by(&conn, one.id, 1) && !solved_by(&conn, one.id, 2));
        assert!(day_solves(&conn, "2026-09-17").is_empty());
    }

    #[test]
    fn the_board_is_points_then_rounds_then_whoever_got_there_first() {
        let conn = memory();
        let round = |n: i64, points: i64| put(&conn, "Sholay", Clue::Tags, n, points, n).id;
        let (a, b, c, d, e) = (round(1, 1), round(2, 1), round(3, 3), round(4, 1), round(5, 1));
        // 1: two cheap rounds. 2: one dear one — fewer rounds, more points.
        add_solve(&conn, "2026-09-16", 1, a, 1, 1, "sholay", 5, 100).unwrap();
        add_solve(&conn, "2026-09-16", 1, b, 1, 1, "sholay", 5, 110).unwrap();
        add_solve(&conn, "2026-09-16", 2, c, 3, 3, "sholay", 5, 120).unwrap();
        // 3 ties 1 on points with fewer rounds; 4 ties both and finished later.
        add_solve(&conn, "2026-09-16", 3, d, 2, 2, "sholay", 5, 130).unwrap();
        add_solve(&conn, "2026-09-16", 4, e, 2, 2, "sholay", 5, 140).unwrap();
        let tally = day_tally(&conn, "2026-09-16");
        assert_eq!(tally.iter().map(|t| (t.user, t.points, t.solves)).collect::<Vec<_>>(), vec![(2, 3, 1), (1, 2, 2), (3, 2, 1), (4, 2, 1)]);
        // A month is the same reading over a stretch of days, and a stretch with
        // nothing in it is empty rather than wrong.
        add_solve(&conn, "2026-09-02", 4, round(6, 3), 3, 3, "sholay", 5, 90).unwrap();
        let month = tally_between(&conn, "2026-09-01", "2026-09-31");
        assert_eq!(month.first().map(|t| (t.user, t.points, t.solves)), Some((4, 5, 2)));
        assert!(tally_between(&conn, "2026-08-01", "2026-08-31").is_empty());
    }

    #[test]
    fn meta_remembers_the_card() {
        let conn = memory();
        assert_eq!(meta_get(&conn, "card"), None);
        meta_set(&conn, "card", "5:6").unwrap();
        meta_set(&conn, "card", "7:8").unwrap();
        assert_eq!(meta_get(&conn, "card").as_deref(), Some("7:8"));
        let r = put(&conn, "Sholay", Clue::Tags, 0, 3, 1);
        set_message(&conn, r.id, 99).unwrap();
        assert_eq!(get(&conn, r.id).unwrap().message, Some(99));
    }
}
