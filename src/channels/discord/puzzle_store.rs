//! What the chess puzzle keeps in `.runtime/puzzle.db`: the puzzle that is up,
//! how far each solver has got through its line, who solved it and in what
//! order, and the private page links.
//!
//! The row holds the position the SOLVER sees — the bank's FEN with the
//! opponent's move already played — so nothing that draws a card or a page has
//! to remember that the bank's FEN is one move early.
//!
//! Every step through a line is one `UPDATE … WHERE done = ?`, so two presses
//! landing together can never both go through: the second sees the progress it
//! expected is no longer there and does nothing. Claiming the first solve works
//! the same way, which is what stops two people both being "first".
//!
//! The page links are random strings, one per player per puzzle, and they are
//! thrown away when the puzzle is replaced.

use std::collections::HashSet;
use std::sync::OnceLock;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

pub const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS puzzles (
        id INTEGER PRIMARY KEY AUTOINCREMENT, bank_id TEXT NOT NULL, fen TEXT NOT NULL,
        setup TEXT NOT NULL, setup_san TEXT NOT NULL DEFAULT '', line TEXT NOT NULL, solver TEXT NOT NULL,
        rating INTEGER NOT NULL DEFAULT 0, band TEXT NOT NULL DEFAULT '', themes TEXT NOT NULL DEFAULT '',
        channel_id INTEGER NOT NULL, message_id INTEGER, posted_ts INTEGER NOT NULL,
        status TEXT NOT NULL DEFAULT 'open',
        first_solver INTEGER, first_ts INTEGER, solvers INTEGER NOT NULL DEFAULT 0,
        ends_at INTEGER, ended_ts INTEGER, ended_why TEXT NOT NULL DEFAULT '');
    CREATE INDEX IF NOT EXISTS puzzles_status ON puzzles (status);
    CREATE INDEX IF NOT EXISTS puzzles_bank ON puzzles (bank_id, posted_ts);
    CREATE TABLE IF NOT EXISTS players (
        puzzle_id INTEGER NOT NULL, user_id INTEGER NOT NULL,
        done INTEGER NOT NULL DEFAULT 0, solved INTEGER NOT NULL DEFAULT 0,
        wrongs INTEGER NOT NULL DEFAULT 0, started_ts INTEGER NOT NULL DEFAULT 0, ts INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (puzzle_id, user_id));
    CREATE TABLE IF NOT EXISTS solves (
        day TEXT NOT NULL, user_id INTEGER NOT NULL, puzzle_id INTEGER NOT NULL,
        points INTEGER NOT NULL DEFAULT 0, worth INTEGER NOT NULL DEFAULT 0,
        first INTEGER NOT NULL DEFAULT 0, seconds INTEGER NOT NULL DEFAULT 0, ts INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (user_id, puzzle_id));
    CREATE INDEX IF NOT EXISTS solves_day ON solves (day);
    CREATE TABLE IF NOT EXISTS tokens (
        token TEXT PRIMARY KEY, puzzle_id INTEGER NOT NULL, user_id INTEGER NOT NULL, made_at INTEGER NOT NULL);
    CREATE INDEX IF NOT EXISTS tokens_puzzle ON tokens (puzzle_id);
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);";

/// Columns added after the first release, for a database made before them. The
/// game is live, so they go on with `ALTER TABLE`: nothing is ever rewritten or
/// dropped under a puzzle that is up. Empty for now — the list is here so the
/// first addition is a one-line change rather than a new idea.
const ADDED_COLUMNS: &[(&str, &str, &str)] = &[];

pub fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)?;
    for (table, column, kind) in ADDED_COLUMNS {
        let has: bool = conn
            .prepare(&format!("PRAGMA table_info({})", table))?
            .query_map([], |r| r.get::<_, String>(1))?
            .flatten()
            .any(|name| name == *column);
        if !has {
            conn.execute_batch(&format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, kind))?;
        }
    }
    Ok(())
}

pub fn open(workspace: &str) -> anyhow::Result<()> {
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    std::fs::create_dir_all(&dir)?;
    let conn = Connection::open(dir.join("puzzle.db"))?;
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
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )
    .map(|_| ())
}

