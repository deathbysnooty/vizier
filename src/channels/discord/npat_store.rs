//! What Name Place Animal Thing keeps in `.runtime/npat.db`: every game, its
//! rounds (one per letter), each player's last submission, the verdict on every
//! answer, each letter's scores, each game's totals and places (with the house
//! points each place is owed and what the ledger actually credited),
//! challenges, and the verdict cache.
//!
//! The cache (`verdicts`) remembers how an answer was judged for a letter and
//! category, by its folded form, so the model is only asked about answers it
//! hasn't seen. A mod's review writes there too and always wins over the
//! model; letter-only verdicts are never remembered.
//!
//! A round moves `open` → `judging` → `done`, or to `stopped` from either of
//! the first two; a game moves `running` → `done` or `stopped`. Each move is one
//! `UPDATE … WHERE status = …`, so a stop that lands while the model is judging
//! wins and nothing is paid.

use std::collections::HashMap;
use std::sync::OnceLock;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

use super::npat_judge::{Judged, LetterScore, Prizes, Scored, Standing, Verdict};

/// Answers still count this long after the timer, for a pop-up sent at the buzzer.
pub const GRACE_SECS: i64 = 3;
/// What `submissions.house` holds for someone who stepped out of the houses.
pub const MUGGLE: &str = "muggle";
/// A mod plays like a Muggle: they are in no house, so they never take house
/// points, and they don't count towards the "two houses" rule.
pub const MODERATOR: &str = "mod";

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

pub const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS rounds (
        id INTEGER PRIMARY KEY AUTOINCREMENT, channel_id INTEGER NOT NULL, message_id INTEGER,
        results_message_id INTEGER, letter TEXT NOT NULL, started_at INTEGER NOT NULL, ends_at INTEGER NOT NULL,
        status TEXT NOT NULL DEFAULT 'open', letter_only INTEGER NOT NULL DEFAULT 0, pays INTEGER NOT NULL DEFAULT 0,
        paid_out INTEGER NOT NULL DEFAULT 0, judged_at INTEGER, day TEXT NOT NULL DEFAULT '', stopped_by INTEGER);
    CREATE INDEX IF NOT EXISTS rounds_status ON rounds (status);
    CREATE INDEX IF NOT EXISTS rounds_day ON rounds (day);
    CREATE TABLE IF NOT EXISTS submissions (
        round_id INTEGER NOT NULL, user_id INTEGER NOT NULL, name TEXT NOT NULL DEFAULT '', place TEXT NOT NULL DEFAULT '',
        animal TEXT NOT NULL DEFAULT '', thing TEXT NOT NULL DEFAULT '', ts INTEGER NOT NULL, first_ts INTEGER NOT NULL,
        house TEXT NOT NULL DEFAULT '', PRIMARY KEY (round_id, user_id));
    CREATE TABLE IF NOT EXISTS round_verdicts (
        round_id INTEGER NOT NULL, user_id INTEGER NOT NULL, category INTEGER NOT NULL, answer TEXT NOT NULL,
        valid INTEGER NOT NULL, canonical TEXT NOT NULL DEFAULT '', reviewed_by INTEGER, reviewed_ts INTEGER,
        PRIMARY KEY (round_id, user_id, category));
    CREATE TABLE IF NOT EXISTS scores (
        round_id INTEGER NOT NULL, user_id INTEGER NOT NULL, score INTEGER NOT NULL, unique_count INTEGER NOT NULL,
        place INTEGER NOT NULL DEFAULT 0, owed INTEGER NOT NULL DEFAULT 0,
        credited INTEGER NOT NULL DEFAULT 0, fixes INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (round_id, user_id));
    CREATE TABLE IF NOT EXISTS challenges (
        round_id INTEGER NOT NULL, user_id INTEGER NOT NULL, category INTEGER NOT NULL, ts INTEGER NOT NULL,
        resolved_by INTEGER, PRIMARY KEY (round_id, user_id, category));
    CREATE TABLE IF NOT EXISTS verdicts (
        letter TEXT NOT NULL, category INTEGER NOT NULL, folded_answer TEXT NOT NULL, valid INTEGER NOT NULL,
        canonical TEXT NOT NULL DEFAULT '', source TEXT NOT NULL, ts INTEGER NOT NULL,
        PRIMARY KEY (letter, category, folded_answer));
    CREATE TABLE IF NOT EXISTS games (
        id INTEGER PRIMARY KEY AUTOINCREMENT, channel_id INTEGER NOT NULL, started_at INTEGER NOT NULL,
        letters INTEGER NOT NULL, status TEXT NOT NULL DEFAULT 'running', pays INTEGER NOT NULL DEFAULT 0,
        paid_out INTEGER NOT NULL DEFAULT 0, finished_at INTEGER, day TEXT NOT NULL DEFAULT '', ended_early INTEGER NOT NULL DEFAULT 0,
        results_message_id INTEGER, stopped_by INTEGER);
    CREATE INDEX IF NOT EXISTS games_status ON games (status);
    CREATE TABLE IF NOT EXISTS game_scores (
        game_id INTEGER NOT NULL, user_id INTEGER NOT NULL, total INTEGER NOT NULL, reached_at INTEGER NOT NULL,
        in_house INTEGER NOT NULL DEFAULT 1, rank INTEGER NOT NULL DEFAULT 0, place INTEGER NOT NULL DEFAULT 0,
        owed INTEGER NOT NULL DEFAULT 0, credited INTEGER NOT NULL DEFAULT 0, fixes INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (game_id, user_id));
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);";

/// Columns added after the first release, for a database made before them.
const ADDED_COLUMNS: &[(&str, &str, &str)] =
    &[("rounds", "game_id", "INTEGER NOT NULL DEFAULT 0"), ("rounds", "letter_no", "INTEGER NOT NULL DEFAULT 1")];

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
    conn.execute_batch("CREATE INDEX IF NOT EXISTS rounds_game ON rounds (game_id, letter_no);")
}

