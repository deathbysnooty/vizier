//! Everything Geo remembers: the rounds it has posted, who named each place
//! and how near they came, and the matches those rounds are grouped into.
//!
//! One file, `<workspace>/.runtime/geo.db`, opened once at start the way the
//! movie store is. A round that was open when the bot went down is still open
//! when it comes back, so a restart mid-match costs the room nothing.
//!
//! A geo round pays by ACCURACY, not by being first alone — the state, a town
//! nearby, or a bullseye — so unlike Guess the Movie, which counts films, a
//! match here is scored by SUMMING what each round paid. That one difference is
//! why this store keeps `won` on every round.

use std::collections::HashSet;
use std::sync::OnceLock;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

pub const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS rounds (
        id INTEGER PRIMARY KEY AUTOINCREMENT, spot_id TEXT NOT NULL,
        state TEXT NOT NULL, lat REAL NOT NULL, lon REAL NOT NULL,
        posted_ts INTEGER NOT NULL, message_id INTEGER, channel_id INTEGER NOT NULL,
        status TEXT NOT NULL DEFAULT 'open', match_id INTEGER,
        winner INTEGER, winning_guess TEXT, verdict TEXT, km REAL, won INTEGER NOT NULL DEFAULT 0,
        solved_ts INTEGER, seconds INTEGER,
        hint_by INTEGER, hint_ts INTEGER, ended_by INTEGER, ended_ts INTEGER);
    CREATE INDEX IF NOT EXISTS rounds_status ON rounds (status);
    CREATE INDEX IF NOT EXISTS rounds_spot ON rounds (spot_id, posted_ts);
    CREATE INDEX IF NOT EXISTS rounds_match ON rounds (match_id);
    CREATE TABLE IF NOT EXISTS solves (
        day TEXT NOT NULL, user_id INTEGER NOT NULL, round_id INTEGER NOT NULL,
        points INTEGER NOT NULL DEFAULT 0, verdict TEXT NOT NULL DEFAULT '',
        km REAL, guess TEXT NOT NULL DEFAULT '', seconds INTEGER NOT NULL DEFAULT 0,
        ts INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (user_id, round_id));
    CREATE INDEX IF NOT EXISTS solves_day ON solves (day);
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS matches (
        id INTEGER PRIMARY KEY AUTOINCREMENT, status TEXT NOT NULL DEFAULT 'break',
        rounds INTEGER NOT NULL, played INTEGER NOT NULL DEFAULT 0,
        opened_ts INTEGER NOT NULL, ready_from INTEGER NOT NULL,
        started_ts INTEGER, ended_ts INTEGER, channel_id INTEGER NOT NULL);
    CREATE INDEX IF NOT EXISTS matches_status ON matches (status);
    CREATE TABLE IF NOT EXISTS ready (
        match_id INTEGER NOT NULL, user_id INTEGER NOT NULL, ts INTEGER NOT NULL,
        PRIMARY KEY (match_id, user_id));
    CREATE TABLE IF NOT EXISTS prizes (
        match_id INTEGER NOT NULL, user_id INTEGER NOT NULL, place INTEGER NOT NULL,
        score INTEGER NOT NULL, points INTEGER NOT NULL, granted INTEGER NOT NULL, ts INTEGER NOT NULL,
        PRIMARY KEY (match_id, user_id));";

pub fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)
}

pub fn open(workspace: &str) -> anyhow::Result<()> {
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    std::fs::create_dir_all(&dir)?;
    let conn = Connection::open(dir.join("geo.db"))?;
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

// --- rounds ---------------------------------------------------------------------------------

/// How a round ended, or that it hasn't.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Up in the channel, nobody has placed it.
    Open,
    Solved,
    /// A player passed after a hint, or a mod skipped it outright.
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

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: i64,
    /// Which photo, so the same street is not posted twice in a fortnight.
    pub spot_id: String,
    /// The answer, copied out of the bank so an old round can still be read
    /// after the bank is rebuilt.
    pub state: String,
    pub lat: f64,
    pub lon: f64,
    pub posted_ts: i64,
    pub message: Option<u64>,
    pub channel: u64,
    pub status: Status,
    pub match_id: Option<i64>,
    pub winner: Option<u64>,
    pub winning_guess: Option<String>,
    /// How near the winner came: `state`, `near` or `bullseye`.
    pub verdict: Option<String>,
    pub km: Option<f64>,
    /// What the round actually paid, after any hint was taken off it.
    pub won: i64,
    pub solved_ts: Option<i64>,
    pub seconds: Option<i64>,
    pub hint_by: Option<u64>,
    pub hint_ts: Option<i64>,
    pub ended_by: Option<u64>,
    pub ended_ts: Option<i64>,
}