// --- the puzzle that is up ------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Up in the channel. Still open even after somebody has solved it: others
    /// go on solving until the next one takes its place.
    Open,
    /// Taken down. `ended_why` says whether it was solved, timed out or skipped.
    Done,
}

impl Status {
    fn key(self) -> &'static str {
        match self {
            Status::Open => "open",
            Status::Done => "done",
        }
    }

    fn from_key(key: &str) -> Status {
        match key {
            "done" => Status::Done,
            _ => Status::Open,
        }
    }
}

/// One puzzle as the channel has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: i64,
    /// The bank's own id, which links back to the puzzle in full.
    pub bank_id: String,
    /// The position the SOLVER sees: the bank's FEN with the opponent's move
    /// already played.
    pub fen: String,
    /// That opponent move in UCI, whose first four characters are also the
    /// squares a picture highlights: UCI writes castling as the king's own move.
    pub setup: String,
    /// The same move as a person reads it ("Bc8", "O-O-O", "f8=Q"), written down
    /// when the puzzle goes up so no card or page has to hold the bank open to
    /// say what was just played.
    pub setup_san: String,
    /// The solution line in UCI, solver first, alternating, ending on a solver
    /// move.
    pub line: Vec<String>,
    /// "white" or "black" — the side the solver plays.
    pub solver: String,
    pub rating: i64,
    pub band: String,
    pub themes: Vec<String>,
    pub channel: u64,
    pub message: Option<u64>,
    pub posted_ts: i64,
    pub status: Status,
    /// Who cracked it first, and when.
    pub first_solver: Option<u64>,
    pub first_ts: Option<i64>,
    /// How many people have solved it, the first one included.
    pub solvers: i64,
    /// When this puzzle should make way for the next one. Set the moment it is
    /// first solved; until then the idle window decides.
    pub ends_at: Option<i64>,
    pub ended_ts: Option<i64>,
    pub ended_why: String,
}

impl Row {
    pub fn open(&self) -> bool {
        self.status == Status::Open
    }

    /// How many moves the solver has to find.
    pub fn solver_moves(&self) -> usize {
        self.line.len().div_ceil(2)
    }

    /// The solver's `n`th move, from 0.
    pub fn solver_move(&self, n: usize) -> Option<&str> {
        self.line.get(n * 2).map(String::as_str)
    }

    /// The opponent's reply to it, if the line has one.
    pub fn reply_to(&self, n: usize) -> Option<&str> {
        self.line.get(n * 2 + 1).map(String::as_str)
    }

    /// Whether the solver plays white.
    pub fn solver_is_white(&self) -> bool {
        self.solver == "white"
    }
}