pub fn open(workspace: &str) -> anyhow::Result<()> {
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    std::fs::create_dir_all(&dir)?;
    let conn = Connection::open(dir.join("npat.db"))?;
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

// --- games ------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameStatus {
    Running,
    Done,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Game {
    pub id: i64,
    pub channel: u64,
    pub started_at: i64,
    /// Letters the game was set to have.
    pub letters: i64,
    pub status: GameStatus,
    pub pays: bool,
    pub paid_out: bool,
    pub finished_at: Option<i64>,
    pub ended_early: bool,
    pub results_message: Option<u64>,
}

const GAME_COLS: &str = "id, channel_id, started_at, letters, status, pays, paid_out, finished_at, ended_early, results_message_id";

fn game_row(r: &rusqlite::Row) -> rusqlite::Result<Game> {
    Ok(Game {
        id: r.get(0)?,
        channel: r.get::<_, i64>(1)? as u64,
        started_at: r.get(2)?,
        letters: r.get(3)?,
        status: match r.get::<_, String>(4)?.as_str() {
            "running" => GameStatus::Running,
            "done" => GameStatus::Done,
            _ => GameStatus::Stopped,
        },
        pays: r.get::<_, i64>(5)? != 0,
        paid_out: r.get::<_, i64>(6)? != 0,
        finished_at: r.get(7)?,
        ended_early: r.get::<_, i64>(8)? != 0,
        results_message: r.get::<_, Option<i64>>(9)?.map(|m| m as u64),
    })
}

pub fn start_game(conn: &Connection, channel: u64, letters: i64, now: i64) -> rusqlite::Result<Game> {
    conn.execute("INSERT INTO games (channel_id, started_at, letters) VALUES (?1, ?2, ?3)", params![channel as i64, now, letters.max(1)])?;
    get_game(conn, conn.last_insert_rowid()).ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn get_game(conn: &Connection, id: i64) -> Option<Game> {
    conn.query_row(&format!("SELECT {} FROM games WHERE id = ?1", GAME_COLS), params![id], game_row).optional().ok().flatten()
}

/// The game being played, if any (the newest).
pub fn running_game(conn: &Connection) -> Option<Game> {
    conn.query_row(&format!("SELECT {} FROM games WHERE status = 'running' ORDER BY id DESC LIMIT 1", GAME_COLS), [], game_row)
        .optional()
        .ok()
        .flatten()
}

/// Finished games whose points or final card a restart may have left undone.
pub fn unfinished_games(conn: &Connection) -> Vec<Game> {
    let sql = format!(
        "SELECT {} FROM games WHERE status = 'done' AND ((pays = 1 AND paid_out = 0) OR results_message_id IS NULL) ORDER BY id",
        GAME_COLS
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    stmt.query_map([], game_row).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

/// `running` → `done`. False when a mod stopped it meanwhile.
pub fn finish_game(conn: &Connection, id: i64, pays: bool, ended_early: bool, now: i64) -> rusqlite::Result<bool> {
    conn.execute(
        "UPDATE games SET status = 'done', pays = ?2, ended_early = ?3, finished_at = ?4, day = ?5 WHERE id = ?1 AND status = 'running'",
        params![id, pays as i64, ended_early as i64, now, super::points::ist_day(now)],
    )
    .map(|n| n > 0)
}

pub fn mark_game_paid_out(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE games SET paid_out = 1 WHERE id = ?1", params![id]).map(|_| ())
}

pub fn set_game_results_message(conn: &Connection, id: i64, message: u64) -> rusqlite::Result<()> {
    conn.execute("UPDATE games SET results_message_id = ?2 WHERE id = ?1", params![id, message as i64]).map(|_| ())
}

/// A game's letters, in order.
pub fn game_rounds(conn: &Connection, game_id: i64) -> Vec<Round> {
    let sql = format!("SELECT {} FROM rounds WHERE game_id = ?1 ORDER BY letter_no, id", ROUND_COLS);
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    stmt.query_map(params![game_id], round_row).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

/// How many different people answered at least one letter of a game.
pub fn game_players(conn: &Connection, game_id: i64) -> usize {
    conn.query_row(
        "SELECT COUNT(DISTINCT s.user_id) FROM submissions s JOIN rounds r ON r.id = s.round_id WHERE r.game_id = ?1 AND r.status != 'stopped'",
        params![game_id],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0) as usize
}

/// Each player's house key (or [`MUGGLE`]) as of the last letter they answered in a game.
pub fn game_houses(conn: &Connection, game_id: i64) -> HashMap<u64, String> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT s.user_id, s.house FROM submissions s JOIN rounds r ON r.id = s.round_id WHERE r.game_id = ?1 ORDER BY r.letter_no, r.id",
    ) else {
        return HashMap::new();
    };
    stmt.query_map(params![game_id], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, String>(1)?)))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// Each judged letter's scores, in letter order, up to and including `through`
/// (a letter number), for adding a game up.
pub fn letter_scores(conn: &Connection, game_id: i64, through: i64) -> Vec<Vec<LetterScore>> {
    game_rounds(conn, game_id)
        .into_iter()
        .filter(|r| r.status == Status::Done && r.letter_no <= through)
        .map(|round| {
            let sql = "SELECT sc.user_id, sc.score, su.ts, su.house FROM scores sc JOIN submissions su
                       ON su.round_id = sc.round_id AND su.user_id = sc.user_id WHERE sc.round_id = ?1 ORDER BY su.first_ts, sc.user_id";
            let Ok(mut stmt) = conn.prepare(sql) else {
                return Vec::new();
            };
            stmt.query_map(params![round.id], |r| {
                let house: String = r.get(3)?;
                Ok(LetterScore { user: r.get::<_, i64>(0)? as u64, score: r.get(1)?, at: r.get(2)?, in_house: !house.is_empty() && house != MUGGLE && house != MODERATOR })
            })
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
        })
        .collect()
}

/// A game's totals and places, with what each place is owed and was credited.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GameScoreRow {
    pub total: i64,
    pub rank: i64,
    pub place: u8,
    pub owed: i64,
    pub credited: i64,
    pub fixes: i64,
}

/// Writes a game's standings, keeping what was credited and the fix counters.
/// `prizes` is `None` for a game that pays nothing. Anyone no longer in the
/// standings is owed nothing.
pub fn save_game_scores(conn: &mut Connection, game_id: i64, table: &[Standing], prizes: Option<Prizes>) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    tx.execute("UPDATE game_scores SET place = 0, owed = 0, rank = 0 WHERE game_id = ?1", params![game_id])?;
    for s in table {
        let owed = prizes.map(|p| p.for_place(s.place)).unwrap_or(0);
        tx.execute(
            "INSERT INTO game_scores (game_id, user_id, total, reached_at, in_house, rank, place, owed) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(game_id, user_id) DO UPDATE SET total = excluded.total, reached_at = excluded.reached_at,
                 in_house = excluded.in_house, rank = excluded.rank, place = excluded.place, owed = excluded.owed",
            params![game_id, s.user as i64, s.total, s.reached_at, s.in_house as i64, s.rank as i64, s.place as i64, owed],
        )?;
    }
    tx.commit()
}

