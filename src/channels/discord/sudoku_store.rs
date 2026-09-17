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
//! list but scores nothing.
//!
//! Every solve also records what it was WORTH in sudoku points: the puzzle's
//! own value with that player's hints taken off. Sudoku points are the game's
//! own score — they are kept for every solver, houses or no houses, and they
//! don't move the House Cup.

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
        points INTEGER NOT NULL DEFAULT 0, worth INTEGER NOT NULL DEFAULT 0, kind TEXT NOT NULL DEFAULT 'win',
        difficulty TEXT NOT NULL DEFAULT '', seconds INTEGER NOT NULL DEFAULT 0,
        ts INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (user_id, puzzle_id));
    CREATE INDEX IF NOT EXISTS solves_day ON solves (day);
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);";

/// Columns added after the first release, for a database made before them. The
/// game is live, so they go on with `ALTER TABLE`: nothing is ever rewritten or
/// dropped under a puzzle somebody is solving.
const ADDED_COLUMNS: &[(&str, &str, &str)] = &[("solves", "worth", "INTEGER NOT NULL DEFAULT 0")];

/// What the solves written before `worth` existed were worth, worked out from
/// the puzzle each was for: its difficulty's own value. Hints are not taken off
/// here — what one cost is a setting that has moved since, and the puzzle's
/// value is the honest thing the database still knows. A finish was never worth
/// anything, and a solve whose puzzle has gone keeps the zero. Run once, the
/// moment the column is added.
const BACKFILL_WORTH: &str = "
    UPDATE solves SET worth = (SELECT MAX(p.points, 0) FROM puzzles p WHERE p.id = solves.puzzle_id)
    WHERE worth = 0 AND kind = 'win' AND puzzle_id IN (SELECT id FROM puzzles);";

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
    /// First to the puzzle: the one that scores.
    Win,
    /// Right, but somebody else had already solved it. Worth nothing.
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
    /// The HOUSE points the ledger paid. Sudoku stopped paying them: this is
    /// zero for every solve written since, and keeps whatever the old ones were
    /// paid, which is how the history stays true.
    pub points: i64,
    /// The SUDOKU points the solve was worth: the puzzle's own value with this
    /// player's hints taken off, and no cap of any kind. Every solver gets these,
    /// houses or no houses — they are the game's own score.
    pub worth: i64,
    pub kind: Kind,
    pub level: Level,
    pub seconds: i64,
    pub ts: i64,
}

/// Writes a solve. A player counts once per puzzle, and a win never turns back
/// into a finish.
#[allow(clippy::too_many_arguments)]
pub fn add_solve(
    conn: &Connection,
    day: &str,
    user: u64,
    puzzle: i64,
    points: i64,
    worth: i64,
    kind: Kind,
    level: Level,
    seconds: i64,
    ts: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO solves (day, user_id, puzzle_id, points, worth, kind, difficulty, seconds, ts) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(user_id, puzzle_id) DO UPDATE SET
            points = MAX(solves.points, excluded.points),
            worth = MAX(solves.worth, excluded.worth),
            kind = CASE WHEN solves.kind = 'win' THEN 'win' ELSE excluded.kind END",
        params![day, user as i64, puzzle, points, worth, kind.key(), level.key(), seconds, ts],
    )
    .map(|_| ())
}

fn solve_row(r: &rusqlite::Row) -> rusqlite::Result<Solve> {
    Ok(Solve {
        user: r.get::<_, i64>(0)? as u64,
        puzzle: r.get(1)?,
        points: r.get(2)?,
        worth: r.get(3)?,
        kind: if r.get::<_, String>(4)? == "win" { Kind::Win } else { Kind::Finish },
        level: Level::from_key(&r.get::<_, String>(5)?).unwrap_or(Level::Medium),
        seconds: r.get(6)?,
        ts: r.get(7)?,
    })
}

/// A day's solves, first one first.
pub fn day_solves(conn: &Connection, day: &str) -> Vec<Solve> {
    let Ok(mut stmt) = conn
        .prepare("SELECT user_id, puzzle_id, points, worth, kind, difficulty, seconds, ts FROM solves WHERE day = ?1 ORDER BY ts, puzzle_id")
    else {
        return Vec::new();
    };
    stmt.query_map(params![day], solve_row).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

// --- sudoku points ---------------------------------------------------------------------

/// One player's sudoku points over a stretch of days.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tally {
    pub user: u64,
    /// Sudoku points: every puzzle they won at what it was worth to them.
    pub points: i64,
    /// How many puzzles they won. A finish is worth nothing and is not one.
    pub solves: i64,
    /// When they got to that total — their last win of the stretch.
    pub reached: i64,
}