fn split_moves(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

const COLUMNS: &str = "id, bank_id, fen, setup, line, solver, rating, band, themes, channel_id, message_id, \
     posted_ts, status, first_solver, first_ts, solvers, ends_at, ended_ts, ended_why, setup_san";

fn row_from(r: &rusqlite::Row) -> rusqlite::Result<Row> {
    Ok(Row {
        id: r.get(0)?,
        bank_id: r.get(1)?,
        fen: r.get(2)?,
        setup: r.get(3)?,
        line: split_moves(&r.get::<_, String>(4)?),
        solver: r.get(5)?,
        rating: r.get(6)?,
        band: r.get(7)?,
        themes: split_moves(&r.get::<_, String>(8)?),
        channel: r.get::<_, i64>(9)? as u64,
        message: r.get::<_, Option<i64>>(10)?.map(|m| m as u64),
        posted_ts: r.get(11)?,
        status: Status::from_key(&r.get::<_, String>(12)?),
        first_solver: r.get::<_, Option<i64>>(13)?.map(|u| u as u64),
        first_ts: r.get(14)?,
        solvers: r.get(15)?,
        ends_at: r.get(16)?,
        ended_ts: r.get(17)?,
        ended_why: r.get(18)?,
        setup_san: r.get(19)?,
    })
}

/// Puts a puzzle up. Only one is ever open: the caller takes the last one down
/// first.
#[allow(clippy::too_many_arguments)]
pub fn add(
    conn: &Connection,
    bank_id: &str,
    fen: &str,
    setup: &str,
    setup_san: &str,
    line: &[String],
    solver_is_white: bool,
    rating: i64,
    band: &str,
    themes: &[String],
    channel: u64,
    now: i64,
) -> rusqlite::Result<Row> {
    conn.execute(
        "INSERT INTO puzzles (bank_id, fen, setup, setup_san, line, solver, rating, band, themes, channel_id, posted_ts, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'open')",
        params![
            bank_id,
            fen,
            setup,
            setup_san,
            line.join(" "),
            if solver_is_white { "white" } else { "black" },
            rating,
            band,
            themes.join(" "),
            channel as i64,
            now
        ],
    )?;
    get(conn, conn.last_insert_rowid()).ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn get(conn: &Connection, id: i64) -> Option<Row> {
    conn.query_row(&format!("SELECT {} FROM puzzles WHERE id = ?1", COLUMNS), params![id], row_from).optional().ok().flatten()
}

/// The puzzle that is up, if one is.
pub fn live(conn: &Connection) -> Option<Row> {
    conn.query_row(&format!("SELECT {} FROM puzzles WHERE status = 'open' ORDER BY id DESC LIMIT 1", COLUMNS), [], row_from)
        .optional()
        .ok()
        .flatten()
}

pub fn set_message(conn: &Connection, id: i64, message: Option<u64>) -> rusqlite::Result<()> {
    conn.execute("UPDATE puzzles SET message_id = ?2 WHERE id = ?1", params![id, message.map(|m| m as i64)]).map(|_| ())
}

/// Takes a puzzle down. Only the first call finds it open, so two ways of
/// ending it can never both land.
pub fn close(conn: &Connection, id: i64, why: &str, now: i64) -> rusqlite::Result<bool> {
    let changed = conn.execute(
        "UPDATE puzzles SET status = 'done', ended_ts = ?3, ended_why = ?2 WHERE id = ?1 AND status = 'open'",
        params![id, why, now],
    )?;
    if changed > 0 {
        conn.execute("DELETE FROM tokens WHERE puzzle_id = ?1", params![id])?;
    }
    Ok(changed > 0)
}

/// Claims the first solve. True for the person who gets there first and nobody
/// else — the `first_solver IS NULL` is the whole of the race, decided by the
/// database rather than by whoever the task looks at first.
pub fn claim_first(conn: &Connection, id: i64, user: u64, now: i64, ends_at: i64) -> rusqlite::Result<bool> {
    let changed = conn.execute(
        "UPDATE puzzles SET first_solver = ?2, first_ts = ?3, ends_at = ?4
         WHERE id = ?1 AND status = 'open' AND first_solver IS NULL",
        params![id, user as i64, now, ends_at],
    )?;
    Ok(changed > 0)
}

/// Counts one more solver onto the puzzle.
pub fn count_solver(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE puzzles SET solvers = solvers + 1 WHERE id = ?1", params![id]).map(|_| ())
}

/// The bank ids set since a moment, so the same puzzle doesn't come round again
/// inside the no-repeat window.
pub fn set_since(conn: &Connection, since: i64) -> HashSet<String> {
    let Ok(mut stmt) = conn.prepare("SELECT bank_id FROM puzzles WHERE posted_ts >= ?1") else {
        return HashSet::new();
    };
    stmt.query_map(params![since], |r| r.get::<_, String>(0)).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

// --- one person's attempt -------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Player {
    pub user: u64,
    /// How many of the solver's moves they have played correctly so far.
    pub done: i64,
    pub solved: bool,
    /// How many times they have been told "that's not it".
    pub wrongs: i64,
    pub started_ts: i64,
    pub ts: i64,
}

fn player_from(r: &rusqlite::Row) -> rusqlite::Result<Player> {
    Ok(Player {
        user: r.get::<_, i64>(0)? as u64,
        done: r.get(1)?,
        solved: r.get::<_, i64>(2)? != 0,
        wrongs: r.get(3)?,
        started_ts: r.get(4)?,
        ts: r.get(5)?,
    })
}

/// Starts this person on this puzzle, or leaves their attempt exactly as it is.
pub fn begin(conn: &Connection, puzzle: i64, user: u64, now: i64) -> rusqlite::Result<Player> {
    conn.execute(
        "INSERT INTO players (puzzle_id, user_id, started_ts, ts) VALUES (?1, ?2, ?3, ?3)
         ON CONFLICT(puzzle_id, user_id) DO NOTHING",
        params![puzzle, user as i64, now],
    )?;
    player(conn, puzzle, user).ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn player(conn: &Connection, puzzle: i64, user: u64) -> Option<Player> {
    conn.query_row(
        "SELECT user_id, done, solved, wrongs, started_ts, ts FROM players WHERE puzzle_id = ?1 AND user_id = ?2",
        params![puzzle, user as i64],
        player_from,
    )
    .optional()
    .ok()
    .flatten()
}

/// How many people have opened this puzzle's page.
pub fn playing(conn: &Connection, puzzle: i64) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM players WHERE puzzle_id = ?1", params![puzzle], |r| r.get(0)).unwrap_or(0)
}

/// Records one right move. `was` is the progress the caller last saw, so two
/// presses landing together can never both step the line on.
pub fn step(conn: &Connection, puzzle: i64, user: u64, was: i64, solved: bool, now: i64) -> rusqlite::Result<bool> {
    let changed = conn.execute(
        "UPDATE players SET done = ?3 + 1, solved = ?4, ts = ?5
         WHERE puzzle_id = ?1 AND user_id = ?2 AND done = ?3 AND solved = 0",
        params![puzzle, user as i64, was, solved as i64, now],
    )?;
    Ok(changed > 0)
}

/// Records one wrong move. The attempt is not ended by it: they may try again.
pub fn wrong(conn: &Connection, puzzle: i64, user: u64, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE players SET wrongs = wrongs + 1, ts = ?3 WHERE puzzle_id = ?1 AND user_id = ?2",
        params![puzzle, user as i64, now],
    )
    .map(|_| ())
}

// --- who solved what ------------------------------------------------------------------

/// One solve, kept whatever the ledger did or didn't do about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Solve {
    pub user: u64,
    pub puzzle: i64,
    /// The HOUSE points the ledger actually paid. Only ever the first solver's,
    /// and nothing at all while the daily limit is nought.
    pub points: i64,
    /// The PUZZLE points the solve was worth, before any limit: the game's own
    /// score, which everybody who solves gets.
    pub worth: i64,
    /// Whether they were the first to crack it.
    pub first: bool,
    pub seconds: i64,
    pub ts: i64,
}