impl Row {
    pub fn hinted(&self) -> bool {
        self.hint_ts.is_some()
    }
}

const COLUMNS: &str = "id, spot_id, state, lat, lon, posted_ts, message_id, channel_id, status, match_id, \
                       winner, winning_guess, verdict, km, won, solved_ts, seconds, hint_by, hint_ts, ended_by, ended_ts";

fn read_row(row: &rusqlite::Row) -> rusqlite::Result<Row> {
    let status: String = row.get(8)?;
    Ok(Row {
        id: row.get(0)?,
        spot_id: row.get(1)?,
        state: row.get(2)?,
        lat: row.get(3)?,
        lon: row.get(4)?,
        posted_ts: row.get(5)?,
        message: row.get::<_, Option<i64>>(6)?.map(|v| v as u64),
        channel: row.get::<_, i64>(7)? as u64,
        status: Status::from_key(&status),
        match_id: row.get(9)?,
        winner: row.get::<_, Option<i64>>(10)?.map(|v| v as u64),
        winning_guess: row.get(11)?,
        verdict: row.get(12)?,
        km: row.get(13)?,
        won: row.get(14)?,
        solved_ts: row.get(15)?,
        seconds: row.get(16)?,
        hint_by: row.get::<_, Option<i64>>(17)?.map(|v| v as u64),
        hint_ts: row.get(18)?,
        ended_by: row.get::<_, Option<i64>>(19)?.map(|v| v as u64),
        ended_ts: row.get(20)?,
    })
}