pub fn game_scores(conn: &Connection, game_id: i64) -> HashMap<u64, GameScoreRow> {
    let Ok(mut stmt) = conn.prepare("SELECT user_id, total, rank, place, owed, credited, fixes FROM game_scores WHERE game_id = ?1") else {
        return HashMap::new();
    };
    stmt.query_map(params![game_id], |r| {
        Ok((
            r.get::<_, i64>(0)? as u64,
            GameScoreRow { total: r.get(1)?, rank: r.get(2)?, place: r.get::<_, i64>(3)? as u8, owed: r.get(4)?, credited: r.get(5)?, fixes: r.get(6)? },
        ))
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

pub fn add_game_credit(conn: &Connection, game_id: i64, user: u64, granted: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE game_scores SET credited = credited + ?3 WHERE game_id = ?1 AND user_id = ?2",
        params![game_id, user as i64, granted],
    )
    .map(|_| ())
}

/// The next fix number for someone's game: 1, 2, 3…
pub fn next_game_fix(conn: &Connection, game_id: i64, user: u64) -> rusqlite::Result<i64> {
    conn.execute("UPDATE game_scores SET fixes = fixes + 1 WHERE game_id = ?1 AND user_id = ?2", params![game_id, user as i64])?;
    conn.query_row("SELECT fixes FROM game_scores WHERE game_id = ?1 AND user_id = ?2", params![game_id, user as i64], |r| r.get(0))
}

// --- rounds -----------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Open,
    Judging,
    Done,
    Stopped,
}

impl Status {
    fn from_key(key: &str) -> Status {
        match key {
            "open" => Status::Open,
            "judging" => Status::Judging,
            "done" => Status::Done,
            _ => Status::Stopped,
        }
    }
}

/// One letter of a game.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Round {
    pub id: i64,
    pub game_id: i64,
    /// 1 for the game's first letter.
    pub letter_no: i64,
    pub channel: u64,
    pub message: Option<u64>,
    pub results_message: Option<u64>,
    pub letter: char,
    pub started_at: i64,
    pub ends_at: i64,
    pub status: Status,
    pub letter_only: bool,
    pub judged_at: Option<i64>,
}

const ROUND_COLS: &str = "id, game_id, letter_no, channel_id, message_id, results_message_id, letter, started_at, ends_at, status, letter_only, judged_at";

fn round_row(r: &rusqlite::Row) -> rusqlite::Result<Round> {
    Ok(Round {
        id: r.get(0)?,
        game_id: r.get(1)?,
        letter_no: r.get(2)?,
        channel: r.get::<_, i64>(3)? as u64,
        message: r.get::<_, Option<i64>>(4)?.map(|m| m as u64),
        results_message: r.get::<_, Option<i64>>(5)?.map(|m| m as u64),
        letter: r.get::<_, String>(6)?.chars().next().unwrap_or('A'),
        started_at: r.get(7)?,
        ends_at: r.get(8)?,
        status: Status::from_key(&r.get::<_, String>(9)?),
        letter_only: r.get::<_, i64>(10)? != 0,
        judged_at: r.get(11)?,
    })
}

pub fn get_round(conn: &Connection, id: i64) -> Option<Round> {
    conn.query_row(&format!("SELECT {} FROM rounds WHERE id = ?1", ROUND_COLS), params![id], round_row).optional().ok().flatten()
}

/// The round still being played or judged, if any (the newest).
pub fn live_round(conn: &Connection) -> Option<Round> {
    conn.query_row(
        &format!("SELECT {} FROM rounds WHERE status IN ('open', 'judging') ORDER BY id DESC LIMIT 1", ROUND_COLS),
        [],
        round_row,
    )
    .optional()
    .ok()
    .flatten()
}