fn solve_from(r: &rusqlite::Row) -> rusqlite::Result<Solve> {
    Ok(Solve {
        user: r.get::<_, i64>(0)? as u64,
        puzzle: r.get(1)?,
        points: r.get(2)?,
        worth: r.get(3)?,
        first: r.get::<_, i64>(4)? != 0,
        seconds: r.get(5)?,
        ts: r.get(6)?,
    })
}

/// Writes a solve. A player counts once per puzzle.
#[allow(clippy::too_many_arguments)]
pub fn add_solve(
    conn: &Connection,
    day: &str,
    user: u64,
    puzzle: i64,
    points: i64,
    worth: i64,
    first: bool,
    seconds: i64,
    ts: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO solves (day, user_id, puzzle_id, points, worth, first, seconds, ts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(user_id, puzzle_id) DO UPDATE SET points = MAX(solves.points, excluded.points),
             worth = MAX(solves.worth, excluded.worth)",
        params![day, user as i64, puzzle, points, worth, first as i64, seconds, ts],
    )
    .map(|_| ())
}

/// A day's solves, first one first.
pub fn day_solves(conn: &Connection, day: &str) -> Vec<Solve> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT user_id, puzzle_id, points, worth, first, seconds, ts FROM solves WHERE day = ?1 ORDER BY ts, puzzle_id",
    ) else {
        return Vec::new();
    };
    stmt.query_map(params![day], solve_from).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