pub fn add_round(conn: &Connection, spot_id: &str, state: &str, lat: f64, lon: f64, channel: u64, now: i64) -> rusqlite::Result<Row> {
    conn.execute(
        "INSERT INTO rounds (spot_id, state, lat, lon, posted_ts, channel_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![spot_id, state, lat, lon, now, channel as i64],
    )?;
    get(conn, conn.last_insert_rowid()).ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn get(conn: &Connection, id: i64) -> Option<Row> {
    conn.query_row(&format!("SELECT {} FROM rounds WHERE id = ?1", COLUMNS), params![id], read_row).optional().ok().flatten()
}

/// The round that is up, if one is.
pub fn live(conn: &Connection) -> Option<Row> {
    conn.query_row(&format!("SELECT {} FROM rounds WHERE status = 'open' ORDER BY id DESC LIMIT 1", COLUMNS), [], read_row)
        .optional()
        .ok()
        .flatten()
}

pub fn recent(conn: &Connection, limit: usize) -> Vec<Row> {
    let sql = format!("SELECT {} FROM rounds ORDER BY id DESC LIMIT ?1", COLUMNS);
    let Ok(mut stmt) = conn.prepare(&sql) else { return Vec::new() };
    let Ok(rows) = stmt.query_map(params![limit as i64], read_row) else { return Vec::new() };
    rows.filter_map(Result::ok).collect()
}

pub fn set_message(conn: &Connection, id: i64, message: u64) -> rusqlite::Result<()> {
    conn.execute("UPDATE rounds SET message_id = ?2 WHERE id = ?1", params![id, message as i64]).map(|_| ())
}

/// The photos already used since a moment, so the same street does not come
/// round again too soon.
pub fn spots_since(conn: &Connection, since: i64) -> HashSet<String> {
    let Ok(mut stmt) = conn.prepare("SELECT DISTINCT spot_id FROM rounds WHERE posted_ts >= ?1") else { return HashSet::new() };
    let Ok(rows) = stmt.query_map(params![since], |r| r.get::<_, String>(0)) else { return HashSet::new() };
    rows.filter_map(Result::ok).collect()
}

/// The states a match has already been set in, so it does not run the same
/// one twice while there are others left.
pub fn states_in_match(conn: &Connection, match_id: i64) -> Vec<String> {
    let Ok(mut stmt) = conn.prepare("SELECT DISTINCT state FROM rounds WHERE match_id = ?1") else { return Vec::new() };
    let Ok(rows) = stmt.query_map(params![match_id], |r| r.get::<_, String>(0)) else { return Vec::new() };
    rows.filter_map(Result::ok).collect()
}

/// Hands the round to the first person who placed it. `false` means somebody
/// else got there first — the `status = 'open'` clause is what makes two
/// guesses landing together safe.
#[allow(clippy::too_many_arguments)]
pub fn claim(conn: &Connection, id: i64, user: u64, guess: &str, verdict: &str, km: Option<f64>, won: i64, now: i64) -> rusqlite::Result<bool> {
    conn.execute(
        "UPDATE rounds SET status = 'solved', winner = ?2, winning_guess = ?3, verdict = ?4, km = ?5, won = ?6, \
         solved_ts = ?7, seconds = ?7 - posted_ts WHERE id = ?1 AND status = 'open'",
        params![id, user as i64, guess, verdict, km, won, now],
    )
    .map(|n| n > 0)
}

/// Marks the hint as taken. `false` means it already was.
pub fn take_hint(conn: &Connection, id: i64, user: u64, now: i64) -> rusqlite::Result<bool> {
    conn.execute(
        "UPDATE rounds SET hint_by = ?2, hint_ts = ?3 WHERE id = ?1 AND status = 'open' AND hint_ts IS NULL",
        params![id, user as i64, now],
    )
    .map(|n| n > 0)
}

pub fn skip(conn: &Connection, id: i64, user: u64, now: i64) -> rusqlite::Result<bool> {
    conn.execute(
        "UPDATE rounds SET status = 'skipped', ended_by = ?2, ended_ts = ?3 WHERE id = ?1 AND status = 'open'",
        params![id, user as i64, now],
    )
    .map(|n| n > 0)
}

pub fn expire(conn: &Connection, id: i64, now: i64) -> rusqlite::Result<bool> {
    conn.execute("UPDATE rounds SET status = 'expired', ended_ts = ?2 WHERE id = ?1 AND status = 'open'", params![id, now]).map(|n| n > 0)
}

// --- what people scored ---------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Solve {
    pub user: u64,
    pub round: i64,
    pub points: i64,
    pub verdict: String,
    pub km: Option<f64>,
    pub guess: String,
    pub seconds: i64,
    pub ts: i64,
}

#[allow(clippy::too_many_arguments)]
pub fn add_solve(
    conn: &Connection,
    day: &str,
    user: u64,
    round: i64,
    points: i64,
    verdict: &str,
    km: Option<f64>,
    guess: &str,
    seconds: i64,
    now: i64,
) -> rusqlite::Result<bool> {
    conn.execute(
        "INSERT OR IGNORE INTO solves (day, user_id, round_id, points, verdict, km, guess, seconds, ts) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![day, user as i64, round, points, verdict, km, guess, seconds, now],
    )
    .map(|n| n > 0)
}

pub fn day_solves(conn: &Connection, day: &str) -> Vec<Solve> {
    let sql = "SELECT user_id, round_id, points, verdict, km, guess, seconds, ts FROM solves WHERE day = ?1 ORDER BY ts";
    let Ok(mut stmt) = conn.prepare(sql) else { return Vec::new() };
    let Ok(rows) = stmt.query_map(params![day], |r| {
        Ok(Solve {
            user: r.get::<_, i64>(0)? as u64,
            round: r.get(1)?,
            points: r.get(2)?,
            verdict: r.get(3)?,
            km: r.get(4)?,
            guess: r.get(5)?,
            seconds: r.get(6)?,
            ts: r.get(7)?,
        })
    }) else {
        return Vec::new();
    };
    rows.filter_map(Result::ok).collect()
}

/// One person's standing in the game's own score, which is uncapped and has
/// nothing to do with the House Cup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tally {
    pub user: u64,
    pub points: i64,
    pub solves: i64,
    pub bullseyes: i64,
}