/// The letters of the last `n` rounds, newest first.
pub fn recent_letters(conn: &Connection, n: usize) -> Vec<char> {
    let Ok(mut stmt) = conn.prepare("SELECT letter FROM rounds ORDER BY id DESC LIMIT ?1") else {
        return Vec::new();
    };
    stmt.query_map(params![n as i64], |r| r.get::<_, String>(0))
        .map(|rows| rows.flatten().filter_map(|l| l.chars().next()).collect())
        .unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
pub fn start_round(conn: &Connection, game_id: i64, letter_no: i64, channel: u64, letter: char, now: i64, secs: i64) -> rusqlite::Result<Round> {
    conn.execute(
        "INSERT INTO rounds (game_id, letter_no, channel_id, letter, started_at, ends_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![game_id, letter_no, channel as i64, letter.to_string(), now, now + secs],
    )?;
    get_round(conn, conn.last_insert_rowid()).ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn set_message(conn: &Connection, id: i64, message: u64) -> rusqlite::Result<()> {
    conn.execute("UPDATE rounds SET message_id = ?2 WHERE id = ?1", params![id, message as i64]).map(|_| ())
}

pub fn set_results_message(conn: &Connection, id: i64, message: u64) -> rusqlite::Result<()> {
    conn.execute("UPDATE rounds SET results_message_id = ?2 WHERE id = ?1", params![id, message as i64]).map(|_| ())
}

/// `open` → `judging`. A round already judging (a restart mid-judging) stays so
/// and is judged again. False for a stopped or finished round.
pub fn begin_judging(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    conn.execute("UPDATE rounds SET status = 'judging' WHERE id = ?1 AND status IN ('open', 'judging')", params![id]).map(|n| n > 0)
}

/// `judging` → `done`, with how it was judged. False when a mod stopped it meanwhile.
pub fn finish_judging(conn: &Connection, id: i64, letter_only: bool, now: i64) -> rusqlite::Result<bool> {
    conn.execute(
        "UPDATE rounds SET status = 'done', letter_only = ?2, judged_at = ?3, day = ?4 WHERE id = ?1 AND status = 'judging'",
        params![id, letter_only as i64, now, super::points::ist_day(now)],
    )
    .map(|n| n > 0)
}

/// Stops the live round and the running game, if there are any: no judging,
/// no points.
pub fn stop_live(conn: &Connection, by: u64) -> rusqlite::Result<(Option<Round>, Option<Game>)> {
    let round = match live_round(conn) {
        Some(round) => {
            let n = conn.execute(
                "UPDATE rounds SET status = 'stopped', stopped_by = ?2 WHERE id = ?1 AND status IN ('open', 'judging')",
                params![round.id, by as i64],
            )?;
            (n > 0).then(|| Round { status: Status::Stopped, ..round })
        }
        None => None,
    };
    let game = match running_game(conn) {
        Some(game) => {
            let n = conn.execute("UPDATE games SET status = 'stopped', stopped_by = ?2 WHERE id = ?1 AND status = 'running'", params![game.id, by as i64])?;
            (n > 0).then(|| Game { status: GameStatus::Stopped, ..game })
        }
        None => None,
    };
    Ok((round, game))
}

pub fn status_of(conn: &Connection, id: i64) -> Option<Status> {
    conn.query_row("SELECT status FROM rounds WHERE id = ?1", params![id], |r| r.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
        .map(|s| Status::from_key(&s))
}

// --- submissions ------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
pub enum Submit {
    Saved,
    /// Every box blank: any earlier submission was taken back.
    Cleared,
    Closed,
    Missing,
}

/// Saves someone's answers, replacing earlier ones, while the round is open
/// (the timer plus [`GRACE_SECS`]). Blank answers are kept as empty strings.
/// `house` is their house's key, or [`MUGGLE`].
pub fn submit(conn: &Connection, round_id: i64, user: u64, house: &str, answers: &[String; 4], now: i64) -> rusqlite::Result<Submit> {
    let Some(round) = get_round(conn, round_id) else {
        return Ok(Submit::Missing);
    };
    if round.status != Status::Open || now > round.ends_at + GRACE_SECS {
        return Ok(Submit::Closed);
    }
    if answers.iter().all(|a| a.trim().is_empty()) {
        conn.execute("DELETE FROM submissions WHERE round_id = ?1 AND user_id = ?2", params![round_id, user as i64])?;
        return Ok(Submit::Cleared);
    }
    conn.execute(
        "INSERT INTO submissions (round_id, user_id, name, place, animal, thing, ts, first_ts, house) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8)
         ON CONFLICT(round_id, user_id) DO UPDATE SET name = excluded.name, place = excluded.place,
             animal = excluded.animal, thing = excluded.thing, ts = excluded.ts, house = excluded.house",
        params![round_id, user as i64, answers[0], answers[1], answers[2], answers[3], now, house],
    )?;
    Ok(Submit::Saved)
}

pub fn submission(conn: &Connection, round_id: i64, user: u64) -> Option<[String; 4]> {
    conn.query_row(
        "SELECT name, place, animal, thing FROM submissions WHERE round_id = ?1 AND user_id = ?2",
        params![round_id, user as i64],
        |r| Ok([r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?]),
    )
    .optional()
    .ok()
    .flatten()
}

/// Everyone's answers, in the order they first answered.
pub fn submissions(conn: &Connection, round_id: i64) -> Vec<(u64, [String; 4])> {
    let Ok(mut stmt) =
        conn.prepare("SELECT user_id, name, place, animal, thing FROM submissions WHERE round_id = ?1 ORDER BY first_ts, user_id")
    else {
        return Vec::new();
    };
    stmt.query_map(params![round_id], |r| Ok((r.get::<_, i64>(0)? as u64, [r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?])))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

pub fn submission_count(conn: &Connection, round_id: i64) -> usize {
    conn.query_row("SELECT COUNT(*) FROM submissions WHERE round_id = ?1", params![round_id], |r| r.get::<_, i64>(0))
        .unwrap_or(0) as usize
}

// --- verdicts and scores ---------------------------------------------------------------

/// Stores the verdicts of a round, replacing any from an earlier attempt.
/// `rows` is (user, category, answer, verdict).
pub fn save_verdicts(conn: &mut Connection, round_id: i64, rows: &[(u64, usize, String, Verdict)]) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM round_verdicts WHERE round_id = ?1", params![round_id])?;
    for (user, cat, answer, v) in rows {
        tx.execute(
            "INSERT INTO round_verdicts (round_id, user_id, category, answer, valid, canonical) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![round_id, *user as i64, *cat as i64, answer, v.valid as i64, v.canonical],
        )?;
    }
    tx.commit()
}

/// Each player's house key (or [`MUGGLE`]) as it was when they answered.
pub fn houses(conn: &Connection, round_id: i64) -> HashMap<u64, String> {
    let Ok(mut stmt) = conn.prepare("SELECT user_id, house FROM submissions WHERE round_id = ?1") else {
        return HashMap::new();
    };
    stmt.query_map(params![round_id], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, String>(1)?)))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// Every player of a round with their answers and verdicts, in answering order.
pub fn judged(conn: &Connection, round_id: i64) -> Vec<Judged> {
    let mut out: Vec<Judged> = match conn.prepare("SELECT user_id, ts, house FROM submissions WHERE round_id = ?1 ORDER BY first_ts, user_id") {
        Ok(mut stmt) => stmt
            .query_map(params![round_id], |r| {
                let house: String = r.get(2)?;
                Ok(Judged { user: r.get::<_, i64>(0)? as u64, at: r.get(1)?, in_house: !house.is_empty() && house != MUGGLE && house != MODERATOR, answers: [None, None, None, None] })
            })
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default(),
        Err(_) => return Vec::new(),
    };
    let Ok(mut stmt) = conn.prepare("SELECT user_id, category, answer, valid, canonical FROM round_verdicts WHERE round_id = ?1") else {
        return out;
    };
    let rows = stmt
        .query_map(params![round_id], |r| {
            Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as usize, r.get::<_, String>(2)?, r.get::<_, i64>(3)? != 0, r.get::<_, String>(4)?))
        })
        .map(|rows| rows.flatten().collect::<Vec<_>>())
        .unwrap_or_default();
    for (user, cat, answer, valid, canonical) in rows {
        if let (Some(entry), true) = (out.iter_mut().find(|e| e.user == user), cat < 4) {
            entry.answers[cat] = Some((answer, Verdict { valid, canonical }));
        }
    }
    out
}