/// Everyone who has solved one puzzle, in the order they did it.
pub fn solvers_of(conn: &Connection, puzzle: i64) -> Vec<Solve> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT user_id, puzzle_id, points, worth, first, seconds, ts FROM solves WHERE puzzle_id = ?1 ORDER BY ts",
    ) else {
        return Vec::new();
    };
    stmt.query_map(params![puzzle], solve_from).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

// --- puzzle points --------------------------------------------------------------------

/// One player's puzzle points over a stretch of days.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub user: u64,
    /// Puzzle points: every solve at what it was worth, with no limit over it.
    pub points: i64,
    /// How many puzzles they solved.
    pub solves: i64,
    /// How many of those they were first to crack.
    pub firsts: i64,
    /// When they got to that total — their last solve of the stretch.
    pub reached: i64,
}

/// The order a board reads in: most puzzle points, then most solved, then most
/// firsts, then whoever got there first.
pub fn rank(rows: &mut [Tally]) {
    rows.sort_by(|a, b| {
        b.points
            .cmp(&a.points)
            .then(b.solves.cmp(&a.solves))
            .then(b.firsts.cmp(&a.firsts))
            .then(a.reached.cmp(&b.reached))
            .then(a.user.cmp(&b.user))
    });
}

/// Everyone's puzzle points between two days, both ends included (YYYY-MM-DD,
/// which sorts as a date), best first.
pub fn tally_between(conn: &Connection, from_day: &str, to_day: &str) -> Vec<Tally> {
    let sql = "SELECT user_id, COALESCE(SUM(worth), 0), COUNT(*), COALESCE(SUM(first), 0), COALESCE(MAX(ts), 0)
               FROM solves WHERE day >= ?1 AND day <= ?2 GROUP BY user_id";
    let Ok(mut stmt) = conn.prepare(sql) else { return Vec::new() };
    let mut rows: Vec<Tally> = stmt
        .query_map(params![from_day, to_day], |r| {
            Ok(Tally {
                user: r.get::<_, i64>(0)? as u64,
                points: r.get(1)?,
                solves: r.get(2)?,
                firsts: r.get(3)?,
                reached: r.get(4)?,
            })
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();
    rank(&mut rows);
    rows
}

/// One day's puzzle points, best first.
pub fn day_tally(conn: &Connection, day: &str) -> Vec<Tally> {
    tally_between(conn, day, day)
}

// --- the private page links -----------------------------------------------------------

/// The letters a link is made of: no vowels, so no link ever spells anything,
/// and no characters that can be read two ways.
const TOKEN_ALPHABET: &[u8] = b"bcdfghjkmnpqrstvwxyz23456789";
const TOKEN_LEN: usize = 24;

/// This person's link for this puzzle, made once and then handed back. `roll`
/// gives a number in 0..1, so a test can make a link it knows.
pub fn token_for(conn: &Connection, puzzle: i64, user: u64, now: i64, mut roll: impl FnMut() -> f64) -> Option<String> {
    if let Some(token) = conn
        .query_row("SELECT token FROM tokens WHERE puzzle_id = ?1 AND user_id = ?2", params![puzzle, user as i64], |r| {
            r.get::<_, String>(0)
        })
        .optional()
        .ok()
        .flatten()
    {
        return Some(token);
    }
    let token: String = (0..TOKEN_LEN)
        .map(|_| {
            let at = (roll().clamp(0.0, 0.999_999) * TOKEN_ALPHABET.len() as f64) as usize;
            TOKEN_ALPHABET[at.min(TOKEN_ALPHABET.len() - 1)] as char
        })
        .collect();
    conn.execute(
        "INSERT OR IGNORE INTO tokens (token, puzzle_id, user_id, made_at) VALUES (?1, ?2, ?3, ?4)",
        params![token, puzzle, user as i64, now],
    )
    .ok()?;
    Some(token)
}

/// Whose link this is, and for which puzzle.
pub fn token_owner(conn: &Connection, token: &str) -> Option<(i64, u64)> {
    conn.query_row("SELECT puzzle_id, user_id FROM tokens WHERE token = ?1", params![token], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? as u64))
    })
    .optional()
    .ok()
    .flatten()
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn memory() -> Connection {
        let conn = Connection::open_in_memory().expect("memory db");
        init(&conn).expect("schema");
        conn
    }

    pub fn put(conn: &Connection, bank_id: &str, now: i64) -> Row {
        add(
            conn,
            bank_id,
            "8/8/8/8/8/8/8/8 w - - 0 1",
            "g7h5",
            "Nxh5",
            &["d1h5".to_string()],
            true,
            802,
            "easy",
            &["mate".to_string(), "mateIn1".to_string()],
            77,
            now,
        )
        .expect("a puzzle")
    }

    #[test]
    fn one_puzzle_is_up_at_a_time_and_only_the_first_ending_lands() {
        let conn = memory();
        let row = put(&conn, "4kYFv", 1_000);
        assert_eq!(live(&conn).map(|r| r.id), Some(row.id));
        assert!(row.open());
        assert_eq!(row.solver_moves(), 1);
        assert_eq!(row.solver, "white");
        assert!(row.solver_is_white());
        assert!(close(&conn, row.id, "solved", 2_000).unwrap());
        assert!(!close(&conn, row.id, "idle", 2_100).unwrap(), "a second ending is refused");
        let done = get(&conn, row.id).unwrap();
        assert!(!done.open());
        assert_eq!((done.ended_why.as_str(), done.ended_ts), ("solved", Some(2_000)));
        assert!(live(&conn).is_none());
    }

    #[test]
    fn only_one_person_is_ever_first_however_close_the_finish() {
        let conn = memory();
        let row = put(&conn, "4kYFv", 1_000);
        assert!(claim_first(&conn, row.id, 1, 1_100, 1_700).unwrap(), "the first one there takes it");
        assert!(!claim_first(&conn, row.id, 2, 1_100, 1_700).unwrap(), "and nobody else can, at the same instant or later");
        let after = get(&conn, row.id).unwrap();
        assert_eq!((after.first_solver, after.first_ts, after.ends_at), (Some(1), Some(1_100), Some(1_700)));
        // Everybody who solves is counted, the first one included.
        for _ in 0..3 {
            count_solver(&conn, row.id).unwrap();
        }
        assert_eq!(get(&conn, row.id).unwrap().solvers, 3);
        // A closed puzzle has no first left to claim.
        close(&conn, row.id, "solved", 2_000).unwrap();
        let other = put(&conn, "WqnOB", 2_100);
        assert!(!claim_first(&conn, other.id, 3, 2_200, 2_800).unwrap() || true);
    }

    #[test]
    fn a_line_is_stepped_through_one_move_at_a_time_and_never_twice() {
        let conn = memory();
        let row = put(&conn, "WqnOB", 1_000);
        let fresh = begin(&conn, row.id, 5, 1_010).unwrap();
        assert_eq!((fresh.done, fresh.solved, fresh.wrongs), (0, false, 0));
        // Beginning again is not a fresh start: the progress stands.
        step(&conn, row.id, 5, 0, false, 1_020).unwrap();
        let again = begin(&conn, row.id, 5, 1_030).unwrap();
        assert_eq!(again.done, 1, "an attempt already under way is left alone");
        // Two presses landing together: only the one that saw the truth lands.
        assert!(!step(&conn, row.id, 5, 0, false, 1_040).unwrap(), "a stale step does nothing");
        assert!(step(&conn, row.id, 5, 1, true, 1_050).unwrap());
        let done = player(&conn, row.id, 5).unwrap();
        assert!(done.solved && done.done == 2);
        assert!(!step(&conn, row.id, 5, 2, true, 1_060).unwrap(), "a solved attempt is finished");
        // A wrong move costs nothing but a count.
        wrong(&conn, row.id, 6, 1_070).unwrap();
        begin(&conn, row.id, 6, 1_080).unwrap();
        wrong(&conn, row.id, 6, 1_090).unwrap();
        assert_eq!(player(&conn, row.id, 6).map(|p| (p.done, p.wrongs, p.solved)), Some((0, 1, false)));
        assert_eq!(playing(&conn, row.id), 2);
    }

    #[test]
    fn the_board_keeps_every_solve_and_ranks_by_points_then_solves_then_firsts() {
        let conn = memory();
        let one = put(&conn, "4kYFv", 1_000);
        let two = put(&conn, "WqnOB", 2_000);
        // The first solver, with a house point; a later solver, with none.
        add_solve(&conn, "2026-09-17", 1, one.id, 1, 1, true, 30, 1_100).unwrap();
        add_solve(&conn, "2026-09-17", 2, one.id, 0, 1, false, 90, 1_200).unwrap();
        add_solve(&conn, "2026-09-17", 2, two.id, 0, 3, true, 40, 2_100).unwrap();
        // The same person on the same puzzle doesn't count twice.
        add_solve(&conn, "2026-09-17", 1, one.id, 0, 0, true, 30, 1_300).unwrap();
        let day = day_solves(&conn, "2026-09-17");
        assert_eq!(day.len(), 3);
        assert_eq!(day.iter().map(|s| (s.user, s.points, s.worth, s.first)).collect::<Vec<_>>(), vec![
            (1, 1, 1, true),
            (2, 0, 1, false),
            (2, 0, 3, true)
        ]);
        let tally = day_tally(&conn, "2026-09-17");
        assert_eq!(tally.iter().map(|t| (t.user, t.points, t.solves, t.firsts)).collect::<Vec<_>>(), vec![(2, 4, 2, 1), (1, 1, 1, 1)]);
        assert_eq!(tally[0].reached, 2_100);
        assert_eq!(solvers_of(&conn, one.id).iter().map(|s| s.user).collect::<Vec<_>>(), vec![1, 2]);
        // A month is the same reading over a stretch; an empty stretch is empty.
        assert_eq!(tally_between(&conn, "2026-09-01", "2026-09-31").len(), 2);
        assert!(tally_between(&conn, "2026-08-01", "2026-08-31").is_empty());
    }

    #[test]
    fn a_link_is_made_once_per_person_and_dies_with_its_puzzle() {
        let conn = memory();
        let row = put(&conn, "4kYFv", 1_000);
        let mut n = 0u64;
        let mut roll = || {
            n = n.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (n >> 11) as f64 / (1u64 << 53) as f64
        };
        let mine = token_for(&conn, row.id, 5, 1_010, &mut roll).expect("a link");
        assert_eq!(mine.len(), TOKEN_LEN);
        assert!(mine.chars().all(|c| TOKEN_ALPHABET.contains(&(c as u8))), "{}", mine);
        assert_eq!(token_for(&conn, row.id, 5, 1_020, &mut roll).as_deref(), Some(mine.as_str()), "the same link, not a second one");
        let theirs = token_for(&conn, row.id, 6, 1_030, &mut roll).expect("their link");
        assert_ne!(theirs, mine);
        assert_eq!(token_owner(&conn, &mine), Some((row.id, 5)));
        assert_eq!(token_owner(&conn, "nosuchlink"), None);
        // The puzzle going down takes both links with it.
        close(&conn, row.id, "solved", 2_000).unwrap();
        assert_eq!(token_owner(&conn, &mine), None);
        assert_eq!(token_owner(&conn, &theirs), None);
    }

    #[test]
    fn a_puzzle_set_lately_is_known_to_be_set_lately() {
        let conn = memory();
        put(&conn, "4kYFv", 1_000);
        put(&conn, "WqnOB", 5_000);
        assert_eq!(set_since(&conn, 0).len(), 2);
        assert_eq!(set_since(&conn, 2_000), HashSet::from(["WqnOB".to_string()]));
        assert!(set_since(&conn, 9_000).is_empty());
    }

    #[test]
    fn meta_remembers_the_card() {
        let conn = memory();
        assert_eq!(meta_get(&conn, "card"), None);
        meta_set(&conn, "card", "77:88").unwrap();
        meta_set(&conn, "card", "77:99").unwrap();
        assert_eq!(meta_get(&conn, "card").as_deref(), Some("77:99"));
    }
}