/// The order a board reads in, and the order the day's card is decided in: most
/// sudoku points, then most puzzles won, then whoever got there first.
pub fn rank(rows: &mut [Tally]) {
    rows.sort_by(|a, b| b.points.cmp(&a.points).then(b.solves.cmp(&a.solves)).then(a.reached.cmp(&b.reached)).then(a.user.cmp(&b.user)));
}

/// Everyone's sudoku points between two days, both ends included (YYYY-MM-DD,
/// which sorts as a date), best first. Wins only: a finish scored nothing and
/// would otherwise put someone on the board at nought.
pub fn tally_between(conn: &Connection, from_day: &str, to_day: &str) -> Vec<Tally> {
    let sql = "SELECT user_id, COALESCE(SUM(worth), 0), COUNT(*), COALESCE(MAX(ts), 0)
               FROM solves WHERE kind = 'win' AND day >= ?1 AND day <= ?2 GROUP BY user_id";
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

/// One day's sudoku points, best first. What the day's frog card is decided on.
pub fn day_tally(conn: &Connection, day: &str) -> Vec<Tally> {
    tally_between(conn, day, day)
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
        add_solve(&conn, "2026-09-16", 1, p.id, 0, 4, Kind::Win, Level::Medium, 120, 100).unwrap();
        add_solve(&conn, "2026-09-16", 2, p.id, 0, 0, Kind::Finish, Level::Medium, 300, 200).unwrap();
        // The same person again on the same puzzle doesn't count twice, and a
        // win is never written back down to a finish.
        add_solve(&conn, "2026-09-16", 1, p.id, 0, 0, Kind::Finish, Level::Medium, 400, 300).unwrap();
        let day = day_solves(&conn, "2026-09-16");
        assert_eq!(day.len(), 2);
        assert_eq!((day[0].user, day[0].worth, day[0].kind), (1, 4, Kind::Win));
        assert_eq!((day[1].user, day[1].worth, day[1].kind), (2, 0, Kind::Finish));
        assert_eq!(solved_by(&conn, p.id, 1), Some(Kind::Win));
        assert_eq!(solved_by(&conn, p.id, 2), Some(Kind::Finish));
        assert_eq!(solved_by(&conn, p.id, 3), None);
        assert!(day_solves(&conn, "2026-09-17").is_empty());
    }

    #[test]
    fn what_a_puzzle_was_worth_is_kept_for_everyone_the_ledger_pays_nothing() {
        let conn = memory();
        let easy = put(&conn, Level::Easy, 2, 10);
        let hard = put(&conn, Level::Hard, 6, 20);
        // Sudoku pays no house points at all now: every row's `points` is nought
        // and the sudoku points are the whole of the score.
        add_solve(&conn, "2026-09-16", 1, easy.id, 0, 2, Kind::Win, Level::Easy, 40, 100).unwrap();
        // A mod, who has no house and never had a ledger row, scores the same way.
        add_solve(&conn, "2026-09-16", 9, hard.id, 0, 6, Kind::Win, Level::Hard, 20, 120).unwrap();
        let day = day_solves(&conn, "2026-09-16");
        assert_eq!(day.iter().map(|s| (s.user, s.points, s.worth)).collect::<Vec<_>>(), vec![(1, 0, 2), (9, 0, 6)]);
        let tally = day_tally(&conn, "2026-09-16");
        assert_eq!(tally.iter().map(|t| (t.user, t.points, t.solves)).collect::<Vec<_>>(), vec![(9, 6, 1), (1, 2, 1)]);
        assert_eq!(tally[0].reached, 120);
        // Hints come off what the solve was worth, and the store keeps that.
        let hinted = put(&conn, Level::Hard, 6, 30);
        add_solve(&conn, "2026-09-17", 1, hinted.id, 0, 4, Kind::Win, Level::Hard, 300, 200).unwrap();
        assert_eq!(day_tally(&conn, "2026-09-17").first().map(|t| (t.user, t.points)), Some((1, 4)));
        // A finish scores nothing and doesn't put anyone on the board.
        let late = put(&conn, Level::Easy, 2, 40);
        add_solve(&conn, "2026-09-18", 3, late.id, 0, 0, Kind::Finish, Level::Easy, 900, 300).unwrap();
        assert!(day_tally(&conn, "2026-09-18").is_empty(), "a finish is not a win");
    }

    #[test]
    fn the_board_is_points_then_puzzles_then_whoever_got_there_first() {
        let conn = memory();
        let puzzle = |n: i64, points: i64| put(&conn, Level::Medium, points, n).id;
        let (a, b, c, d, e) = (puzzle(1, 2), puzzle(2, 2), puzzle(3, 6), puzzle(4, 4), puzzle(5, 4));
        // 1: two easy ones. 2: one hard one — fewer puzzles, more points.
        add_solve(&conn, "2026-09-16", 1, a, 0, 2, Kind::Win, Level::Easy, 5, 100).unwrap();
        add_solve(&conn, "2026-09-16", 1, b, 0, 2, Kind::Win, Level::Easy, 5, 110).unwrap();
        add_solve(&conn, "2026-09-16", 2, c, 0, 6, Kind::Win, Level::Hard, 5, 120).unwrap();
        // 3 ties 1 on points with fewer puzzles; 4 ties both and finished later.
        add_solve(&conn, "2026-09-16", 3, d, 0, 4, Kind::Win, Level::Medium, 5, 130).unwrap();
        add_solve(&conn, "2026-09-16", 4, e, 0, 4, Kind::Win, Level::Medium, 5, 140).unwrap();
        let tally = day_tally(&conn, "2026-09-16");
        assert_eq!(tally.iter().map(|t| (t.user, t.points, t.solves)).collect::<Vec<_>>(), vec![(2, 6, 1), (1, 4, 2), (3, 4, 1), (4, 4, 1)]);
        // A month is the same reading over a stretch of days, and a stretch with
        // nothing in it is empty rather than wrong.
        add_solve(&conn, "2026-09-02", 4, puzzle(6, 6), 0, 6, Kind::Win, Level::Hard, 5, 90).unwrap();
        let month = tally_between(&conn, "2026-09-01", "2026-09-31");
        assert_eq!(month.first().map(|t| (t.user, t.points, t.solves)), Some((4, 10, 2)));
        assert!(tally_between(&conn, "2026-08-01", "2026-08-31").is_empty());
    }

    #[test]
    fn the_worth_column_goes_onto_a_database_that_is_already_being_played_in() {
        // Exactly the schema as it shipped, rows and all: the live database.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE puzzles (
                id INTEGER PRIMARY KEY AUTOINCREMENT, givens TEXT NOT NULL, solution TEXT NOT NULL,
                difficulty TEXT NOT NULL, points INTEGER NOT NULL, posted_ts INTEGER NOT NULL,
                message_id INTEGER, channel_id INTEGER NOT NULL, status TEXT NOT NULL DEFAULT 'open',
                winner INTEGER, solved_ts INTEGER, seconds INTEGER);
             CREATE TABLE players (
                puzzle_id INTEGER NOT NULL, user_id INTEGER NOT NULL, started_ts INTEGER NOT NULL,
                hints INTEGER NOT NULL DEFAULT 0, hint_cells TEXT NOT NULL DEFAULT '',
                tries INTEGER NOT NULL DEFAULT 0, last_try_ts INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (puzzle_id, user_id));
             CREATE TABLE solves (
                day TEXT NOT NULL, user_id INTEGER NOT NULL, puzzle_id INTEGER NOT NULL,
                points INTEGER NOT NULL DEFAULT 0, kind TEXT NOT NULL DEFAULT 'win',
                difficulty TEXT NOT NULL DEFAULT '', seconds INTEGER NOT NULL DEFAULT 0,
                ts INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (user_id, puzzle_id));
             CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO puzzles (id, givens, solution, difficulty, points, posted_ts, channel_id, status, winner)
                VALUES (1, '', '', 'easy', 2, 10, 77, 'solved', 5),
                       (2, '', '', 'hard', 6, 20, 77, 'solved', 6),
                       (3, '', '', 'medium', 4, 30, 77, 'solved', 7);
             INSERT INTO solves (day, user_id, puzzle_id, points, kind, difficulty, seconds, ts)
                VALUES ('2026-09-15', 5, 1, 2, 'win', 'easy', 40, 100),
                       ('2026-09-15', 6, 2, 0, 'win', 'hard', 20, 200),
                       ('2026-09-15', 7, 3, 0, 'finish', 'medium', 900, 300),
                       ('2026-09-15', 8, 9, 4, 'win', 'medium', 15, 400);",
        )
        .unwrap();
        init(&conn).unwrap();
        // Nothing was dropped or rewritten: every solve and every puzzle survived,
        // and the house points already paid are exactly as they were.
        let kept: i64 = conn.query_row("SELECT COUNT(*) FROM solves", [], |r| r.get(0)).unwrap();
        assert_eq!(kept, 4, "every solve survived the migration");
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM puzzles", [], |r| r.get::<_, i64>(0)).unwrap(), 3);
        let day = day_solves(&conn, "2026-09-15");
        // Worked out from the puzzle's difficulty. A capped win (paid 0) is worth
        // its puzzle all the same; a finish stays at nought; and nothing is
        // claimed for a solve whose puzzle has gone.
        assert_eq!(
            day.iter().map(|s| (s.user, s.points, s.worth)).collect::<Vec<_>>(),
            vec![(5, 2, 2), (6, 0, 6), (7, 0, 0), (8, 4, 0)]
        );
        // And running it again changes nothing.
        init(&conn).unwrap();
        assert_eq!(day_solves(&conn, "2026-09-15"), day);
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