/// A letter's score for someone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScoreRow {
    pub score: i64,
    pub unique: i64,
}

/// Writes a letter's scores, replacing earlier ones.
pub fn save_scores(conn: &mut Connection, round_id: i64, scored: &[Scored]) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    for s in scored {
        tx.execute(
            "INSERT INTO scores (round_id, user_id, score, unique_count) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(round_id, user_id) DO UPDATE SET score = excluded.score, unique_count = excluded.unique_count",
            params![round_id, s.user as i64, s.score, s.unique],
        )?;
    }
    tx.commit()
}

pub fn scores(conn: &Connection, round_id: i64) -> HashMap<u64, ScoreRow> {
    let Ok(mut stmt) = conn.prepare("SELECT user_id, score, unique_count FROM scores WHERE round_id = ?1") else {
        return HashMap::new();
    };
    stmt.query_map(params![round_id], |r| Ok((r.get::<_, i64>(0)? as u64, ScoreRow { score: r.get(1)?, unique: r.get(2)? })))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// A mod's review: flips one answer between valid and not. Returns the new
/// verdict, or `None` when there is no such answer.
pub fn toggle_verdict(conn: &Connection, round_id: i64, user: u64, cat: usize, by: u64, now: i64) -> rusqlite::Result<Option<Verdict>> {
    let found: Option<(String, bool, String)> = conn
        .query_row(
            "SELECT answer, valid, canonical FROM round_verdicts WHERE round_id = ?1 AND user_id = ?2 AND category = ?3",
            params![round_id, user as i64, cat as i64],
            |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0, r.get(2)?)),
        )
        .optional()?;
    let Some((answer, valid, canonical)) = found else {
        return Ok(None);
    };
    let next = super::npat_judge::toggled(&Verdict { valid, canonical }, &answer);
    if let Some(round) = get_round(conn, round_id) {
        cache_put(conn, round.letter, cat, &super::npat_judge::fold_key(&answer), &next, "mod", now)?;
    }
    conn.execute(
        "UPDATE round_verdicts SET valid = ?4, canonical = ?5, reviewed_by = ?6, reviewed_ts = ?7 WHERE round_id = ?1 AND user_id = ?2 AND category = ?3",
        params![round_id, user as i64, cat as i64, next.valid as i64, next.canonical, by as i64, now],
    )?;
    conn.execute(
        "UPDATE challenges SET resolved_by = ?4 WHERE round_id = ?1 AND user_id = ?2 AND category = ?3",
        params![round_id, user as i64, cat as i64, by as i64],
    )?;
    Ok(Some(next))
}

// --- the verdict cache ---------------------------------------------------------------------

/// Remembers a verdict. `source` is "ai" or "mod"; a model verdict never
/// replaces a mod's.
pub fn cache_put(conn: &Connection, letter: char, cat: usize, folded: &str, v: &Verdict, source: &str, now: i64) -> rusqlite::Result<()> {
    if folded.is_empty() {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO verdicts (letter, category, folded_answer, valid, canonical, source, ts) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(letter, category, folded_answer) DO UPDATE SET valid = excluded.valid, canonical = excluded.canonical,
             source = excluded.source, ts = excluded.ts
         WHERE excluded.source = 'mod' OR verdicts.source != 'mod'",
        params![letter.to_ascii_uppercase().to_string(), cat as i64, folded, v.valid as i64, v.canonical, source, now],
    )
    .map(|_| ())
}

/// A remembered verdict and where it came from.
pub fn cache_get(conn: &Connection, letter: char, cat: usize, folded: &str) -> Option<(Verdict, String)> {
    conn.query_row(
        "SELECT valid, canonical, source FROM verdicts WHERE letter = ?1 AND category = ?2 AND folded_answer = ?3",
        params![letter.to_ascii_uppercase().to_string(), cat as i64, folded],
        |r| Ok((Verdict { valid: r.get::<_, i64>(0)? != 0, canonical: r.get(1)? }, r.get::<_, String>(2)?)),
    )
    .optional()
    .ok()
    .flatten()
}

/// The remembered verdicts for a round's items, by item id.
pub fn cached_for(conn: &Connection, letter: char, items: &[super::npat_judge::Item]) -> HashMap<usize, Verdict> {
    items
        .iter()
        .filter_map(|i| {
            let key = super::npat_judge::fold_key(&i.text);
            (!key.is_empty()).then(|| cache_get(conn, letter, i.category, &key)).flatten().map(|(v, _)| (i.id, v))
        })
        .collect()
}

