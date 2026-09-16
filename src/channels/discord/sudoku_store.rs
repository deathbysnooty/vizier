//! What Sudoku keeps in `.runtime/sudoku.db`: every puzzle with its answer,
//! who was playing each one, and every solve.
//!
//! A puzzle moves `open` → `solved`, or `open` → `skipped` when a mod presses
//! on. Only the FIRST correct code wins: the move to `solved` is one
//! `UPDATE … WHERE status = 'open'`, so two codes arriving together can only
//! make one winner, and the loser is told who beat them.
//!
//! Old puzzles are never thrown away. Their answers stay so that someone who
//! was half way through when the channel moved on can still paste their code
//! and be told whether it was right — a "finish", which counts in the day's
//! list but is worth no house points.

use std::sync::OnceLock;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

use super::sudoku_gen::{Grid, Level, Puzzle, grid_from_str, grid_to_str};

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

pub const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS puzzles (
        id INTEGER PRIMARY KEY AUTOINCREMENT, givens TEXT NOT NULL, solution TEXT NOT NULL,
        difficulty TEXT NOT NULL, points INTEGER NOT NULL, posted_ts INTEGER NOT NULL,
        message_id INTEGER, channel_id INTEGER NOT NULL, status TEXT NOT NULL DEFAULT 'open',
        winner INTEGER, solved_ts INTEGER, seconds INTEGER);
    CREATE INDEX IF NOT EXISTS puzzles_status ON puzzles (status);
    CREATE INDEX IF NOT EXISTS puzzles_posted ON puzzles (posted_ts);
    CREATE TABLE IF NOT EXISTS players (
        puzzle_id INTEGER NOT NULL, user_id INTEGER NOT NULL, started_ts INTEGER NOT NULL,
        hints INTEGER NOT NULL DEFAULT 0, hint_cells TEXT NOT NULL DEFAULT '',
        tries INTEGER NOT NULL DEFAULT 0, last_try_ts INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (puzzle_id, user_id));
    CREATE TABLE IF NOT EXISTS solves (
        day TEXT NOT NULL, user_id INTEGER NOT NULL, puzzle_id INTEGER NOT NULL,
        points INTEGER NOT NULL DEFAULT 0, kind TEXT NOT NULL DEFAULT 'win',
        difficulty TEXT NOT NULL DEFAULT '', seconds INTEGER NOT NULL DEFAULT 0,
        ts INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (user_id, puzzle_id));
    CREATE INDEX IF NOT EXISTS solves_day ON solves (day);
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);";

pub fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)
}

pub fn open(workspace: &str) -> anyhow::Result<()> {
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    std::fs::create_dir_all(&dir)?;
    let conn = Connection::open(dir.join("sudoku.db"))?;
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

// --- puzzles ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Up in the channel, nobody has solved it.
    Open,
    Solved,
    /// A mod pressed on before anyone solved it.
    Skipped,
}