pub fn tally_between(conn: &Connection, from_day: &str, to_day: &str) -> Vec<Tally> {
    let sql = "SELECT user_id, SUM(points), COUNT(*), SUM(verdict = 'bullseye') FROM solves \
               WHERE day BETWEEN ?1 AND ?2 GROUP BY user_id ORDER BY SUM(points) DESC, COUNT(*) ASC";
    let Ok(mut stmt) = conn.prepare(sql) else { return Vec::new() };
    let Ok(rows) = stmt.query_map(params![from_day, to_day], |r| {
        Ok(Tally { user: r.get::<_, i64>(0)? as u64, points: r.get(1)?, solves: r.get(2)?, bullseyes: r.get(3)? })
    }) else {
        return Vec::new();
    };
    rows.filter_map(Result::ok).collect()
}

// --- matches --------------------------------------------------------------------------------

/// Where a match has got to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchStatus {
    /// Between matches: the ready list fills and the break runs down.
    Break,
    Playing,
    Done,
}

impl MatchStatus {
    pub fn key(self) -> &'static str {
        match self {
            MatchStatus::Break => "break",
            MatchStatus::Playing => "playing",
            MatchStatus::Done => "done",
        }
    }

    pub fn from_key(key: &str) -> MatchStatus {
        match key {
            "break" => MatchStatus::Break,
            "playing" => MatchStatus::Playing,
            _ => MatchStatus::Done,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub id: i64,
    pub status: MatchStatus,
    /// How many places this match runs before it is scored.
    pub rounds: i64,
    pub played: i64,
    pub opened_ts: i64,
    /// The earliest it may start, however many people are ready.
    pub ready_from: i64,
    pub started_ts: Option<i64>,
    pub ended_ts: Option<i64>,
    pub channel: u64,
}

const MATCH_COLUMNS: &str = "id, status, rounds, played, opened_ts, ready_from, started_ts, ended_ts, channel_id";

fn read_match(row: &rusqlite::Row) -> rusqlite::Result<Match> {
    let status: String = row.get(1)?;
    Ok(Match {
        id: row.get(0)?,
        status: MatchStatus::from_key(&status),
        rounds: row.get(2)?,
        played: row.get(3)?,
        opened_ts: row.get(4)?,
        ready_from: row.get(5)?,
        started_ts: row.get(6)?,
        ended_ts: row.get(7)?,
        channel: row.get::<_, i64>(8)? as u64,
    })
}

/// Opens the break before a match: nobody ready, nothing starting before
/// `ready_from`.
pub fn open_match(conn: &Connection, rounds: i64, channel: u64, now: i64, ready_from: i64) -> rusqlite::Result<Match> {
    conn.execute(
        "INSERT INTO matches (status, rounds, opened_ts, ready_from, channel_id) VALUES ('break', ?1, ?2, ?3, ?4)",
        params![rounds, now, ready_from, channel as i64],
    )?;
    get_match(conn, conn.last_insert_rowid()).ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn get_match(conn: &Connection, id: i64) -> Option<Match> {
    conn.query_row(&format!("SELECT {} FROM matches WHERE id = ?1", MATCH_COLUMNS), params![id], read_match).optional().ok().flatten()
}

/// The match the channel is on, breaking or playing.
pub fn live_match(conn: &Connection) -> Option<Match> {
    conn.query_row(
        &format!("SELECT {} FROM matches WHERE status IN ('break', 'playing') ORDER BY id DESC LIMIT 1", MATCH_COLUMNS),
        [],
        read_match,
    )
    .optional()
    .ok()
    .flatten()
}

/// Says somebody is ready. `false` means they already were.
pub fn mark_ready(conn: &Connection, match_id: i64, user: u64, now: i64) -> rusqlite::Result<bool> {
    conn.execute("INSERT OR IGNORE INTO ready (match_id, user_id, ts) VALUES (?1, ?2, ?3)", params![match_id, user as i64, now])
        .map(|n| n > 0)
}

pub fn unready(conn: &Connection, match_id: i64, user: u64) -> rusqlite::Result<bool> {
    conn.execute("DELETE FROM ready WHERE match_id = ?1 AND user_id = ?2", params![match_id, user as i64]).map(|n| n > 0)
}

pub fn is_ready(conn: &Connection, match_id: i64, user: u64) -> bool {
    conn.query_row("SELECT 1 FROM ready WHERE match_id = ?1 AND user_id = ?2", params![match_id, user as i64], |_| Ok(()))
        .optional()
        .ok()
        .flatten()
        .is_some()
}

pub fn ready_list(conn: &Connection, match_id: i64) -> Vec<u64> {
    let Ok(mut stmt) = conn.prepare("SELECT user_id FROM ready WHERE match_id = ?1 ORDER BY ts") else { return Vec::new() };
    let Ok(rows) = stmt.query_map(params![match_id], |r| r.get::<_, i64>(0)) else { return Vec::new() };
    rows.filter_map(Result::ok).map(|v| v as u64).collect()
}

/// Break to playing. Only a break can start, so two ticks cannot start it twice.
pub fn start_match(conn: &Connection, id: i64, now: i64) -> rusqlite::Result<bool> {
    conn.execute("UPDATE matches SET status = 'playing', started_ts = ?2 WHERE id = ?1 AND status = 'break'", params![id, now])
        .map(|n| n > 0)
}

/// Ties a round to its match and counts it against the total.
pub fn claim_round(conn: &Connection, match_id: i64, round: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE rounds SET match_id = ?2 WHERE id = ?1", params![round, match_id])?;
    conn.execute("UPDATE matches SET played = played + 1 WHERE id = ?1", params![match_id]).map(|_| ())
}

pub fn end_match(conn: &Connection, id: i64, now: i64) -> rusqlite::Result<bool> {
    conn.execute("UPDATE matches SET status = 'done', ended_ts = ?2 WHERE id = ?1 AND status = 'playing'", params![id, now])
        .map(|n| n > 0)
}

/// What each person scored in a match, best first.
///
/// Geo rounds pay differently from each other — two for the state, five for a
/// bullseye — so a match is won on POINTS, not on rounds taken. Ties break on
/// who got there first, so the order is the one the card showed all along.
pub fn match_scores(conn: &Connection, match_id: i64) -> Vec<(u64, i64)> {
    let sql = "SELECT winner, SUM(won) AS score FROM rounds \
               WHERE match_id = ?1 AND status = 'solved' AND winner IS NOT NULL \
               GROUP BY winner ORDER BY score DESC, MIN(solved_ts) ASC";
    let Ok(mut stmt) = conn.prepare(sql) else { return Vec::new() };
    let Ok(rows) = stmt.query_map(params![match_id], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)?))) else {
        return Vec::new();
    };
    rows.filter_map(Result::ok).collect()
}