// --- challenges -------------------------------------------------------------------------

pub fn add_challenges(conn: &Connection, round_id: i64, user: u64, cats: &[usize], now: i64) -> rusqlite::Result<usize> {
    let mut added = 0;
    for cat in cats.iter().filter(|c| **c < 4) {
        added += conn.execute(
            "INSERT OR IGNORE INTO challenges (round_id, user_id, category, ts) VALUES (?1, ?2, ?3, ?4)",
            params![round_id, user as i64, *cat as i64, now],
        )?;
    }
    Ok(added)
}

/// A round's challenges: (user, category, resolved), oldest first.
pub fn challenges(conn: &Connection, round_id: i64) -> Vec<(u64, usize, bool)> {
    let Ok(mut stmt) = conn.prepare("SELECT user_id, category, resolved_by IS NOT NULL FROM challenges WHERE round_id = ?1 ORDER BY ts") else {
        return Vec::new();
    };
    stmt.query_map(params![round_id], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as usize, r.get::<_, bool>(2)?)))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

// --- the day's tops ---------------------------------------------------------------------

/// Every 1st and 2nd place of a game on an India day, over games that paid:
/// (user, place, when the game finished), oldest first.
pub fn places_on(conn: &Connection, day: &str) -> Vec<(u64, u8, i64)> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT s.user_id, s.place, g.finished_at FROM game_scores s JOIN games g ON g.id = s.game_id
         WHERE g.day = ?1 AND g.status = 'done' AND g.pays = 1 AND s.place IN (1, 2) ORDER BY g.finished_at, g.id, s.place",
    ) else {
        return Vec::new();
    };
    stmt.query_map(params![day], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u8, r.get::<_, Option<i64>>(2)?.unwrap_or(0))))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::npat_judge::{self as judge, Mark, Points, Prizes};
    use super::*;

    pub fn memory() -> Connection {
        let conn = Connection::open_in_memory().expect("memory db");
        init(&conn).expect("schema");
        conn
    }

    fn answers(a: [&str; 4]) -> [String; 4] {
        a.map(String::from)
    }

    /// 2026-09-14 12:00 India time.
    const NOON: i64 = 1_789_367_400;

    #[test]
    fn a_round_takes_the_last_answer_in_time_and_moves_through_its_states() {
        let mut conn = memory();
        let round = start_round(&conn, 1, 1, 7, 'P', NOON, 45).unwrap();
        assert_eq!((round.letter, round.ends_at, round.status), ('P', NOON + 45, Status::Open));
        assert_eq!(live_round(&conn).map(|r| r.id), Some(round.id));
        assert_eq!(submit(&conn, round.id, 1, "gryffindor", &answers(["Priya", "", "", ""]), NOON + 5).unwrap(), Submit::Saved);
        assert_eq!(submit(&conn, round.id, 2, "gryffindor", &answers(["Pooja", "Pune", "", ""]), NOON + 6).unwrap(), Submit::Saved);
        assert_eq!(submit(&conn, round.id, 1, "gryffindor", &answers(["Priya", "Patna", "Parrot", "Pen"]), NOON + 47).unwrap(), Submit::Saved, "inside the grace");
        assert_eq!(submit(&conn, round.id, 1, "gryffindor", &answers(["X", "", "", ""]), NOON + 49).unwrap(), Submit::Closed);
        assert_eq!(submission(&conn, round.id, 1), Some(answers(["Priya", "Patna", "Parrot", "Pen"])));
        let order: Vec<u64> = submissions(&conn, round.id).iter().map(|(u, _)| *u).collect();
        assert_eq!(order, vec![1, 2], "a later change keeps the first answering time");
        assert_eq!(submit(&conn, round.id, 3, "gryffindor", &answers(["Q", "", "", ""]), NOON + 10).unwrap(), Submit::Saved);
        assert_eq!(submit(&conn, round.id, 3, "gryffindor", &answers(["", " ", "", ""]), NOON + 11).unwrap(), Submit::Cleared);
        assert_eq!(submission_count(&conn, round.id), 2);
        submit(&conn, round.id, 5, MUGGLE, &answers(["Pia", "", "", ""]), NOON + 12).unwrap();
        assert_eq!(houses(&conn, round.id).get(&5).map(String::as_str), Some(MUGGLE));
        assert!(!judged(&conn, round.id).iter().find(|e| e.user == 5).unwrap().in_house, "a Muggle plays but can't place");
        submit(&conn, round.id, 5, MUGGLE, &answers(["", "", "", ""]), NOON + 13).unwrap();
        assert_eq!(submit(&conn, 999, 1, "gryffindor", &answers(["a", "", "", ""]), NOON).unwrap(), Submit::Missing);

        assert!(begin_judging(&conn, round.id).unwrap());
        assert!(begin_judging(&conn, round.id).unwrap(), "a restart judges again");
        assert_eq!(submit(&conn, round.id, 4, "gryffindor", &answers(["Pia", "", "", ""]), NOON + 20).unwrap(), Submit::Closed);
        let rows = vec![(1, 0, "Priya".to_string(), judge::Verdict { valid: true, canonical: "Priya".into() })];
        save_verdicts(&mut conn, round.id, &rows).unwrap();
        save_verdicts(&mut conn, round.id, &rows).unwrap();
        let j = judged(&conn, round.id);
        assert_eq!(j.len(), 2);
        assert_eq!(j[0].answers[0].as_ref().map(|(a, v)| (a.as_str(), v.valid)), Some(("Priya", true)));
        assert_eq!((j[0].user, j[0].at, j[0].in_house), (1, NOON + 47, true), "the final answers' time");
        assert!(j[1].answers.iter().all(Option::is_none));
        assert!(finish_judging(&conn, round.id, false, NOON + 60).unwrap());
        let done = get_round(&conn, round.id).unwrap();
        assert_eq!((done.status, done.judged_at, done.game_id, done.letter_no), (Status::Done, Some(NOON + 60), 1, 1));
        assert!(live_round(&conn).is_none());
        assert_eq!(recent_letters(&conn, 5), vec!['P']);
    }

    #[test]
    fn the_cache_remembers_model_verdicts_and_a_mod_always_wins() {
        let conn = memory();
        let v = |valid: bool, c: &str| judge::Verdict { valid, canonical: c.into() };
        cache_put(&conn, 'p', 2, "pegasus", &v(false, "Pegasus"), "ai", NOON).unwrap();
        assert_eq!(cache_get(&conn, 'P', 2, "pegasus"), Some((v(false, "Pegasus"), "ai".to_string())));
        assert_eq!(cache_get(&conn, 'P', 1, "pegasus"), None, "per category");
        assert_eq!(cache_get(&conn, 'Q', 2, "pegasus"), None, "per letter");
        cache_put(&conn, 'P', 2, "pegasus", &v(true, "Pegasus"), "mod", NOON + 1).unwrap();
        cache_put(&conn, 'P', 2, "pegasus", &v(false, "no"), "ai", NOON + 2).unwrap();
        assert_eq!(cache_get(&conn, 'P', 2, "pegasus"), Some((v(true, "Pegasus"), "mod".to_string())), "the model can't overwrite a mod");
        cache_put(&conn, 'P', 2, "pegasus", &v(false, "Pegasus"), "mod", NOON + 3).unwrap();
        assert_eq!(cache_get(&conn, 'P', 2, "pegasus").unwrap().0.valid, false, "a later mod review can");
        cache_put(&conn, 'P', 2, "", &v(true, "x"), "ai", NOON).unwrap();
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM verdicts", [], |r| r.get::<_, i64>(0)).unwrap(), 1, "nothing stored for a blank key");

        let items = vec![
            judge::Item { id: 0, category: 2, text: "The Pegasus".into() },
            judge::Item { id: 1, category: 3, text: "Pen".into() },
        ];
        let cached = cached_for(&conn, 'P', &items);
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[&0], v(false, "Pegasus"));

        // A review writes the cache as a mod verdict for that letter.
        let round = start_round(&conn, 1, 1, 7, 'P', NOON, 45).unwrap();
        submit(&conn, round.id, 1, "gryffindor", &answers(["", "", "", "Pen"]), NOON + 1).unwrap();
        conn.execute("INSERT INTO round_verdicts (round_id, user_id, category, answer, valid, canonical) VALUES (?1, 1, 3, 'Pen', 0, '')", params![round.id]).unwrap();
        cache_put(&conn, 'P', 3, "pen", &v(false, ""), "ai", NOON).unwrap();
        toggle_verdict(&conn, round.id, 1, 3, 42, NOON + 5).unwrap();
        assert_eq!(cache_get(&conn, 'P', 3, "pen"), Some((v(true, "Pen"), "mod".to_string())));
    }

    #[test]
    fn a_stop_beats_judging() {
        let conn = memory();
        let round = start_round(&conn, 1, 1, 7, 'M', NOON, 45).unwrap();
        assert!(begin_judging(&conn, round.id).unwrap());
        let game = start_game(&conn, 7, 5, NOON).unwrap();
        let (round_stopped, game_stopped) = stop_live(&conn, 9).unwrap();
        assert_eq!(round_stopped.map(|r| r.status), Some(Status::Stopped));
        assert_eq!(game_stopped.map(|g| (g.id, g.status)), Some((game.id, GameStatus::Stopped)));
        assert!(!finish_judging(&conn, round.id, false, NOON + 60).unwrap(), "stopped: nothing to finish");
        assert!(!finish_game(&conn, game.id, true, false, NOON + 60).unwrap(), "a stopped game can't finish");
        assert_eq!(status_of(&conn, round.id), Some(Status::Stopped));
        assert_eq!(stop_live(&conn, 9).unwrap(), (None, None));
        assert!(!begin_judging(&conn, round.id).unwrap());
        let next = start_round(&conn, 1, 2, 7, 'K', NOON + 100, 45).unwrap();
        assert_eq!(recent_letters(&conn, 5), vec!['K', 'M']);
        assert_eq!(live_round(&conn).map(|r| r.id), Some(next.id));
    }

    #[test]
    fn a_game_adds_up_its_letters_and_a_review_can_move_its_places() {
        let mut conn = memory();
        let p = Points { unique: 10, shared: 5 };
        let prizes = Some(Prizes { first: 2, second: 1 });
        let v = |valid: bool, c: &str| judge::Verdict { valid, canonical: c.into() };
        let game = start_game(&conn, 7, 5, NOON).unwrap();
        assert_eq!((running_game(&conn).map(|g| g.id), game.letters, game.status), (Some(game.id), 5, GameStatus::Running));

        // Letter 1: 1 and 2 answer.
        let r1 = start_round(&conn, game.id, 1, 7, 'P', NOON, 45).unwrap();
        submit(&conn, r1.id, 1, "gryffindor", &answers(["Priya", "Pondicherry", "", ""]), NOON + 1).unwrap();
        submit(&conn, r1.id, 2, "slytherin", &answers(["", "Puducherry", "Panda", ""]), NOON + 2).unwrap();
        begin_judging(&conn, r1.id).unwrap();
        save_verdicts(
            &mut conn,
            r1.id,
            &[(1, 0, "Priya".into(), v(true, "Priya")), (1, 1, "Pondicherry".into(), v(false, "")), (2, 1, "Puducherry".into(), v(true, "Puducherry")), (2, 2, "Panda".into(), v(true, "Panda"))],
        )
        .unwrap();
        let s1 = judge::score(&judged(&conn, r1.id), p);
        save_scores(&mut conn, r1.id, &s1).unwrap();
        finish_judging(&conn, r1.id, false, NOON + 50).unwrap();
        // Letter 2: 3 joins late; 1 answers again.
        let r2 = start_round(&conn, game.id, 2, 7, 'M', NOON + 100, 45).unwrap();
        submit(&conn, r2.id, 3, MUGGLE, &answers(["Meera", "", "", ""]), NOON + 101).unwrap();
        submit(&conn, r2.id, 1, "gryffindor", &answers(["", "", "", "Mug"]), NOON + 102).unwrap();
        begin_judging(&conn, r2.id).unwrap();
        save_verdicts(&mut conn, r2.id, &[(3, 0, "Meera".into(), v(true, "Meera")), (1, 3, "Mug".into(), v(true, "Mug"))]).unwrap();
        let s2 = judge::score(&judged(&conn, r2.id), p);
        save_scores(&mut conn, r2.id, &s2).unwrap();
        finish_judging(&conn, r2.id, false, NOON + 150).unwrap();
        assert_eq!(game_players(&conn, game.id), 3);
        assert_eq!(game_houses(&conn, game.id).get(&3).map(String::as_str), Some(MUGGLE));
        assert_eq!(game_houses(&conn, game.id).get(&2).map(String::as_str), Some("slytherin"));
        assert_eq!(game_rounds(&conn, game.id).iter().map(|r| r.letter).collect::<String>(), "PM");
        assert_eq!(letter_scores(&conn, game.id, 1).len(), 1, "only up to the letter asked for");

        let table = judge::standings(&letter_scores(&conn, game.id, 5));
        let rows: Vec<(u64, i64, u8)> = table.iter().map(|s| (s.user, s.total, s.place)).collect();
        assert_eq!(rows, vec![(2, 20, 1), (1, 20, 2), (3, 10, 0)], "level on 20: 2 locked their total in during letter 1, 1 only in letter 2; the Muggle can't place");
        assert!(finish_game(&conn, game.id, true, false, NOON + 160).unwrap());
        save_game_scores(&mut conn, game.id, &table, prizes).unwrap();
        add_game_credit(&conn, game.id, 2, 2).unwrap();
        add_game_credit(&conn, game.id, 1, 1).unwrap();
        let before = game_scores(&conn, game.id);
        assert_eq!(unfinished_games(&conn).len(), 1);
        mark_game_paid_out(&conn, game.id).unwrap();
        set_game_results_message(&conn, game.id, 77).unwrap();
        assert!(unfinished_games(&conn).is_empty());
        assert!(running_game(&conn).is_none());

        // A mod makes Pondicherry valid, and the same place as Puducherry.
        add_challenges(&conn, r1.id, 1, &[1, 1, 9], NOON + 170).unwrap();
        assert_eq!(challenges(&conn, r1.id), vec![(1, 1, false)]);
        assert_eq!(toggle_verdict(&conn, r1.id, 1, 1, 42, NOON + 180).unwrap(), Some(v(true, "Pondicherry")));
        conn.execute("UPDATE round_verdicts SET canonical = 'Puducherry' WHERE round_id = ?1 AND user_id = 1 AND category = 1", params![r1.id]).unwrap();
        assert_eq!(challenges(&conn, r1.id), vec![(1, 1, true)], "the review resolves the challenge");
        let s1 = judge::score(&judged(&conn, r1.id), p);
        save_scores(&mut conn, r1.id, &s1).unwrap();
        assert_eq!(scores(&conn, r1.id)[&2].score, 15, "2's place is now shared");
        let table = judge::standings(&letter_scores(&conn, game.id, 5));
        save_game_scores(&mut conn, game.id, &table, prizes).unwrap();
        let after = game_scores(&conn, game.id);
        assert_eq!((after[&1].total, after[&1].place, after[&1].owed, after[&1].credited), (25, 1, 2, 1), "credited is kept");
        assert_eq!((after[&2].total, after[&2].place, after[&2].owed), (15, 2, 1));
        assert_eq!(judge::adjustment(before[&1].owed, after[&1].owed, before[&1].credited), 1, "2nd became 1st");
        assert_eq!(judge::adjustment(before[&2].owed, after[&2].owed, before[&2].credited), -1, "1st became 2nd");
        assert_eq!(next_game_fix(&conn, game.id, 1).unwrap(), 1);
        assert_eq!(next_game_fix(&conn, game.id, 1).unwrap(), 2);
        assert!(toggle_verdict(&conn, r1.id, 1, 3, 42, NOON).unwrap().is_none(), "a blank has nothing to flip");

        let places = places_on(&conn, "2026-09-14");
        assert_eq!(places, vec![(1, 1, NOON + 160), (2, 2, NOON + 160)], "the places as they stand after the review");
        conn.execute("UPDATE games SET pays = 0", []).unwrap();
        assert!(places_on(&conn, "2026-09-14").is_empty(), "games that didn't pay don't count");
    }

    #[test]
    fn an_old_database_gets_the_new_columns() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE rounds (id INTEGER PRIMARY KEY AUTOINCREMENT, channel_id INTEGER NOT NULL, message_id INTEGER,
             results_message_id INTEGER, letter TEXT NOT NULL, started_at INTEGER NOT NULL, ends_at INTEGER NOT NULL,
             status TEXT NOT NULL DEFAULT 'open', letter_only INTEGER NOT NULL DEFAULT 0, pays INTEGER NOT NULL DEFAULT 0,
             paid_out INTEGER NOT NULL DEFAULT 0, judged_at INTEGER, day TEXT NOT NULL DEFAULT '', stopped_by INTEGER);
             INSERT INTO rounds (channel_id, letter, started_at, ends_at, status) VALUES (7, 'P', 1, 2, 'done');",
        )
        .unwrap();
        init(&conn).unwrap();
        init(&conn).unwrap();
        let old = get_round(&conn, 1).unwrap();
        assert_eq!((old.game_id, old.letter_no, old.letter), (0, 1, 'P'));
    }
}