impl Status {
    pub fn key(self) -> &'static str {
        match self {
            Status::Open => "open",
            Status::Solved => "solved",
            Status::Skipped => "skipped",
        }
    }

    fn from_key(key: &str) -> Status {
        match key {
            "solved" => Status::Solved,
            "skipped" => Status::Skipped,
            _ => Status::Open,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: i64,
    pub givens: Grid,
    pub solution: Grid,
    pub level: Level,
    /// What solving it was worth when it was posted.
    pub points: i64,
    pub posted_ts: i64,
    pub message: Option<u64>,
    pub channel: u64,
    pub status: Status,
    pub winner: Option<u64>,
    pub solved_ts: Option<i64>,
    /// How long the winner took.
    pub seconds: Option<i64>,
}

const COLS: &str = "id, givens, solution, difficulty, points, posted_ts, message_id, channel_id, status, winner, solved_ts, seconds";

fn row(r: &rusqlite::Row) -> rusqlite::Result<Row> {
    let text = |i: usize| -> rusqlite::Result<String> { r.get(i) };
    Ok(Row {
        id: r.get(0)?,
        givens: grid_from_str(&text(1)?).unwrap_or([0; 81]),
        solution: grid_from_str(&text(2)?).unwrap_or([0; 81]),
        level: Level::from_key(&text(3)?).unwrap_or(Level::Medium),
        points: r.get(4)?,
        posted_ts: r.get(5)?,
        message: r.get::<_, Option<i64>>(6)?.map(|m| m as u64),
        channel: r.get::<_, i64>(7)? as u64,
        status: Status::from_key(&text(8)?),
        winner: r.get::<_, Option<i64>>(9)?.map(|w| w as u64),
        solved_ts: r.get(10)?,
        seconds: r.get(11)?,
    })
}

/// Writes a fresh puzzle and returns it with its number.
pub fn add_puzzle(conn: &Connection, p: &Puzzle, points: i64, channel: u64, now: i64) -> rusqlite::Result<Row> {
    conn.execute(
        "INSERT INTO puzzles (givens, solution, difficulty, points, posted_ts, channel_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![grid_to_str(&p.givens), grid_to_str(&p.solution), p.level.key(), points, now, channel as i64],
    )?;
    get(conn, conn.last_insert_rowid()).ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn get(conn: &Connection, id: i64) -> Option<Row> {
    conn.query_row(&format!("SELECT {} FROM puzzles WHERE id = ?1", COLS), params![id], row).optional().ok().flatten()
}

/// The puzzle that is up now, if any.
pub fn live(conn: &Connection) -> Option<Row> {
    conn.query_row(&format!("SELECT {} FROM puzzles WHERE status = 'open' ORDER BY id DESC LIMIT 1", COLS), [], row).optional().ok().flatten()
}

/// The newest puzzles, however they ended.
pub fn recent(conn: &Connection, limit: usize) -> Vec<Row> {
    let sql = format!("SELECT {} FROM puzzles ORDER BY id DESC LIMIT ?1", COLS);
    let Ok(mut stmt) = conn.prepare(&sql) else { return Vec::new() };
    stmt.query_map(params![limit as i64], row).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

/// Puzzles whose codes are still worth checking: posted inside the window, or
/// among the newest `keep` however old they are. Newest first.
pub fn checkable(conn: &Connection, since: i64, keep: usize) -> Vec<Row> {
    let sql = format!(
        "SELECT {} FROM puzzles WHERE posted_ts >= ?1 OR id > (SELECT COALESCE(MAX(id), 0) - ?2 FROM puzzles) ORDER BY id DESC",
        COLS
    );
    let Ok(mut stmt) = conn.prepare(&sql) else { return Vec::new() };
    stmt.query_map(params![since, keep as i64], row).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

pub fn set_message(conn: &Connection, id: i64, message: u64) -> rusqlite::Result<()> {
    conn.execute("UPDATE puzzles SET message_id = ?2 WHERE id = ?1", params![id, message as i64]).map(|_| ())
}

/// The one move that decides a race: `open` → `solved`, only the first wins.
/// True for the winner, false for everyone who came second.
pub fn claim(conn: &Connection, id: i64, user: u64, now: i64) -> rusqlite::Result<bool> {
    conn.execute(
        "UPDATE puzzles SET status = 'solved', winner = ?2, solved_ts = ?3,
         seconds = MAX(?3 - posted_ts, 0) WHERE id = ?1 AND status = 'open'",
        params![id, user as i64, now],
    )
    .map(|n| n > 0)
}

/// A mod skipping the live puzzle. True when there was one to skip.
pub fn skip(conn: &Connection, id: i64, now: i64) -> rusqlite::Result<bool> {
    conn.execute("UPDATE puzzles SET status = 'skipped', solved_ts = ?2 WHERE id = ?1 AND status = 'open'", params![id, now])
        .map(|n| n > 0)
}

// --- players ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Player {
    pub started_ts: i64,
    pub hints: i64,
    /// The squares already given away to this player, as cell numbers.
    pub hint_cells: Vec<usize>,
    pub tries: i64,
    pub last_try_ts: i64,
}

fn cells_from(text: &str) -> Vec<usize> {
    text.split(',').filter_map(|p| p.trim().parse().ok()).filter(|c| *c < 81).collect()
}

fn cells_to(cells: &[usize]) -> String {
    cells.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(",")
}

/// Notes that someone has started this puzzle. True the first time.
pub fn start_playing(conn: &Connection, puzzle: i64, user: u64, now: i64) -> rusqlite::Result<bool> {
    conn.execute(
        "INSERT OR IGNORE INTO players (puzzle_id, user_id, started_ts) VALUES (?1, ?2, ?3)",
        params![puzzle, user as i64, now],
    )
    .map(|n| n > 0)
}

pub fn player(conn: &Connection, puzzle: i64, user: u64) -> Option<Player> {
    conn.query_row(
        "SELECT started_ts, hints, hint_cells, tries, last_try_ts FROM players WHERE puzzle_id = ?1 AND user_id = ?2",
        params![puzzle, user as i64],
        |r| {
            Ok(Player {
                started_ts: r.get(0)?,
                hints: r.get(1)?,
                hint_cells: cells_from(&r.get::<_, String>(2)?),
                tries: r.get(3)?,
                last_try_ts: r.get(4)?,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

pub fn playing(conn: &Connection, puzzle: i64) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM players WHERE puzzle_id = ?1", params![puzzle], |r| r.get(0)).unwrap_or(0)
}

/// Everyone who opened this puzzle, in the order they started.
pub fn players(conn: &Connection, puzzle: i64) -> Vec<u64> {
    let Ok(mut stmt) = conn.prepare("SELECT user_id FROM players WHERE puzzle_id = ?1 ORDER BY started_ts, user_id") else {
        return Vec::new();
    };
    stmt.query_map(params![puzzle], |r| r.get::<_, i64>(0)).map(|rows| rows.flatten().map(|u| u as u64).collect()).unwrap_or_default()
}

/// Hints handed to everyone on this puzzle.
pub fn hints_used(conn: &Connection, puzzle: i64) -> i64 {
    conn.query_row("SELECT COALESCE(SUM(hints), 0) FROM players WHERE puzzle_id = ?1", params![puzzle], |r| r.get(0)).unwrap_or(0)
}

/// Writes down a hint and returns how many that player has now had.
pub fn add_hint(conn: &Connection, puzzle: i64, user: u64, cell: usize, now: i64) -> rusqlite::Result<i64> {
    start_playing(conn, puzzle, user, now)?;
    let mut player = player(conn, puzzle, user).unwrap_or_default();
    if !player.hint_cells.contains(&cell) {
        player.hint_cells.push(cell);
    }
    player.hints = player.hint_cells.len() as i64;
    conn.execute(
        "UPDATE players SET hints = ?3, hint_cells = ?4 WHERE puzzle_id = ?1 AND user_id = ?2",
        params![puzzle, user as i64, player.hints, cells_to(&player.hint_cells)],
    )?;
    Ok(player.hints)
}

/// Counts a try against a player and returns how many they have now used.
pub fn add_try(conn: &Connection, puzzle: i64, user: u64, now: i64) -> rusqlite::Result<i64> {
    start_playing(conn, puzzle, user, now)?;
    conn.execute(
        "UPDATE players SET tries = tries + 1, last_try_ts = ?3 WHERE puzzle_id = ?1 AND user_id = ?2",
        params![puzzle, user as i64, now],
    )?;
    Ok(player(conn, puzzle, user).map(|p| p.tries).unwrap_or(0))
}

// --- solves ----------------------------------------------------------------------------

/// How a solve counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// First to the puzzle: the one that pays house points.
    Win,
    /// Right, but somebody else had already solved it. No points.
    Finish,
}

impl Kind {
    pub fn key(self) -> &'static str {
        match self {
            Kind::Win => "win",
            Kind::Finish => "finish",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Solve {
    pub user: u64,
    pub puzzle: i64,
    pub points: i64,
    pub kind: Kind,
    pub level: Level,
    pub seconds: i64,
    pub ts: i64,
}

/// Writes a solve. A player counts once per puzzle, and a win never turns back
/// into a finish.
#[allow(clippy::too_many_arguments)]
pub fn add_solve(conn: &Connection, day: &str, user: u64, puzzle: i64, points: i64, kind: Kind, level: Level, seconds: i64, ts: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO solves (day, user_id, puzzle_id, points, kind, difficulty, seconds, ts) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(user_id, puzzle_id) DO UPDATE SET
            points = MAX(solves.points, excluded.points),
            kind = CASE WHEN solves.kind = 'win' THEN 'win' ELSE excluded.kind END",
        params![day, user as i64, puzzle, points, kind.key(), level.key(), seconds, ts],
    )
    .map(|_| ())
}

fn solve_row(r: &rusqlite::Row) -> rusqlite::Result<Solve> {
    Ok(Solve {
        user: r.get::<_, i64>(0)? as u64,
        puzzle: r.get(1)?,
        points: r.get(2)?,
        kind: if r.get::<_, String>(3)? == "win" { Kind::Win } else { Kind::Finish },
        level: Level::from_key(&r.get::<_, String>(4)?).unwrap_or(Level::Medium),
        seconds: r.get(5)?,
        ts: r.get(6)?,
    })
}

/// A day's solves, first one first.
pub fn day_solves(conn: &Connection, day: &str) -> Vec<Solve> {
    let Ok(mut stmt) =
        conn.prepare("SELECT user_id, puzzle_id, points, kind, difficulty, seconds, ts FROM solves WHERE day = ?1 ORDER BY ts, puzzle_id")
    else {
        return Vec::new();
    };
    stmt.query_map(params![day], solve_row).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

/// Whether this player has already been counted on this puzzle.
pub fn solved_by(conn: &Connection, puzzle: i64, user: u64) -> Option<Kind> {
    conn.query_row("SELECT kind FROM solves WHERE puzzle_id = ?1 AND user_id = ?2", params![puzzle, user as i64], |r| r.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
        .map(|k| if k == "win" { Kind::Win } else { Kind::Finish })
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::channels::discord::sudoku_gen::{Rng, generate};

    pub fn memory() -> Connection {
        let conn = Connection::open_in_memory().expect("memory db");
        init(&conn).expect("schema");
        conn
    }

    pub fn put(conn: &Connection, level: Level, points: i64, now: i64) -> Row {
        let p = generate(level, &mut Rng::seeded(now as u64 * 31 + points as u64));
        add_puzzle(conn, &p, points, 77, now).expect("puzzle")
    }

    #[test]
    fn only_the_first_correct_code_claims_a_puzzle() {
        let conn = memory();
        let p = put(&conn, Level::Medium, 4, 1_000);
        assert_eq!(live(&conn).map(|l| l.id), Some(p.id));
        assert!(claim(&conn, p.id, 11, 1_400).unwrap(), "the first gets it");
        assert!(!claim(&conn, p.id, 22, 1_401).unwrap(), "the second is too late");
        let after = get(&conn, p.id).unwrap();
        assert_eq!((after.status, after.winner, after.seconds), (Status::Solved, Some(11), Some(400)));
        assert!(live(&conn).is_none(), "a solved puzzle is no longer the live one");
        // Skipping a solved puzzle does nothing.
        assert!(!skip(&conn, p.id, 1_500).unwrap());
    }

    #[test]
    fn a_mod_can_skip_the_live_puzzle() {
        let conn = memory();
        let p = put(&conn, Level::Easy, 2, 500);
        assert!(skip(&conn, p.id, 600).unwrap());
        assert_eq!(get(&conn, p.id).unwrap().status, Status::Skipped);
        assert!(!claim(&conn, p.id, 5, 700).unwrap(), "a skipped puzzle can't be won");
        assert!(live(&conn).is_none());
    }

    #[test]
    fn players_hints_and_tries_are_kept_per_puzzle() {
        let conn = memory();
        let p = put(&conn, Level::Hard, 6, 10);
        assert!(start_playing(&conn, p.id, 1, 20).unwrap());
        assert!(!start_playing(&conn, p.id, 1, 30).unwrap(), "pressing Play twice is still one player");
        start_playing(&conn, p.id, 2, 40).unwrap();
        assert_eq!(playing(&conn, p.id), 2);
        assert_eq!(players(&conn, p.id), vec![1, 2]);
        assert_eq!(add_hint(&conn, p.id, 1, 40, 50).unwrap(), 1);
        assert_eq!(add_hint(&conn, p.id, 1, 40, 51).unwrap(), 1, "the same square again is not a new hint");
        assert_eq!(add_hint(&conn, p.id, 1, 41, 52).unwrap(), 2);
        assert_eq!(hints_used(&conn, p.id), 2);
        assert_eq!(player(&conn, p.id, 1).unwrap().hint_cells, vec![40, 41]);
        assert_eq!(add_try(&conn, p.id, 2, 60).unwrap(), 1);
        assert_eq!(add_try(&conn, p.id, 2, 70).unwrap(), 2);
        assert_eq!(player(&conn, p.id, 2).unwrap().last_try_ts, 70);
        // Another puzzle starts everyone again.
        let other = put(&conn, Level::Easy, 2, 99);
        assert_eq!(playing(&conn, other.id), 0);
        assert_eq!(player(&conn, other.id, 1), None);
    }

    #[test]
    fn a_day_of_solves_keeps_wins_and_finishes_apart() {
        let conn = memory();
        let p = put(&conn, Level::Medium, 4, 10);
        add_solve(&conn, "2026-09-16", 1, p.id, 4, Kind::Win, Level::Medium, 120, 100).unwrap();
        add_solve(&conn, "2026-09-16", 2, p.id, 0, Kind::Finish, Level::Medium, 300, 200).unwrap();
        // The same person again on the same puzzle doesn't count twice, and a
        // win is never written back down to a finish.
        add_solve(&conn, "2026-09-16", 1, p.id, 0, Kind::Finish, Level::Medium, 400, 300).unwrap();
        let day = day_solves(&conn, "2026-09-16");
        assert_eq!(day.len(), 2);
        assert_eq!((day[0].user, day[0].points, day[0].kind), (1, 4, Kind::Win));
        assert_eq!((day[1].user, day[1].points, day[1].kind), (2, 0, Kind::Finish));
        assert_eq!(solved_by(&conn, p.id, 1), Some(Kind::Win));
        assert_eq!(solved_by(&conn, p.id, 2), Some(Kind::Finish));
        assert_eq!(solved_by(&conn, p.id, 3), None);
        assert!(day_solves(&conn, "2026-09-17").is_empty());
    }

    #[test]
    fn old_puzzles_stay_checkable_for_a_while() {
        let conn = memory();
        let day = 86_400;
        let old = put(&conn, Level::Easy, 2, 1_000);
        let recent_one = put(&conn, Level::Medium, 4, 1_000 + day);
        let now = 1_000 + day + 60;
        let window = now - day;
        let ids: Vec<i64> = checkable(&conn, window, 10).iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![recent_one.id, old.id], "the last ten are checkable however old");
        let ids: Vec<i64> = checkable(&conn, window, 1).iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![recent_one.id], "outside the window and outside the last few: too old");
        assert_eq!(recent(&conn, 1).len(), 1);
        // Every puzzle keeps its answer for good.
        assert!(get(&conn, old.id).unwrap().solution.iter().all(|d| *d != 0));
    }

    #[test]
    fn meta_remembers_the_card() {
        let conn = memory();
        assert_eq!(meta_get(&conn, "card"), None);
        meta_set(&conn, "card", "5:6").unwrap();
        meta_set(&conn, "card", "7:8").unwrap();
        assert_eq!(meta_get(&conn, "card").as_deref(), Some("7:8"));
    }
}