/// Writes down what a match paid. The primary key IS the dedupe: a match can
/// only ever pay one person once, however often the scoring is retried.
#[allow(clippy::too_many_arguments)]
pub fn add_prize(conn: &Connection, match_id: i64, user: u64, place: i64, score: i64, points: i64, granted: i64, now: i64) -> rusqlite::Result<bool> {
    conn.execute(
        "INSERT OR IGNORE INTO prizes (match_id, user_id, place, score, points, granted, ts) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![match_id, user as i64, place, score, points, granted, now],
    )
    .map(|n| n > 0)
}

pub fn prizes_paid(conn: &Connection, match_id: i64) -> bool {
    conn.query_row("SELECT 1 FROM prizes WHERE match_id = ?1", params![match_id], |_| Ok(())).optional().ok().flatten().is_some()
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn memory() -> Connection {
        let conn = Connection::open_in_memory().expect("a database");
        init(&conn).expect("the schema");
        conn
    }

    fn put(conn: &Connection, spot: &str, state: &str, now: i64) -> Row {
        add_round(conn, spot, state, 17.4, 78.5, 1, now).expect("a round")
    }

    #[test]
    fn a_round_survives_being_written_and_read() {
        let conn = memory();
        let row = put(&conn, "kv1", "Telangana", 100);
        let back = get(&conn, row.id).expect("the round");
        assert_eq!(back.spot_id, "kv1");
        assert_eq!(back.state, "Telangana");
        assert_eq!(back.status, Status::Open);
        assert_eq!(back.won, 0);
    }

    /// Two guesses landing in the same instant must not both win.
    #[test]
    fn only_the_first_guess_takes_a_round() {
        let conn = memory();
        let row = put(&conn, "kv1", "Telangana", 100);
        assert!(claim(&conn, row.id, 7, "hyderabad", "bullseye", Some(0.6), 5, 110).expect("first"));
        assert!(!claim(&conn, row.id, 8, "telangana", "state", None, 2, 110).expect("second"));
        let back = get(&conn, row.id).expect("the round");
        assert_eq!(back.winner, Some(7));
        assert_eq!(back.won, 5);
        assert_eq!(back.seconds, Some(10));
    }

    #[test]
    fn the_hint_is_taken_once() {
        let conn = memory();
        let row = put(&conn, "kv1", "Telangana", 100);
        assert!(take_hint(&conn, row.id, 7, 105).expect("first"));
        assert!(!take_hint(&conn, row.id, 8, 106).expect("second"));
        assert!(get(&conn, row.id).expect("the round").hinted());
    }

    /// A match is won on POINTS, not on rounds taken: three states beat one
    /// bullseye, but one bullseye beats two states.
    #[test]
    fn a_match_is_scored_on_points_not_rounds() {
        let conn = memory();
        let m = open_match(&conn, 3, 1, 100, 100).expect("a match");
        for (i, (user, verdict, won)) in [(7u64, "state", 2i64), (7, "state", 2), (8, "bullseye", 5)].into_iter().enumerate() {
            let row = put(&conn, &format!("kv{i}"), "Telangana", 100 + i as i64);
            claim(&conn, row.id, user, "x", verdict, None, won, 110).expect("a claim");
            claim_round(&conn, m.id, row.id).expect("tied to the match");
        }
        assert_eq!(match_scores(&conn, m.id), vec![(8, 5), (7, 4)]);
        assert_eq!(get_match(&conn, m.id).expect("the match").played, 3);
    }

    /// Paying twice for the same match is the mistake the ledger must never
    /// make, however often scoring is retried.
    #[test]
    fn a_match_pays_each_person_once() {
        let conn = memory();
        let m = open_match(&conn, 5, 1, 100, 100).expect("a match");
        assert!(add_prize(&conn, m.id, 7, 1, 12, 5, 5, 200).expect("first"));
        assert!(!add_prize(&conn, m.id, 7, 1, 12, 5, 5, 200).expect("again"));
        assert!(prizes_paid(&conn, m.id));
    }

    #[test]
    fn a_match_starts_once() {
        let conn = memory();
        let m = open_match(&conn, 5, 1, 100, 120).expect("a match");
        assert!(start_match(&conn, m.id, 120).expect("first"));
        assert!(!start_match(&conn, m.id, 121).expect("second"));
        assert_eq!(live_match(&conn).expect("live").status, MatchStatus::Playing);
    }

    #[test]
    fn the_day_tally_adds_up_what_each_person_scored() {
        let conn = memory();
        add_solve(&conn, "2026-09-21", 7, 1, 5, "bullseye", Some(2.0), "hyderabad", 9, 100).expect("one");
        add_solve(&conn, "2026-09-21", 7, 2, 2, "state", None, "telangana", 30, 200).expect("two");
        add_solve(&conn, "2026-09-21", 8, 3, 4, "near", Some(40.0), "warangal", 12, 300).expect("three");
        let tally = tally_between(&conn, "2026-09-21", "2026-09-21");
        assert_eq!(tally[0], Tally { user: 7, points: 7, solves: 2, bullseyes: 1 });
        assert_eq!(tally[1], Tally { user: 8, points: 4, solves: 1, bullseyes: 0 });
    }

    #[test]
    fn a_photo_is_remembered_so_it_does_not_come_round_again() {
        let conn = memory();
        put(&conn, "kv1", "Telangana", 500);
        put(&conn, "kv2", "Delhi", 600);
        let used = spots_since(&conn, 550);
        assert!(used.contains("kv2"));
        assert!(!used.contains("kv1"));
    }
}
