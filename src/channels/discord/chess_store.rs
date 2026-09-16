//! What chess keeps in `.runtime/chess.db`: open challenges, the games
//! themselves, the private board links, and the note of which pair has already
//! been paid today.
//!
//! A game's move list is the record; the FEN beside it is kept in step on every
//! move so a card can be drawn without replaying. Every move that changes a
//! game is one `UPDATE … WHERE status = 'running' AND moves = ?`, so two
//! presses landing together can never both go through - the second sees the
//! move list it expected is no longer there and does nothing. Finishing a game
//! works the same way, which is what stops a resignation and a checkmate paying
//! twice.
//!
//! The board links are random strings, one per player per game, and they are
//! deleted the moment the game ends: a link is only ever good for the game and
//! the side it was made for.

use std::sync::OnceLock;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

pub const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS challenges (
        id INTEGER PRIMARY KEY AUTOINCREMENT, channel_id INTEGER NOT NULL, message_id INTEGER,
        challenger INTEGER NOT NULL, opponent INTEGER NOT NULL, time_control TEXT NOT NULL,
        created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL,
        status TEXT NOT NULL DEFAULT 'open', game_id INTEGER);
    CREATE INDEX IF NOT EXISTS challenges_status ON challenges (status);
    CREATE TABLE IF NOT EXISTS games (
        id INTEGER PRIMARY KEY AUTOINCREMENT, channel_id INTEGER NOT NULL,
        white INTEGER NOT NULL, black INTEGER NOT NULL,
        white_house TEXT NOT NULL DEFAULT '', black_house TEXT NOT NULL DEFAULT '',
        fen TEXT NOT NULL, moves TEXT NOT NULL DEFAULT '',
        status TEXT NOT NULL DEFAULT 'running', time_control TEXT NOT NULL,
        per_move_secs INTEGER NOT NULL, started_at INTEGER NOT NULL, last_move_ts INTEGER NOT NULL,
        message_id INTEGER, result TEXT, winner INTEGER, by_resignation INTEGER NOT NULL DEFAULT 0,
        finished_at INTEGER, points_white INTEGER NOT NULL DEFAULT 0, points_black INTEGER NOT NULL DEFAULT 0,
        paid_out INTEGER NOT NULL DEFAULT 0, why_nothing TEXT NOT NULL DEFAULT '',
        draw_offer INTEGER, draw_announced INTEGER NOT NULL DEFAULT 0,
        nudged_white INTEGER NOT NULL DEFAULT 0, nudged_black INTEGER NOT NULL DEFAULT 0,
        day TEXT NOT NULL DEFAULT '');
    CREATE INDEX IF NOT EXISTS games_status ON games (status);
    CREATE INDEX IF NOT EXISTS games_players ON games (white, black);
    CREATE TABLE IF NOT EXISTS tokens (
        token TEXT PRIMARY KEY, game_id INTEGER NOT NULL, user_id INTEGER NOT NULL, made_at INTEGER NOT NULL);
    CREATE INDEX IF NOT EXISTS tokens_game ON tokens (game_id);
    CREATE TABLE IF NOT EXISTS pair_days (
        day TEXT NOT NULL, low INTEGER NOT NULL, high INTEGER NOT NULL, game_id INTEGER NOT NULL,
        PRIMARY KEY (day, low, high));
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);";

pub fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)
}

pub fn open(workspace: &str) -> anyhow::Result<()> {
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    std::fs::create_dir_all(&dir)?;
    let conn = Connection::open(dir.join("chess.db"))?;
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

// --- challenges -----------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChallengeStatus {
    Open,
    Accepted,
    Declined,
    Expired,
}

impl ChallengeStatus {
    fn key(self) -> &'static str {
        match self {
            ChallengeStatus::Open => "open",
            ChallengeStatus::Accepted => "accepted",
            ChallengeStatus::Declined => "declined",
            ChallengeStatus::Expired => "expired",
        }
    }

    fn from_key(key: &str) -> ChallengeStatus {
        match key {
            "accepted" => ChallengeStatus::Accepted,
            "declined" => ChallengeStatus::Declined,
            "expired" => ChallengeStatus::Expired,
            _ => ChallengeStatus::Open,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Challenge {
    pub id: i64,
    pub channel: u64,
    pub message: Option<u64>,
    pub challenger: u64,
    pub opponent: u64,
    pub time_control: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub status: ChallengeStatus,
    pub game_id: Option<i64>,
}

fn challenge_from(row: &rusqlite::Row) -> rusqlite::Result<Challenge> {
    Ok(Challenge {
        id: row.get(0)?,
        channel: row.get::<_, i64>(1)? as u64,
        message: row.get::<_, Option<i64>>(2)?.map(|m| m as u64),
        challenger: row.get::<_, i64>(3)? as u64,
        opponent: row.get::<_, i64>(4)? as u64,
        time_control: row.get(5)?,
        created_at: row.get(6)?,
        expires_at: row.get(7)?,
        status: ChallengeStatus::from_key(&row.get::<_, String>(8)?),
        game_id: row.get(9)?,
    })
}

const CHALLENGE_COLUMNS: &str =
    "id, channel_id, message_id, challenger, opponent, time_control, created_at, expires_at, status, game_id";

pub fn create_challenge(
    conn: &Connection,
    channel: u64,
    challenger: u64,
    opponent: u64,
    time_control: &str,
    now: i64,
    expires_at: i64,
) -> rusqlite::Result<Challenge> {
    conn.execute(
        "INSERT INTO challenges (channel_id, challenger, opponent, time_control, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![channel as i64, challenger as i64, opponent as i64, time_control, now, expires_at],
    )?;
    let id = conn.last_insert_rowid();
    get_challenge(conn, id).ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn get_challenge(conn: &Connection, id: i64) -> Option<Challenge> {
    conn.query_row(&format!("SELECT {} FROM challenges WHERE id = ?1", CHALLENGE_COLUMNS), params![id], challenge_from)
        .optional()
        .ok()
        .flatten()
}

pub fn set_challenge_message(conn: &Connection, id: i64, message: u64) -> rusqlite::Result<()> {
    conn.execute("UPDATE challenges SET message_id = ?2 WHERE id = ?1", params![id, message as i64]).map(|_| ())
}

/// The open challenge between two members, whichever way round it was sent.
pub fn open_between(conn: &Connection, a: u64, b: u64, now: i64) -> Option<Challenge> {
    conn.query_row(
        &format!(
            "SELECT {} FROM challenges WHERE status = 'open' AND expires_at > ?3
             AND ((challenger = ?1 AND opponent = ?2) OR (challenger = ?2 AND opponent = ?1))
             ORDER BY id DESC LIMIT 1",
            CHALLENGE_COLUMNS
        ),
        params![a as i64, b as i64, now],
        challenge_from,
    )
    .optional()
    .ok()
    .flatten()
}

pub fn open_challenges(conn: &Connection) -> Vec<Challenge> {
    let Ok(mut stmt) = conn.prepare(&format!("SELECT {} FROM challenges WHERE status = 'open' ORDER BY id", CHALLENGE_COLUMNS))
    else {
        return Vec::new();
    };
    stmt.query_map([], challenge_from).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

/// Closes an open challenge. `false` when it was already answered, which is how
/// two presses at once end up with only one game.
pub fn close_challenge(conn: &Connection, id: i64, status: ChallengeStatus) -> rusqlite::Result<bool> {
    let changed = conn.execute(
        "UPDATE challenges SET status = ?2 WHERE id = ?1 AND status = 'open'",
        params![id, status.key()],
    )?;
    Ok(changed > 0)
}

pub fn set_challenge_game(conn: &Connection, id: i64, game_id: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE challenges SET game_id = ?2 WHERE id = ?1", params![id, game_id]).map(|_| ())
}

// --- games ------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Game {
    pub id: i64,
    pub channel: u64,
    pub white: u64,
    pub black: u64,
    pub white_house: String,
    pub black_house: String,
    pub fen: String,
    /// The move list in SAN, oldest first.
    pub moves: Vec<String>,
    /// "running" or "done".
    pub status: String,
    pub time_control: String,
    pub per_move_secs: i64,
    pub started_at: i64,
    pub last_move_ts: i64,
    pub message: Option<u64>,
    /// "checkmate", "resign", "draw", "timeout", "stalemate", … while done.
    pub result: Option<String>,
    /// The winning member, or `None` for a draw.
    pub winner: Option<u64>,
    pub by_resignation: bool,
    pub finished_at: Option<i64>,
    pub points_white: i64,
    pub points_black: i64,
    pub paid_out: bool,
    pub why_nothing: String,
    /// Who has a draw on offer that the other side hasn't answered.
    pub draw_offer: Option<u64>,
    /// Whether the channel has been told about that offer.
    pub draw_announced: bool,
    pub nudged_white: bool,
    pub nudged_black: bool,
}

impl Game {
    pub fn running(&self) -> bool {
        self.status == "running"
    }

    pub fn has(&self, user: u64) -> bool {
        self.white == user || self.black == user
    }

    pub fn other(&self, user: u64) -> u64 {
        if user == self.white { self.black } else { self.white }
    }

    /// Whether a member plays white in this game.
    pub fn is_white(&self, user: u64) -> bool {
        self.white == user
    }

    /// Both players in one house, so the game pays nothing.
    pub fn same_house(&self) -> bool {
        !self.white_house.is_empty() && self.white_house == self.black_house
    }

    /// Whose move it is, from the move list: white on an even count.
    pub fn to_move(&self) -> u64 {
        if self.moves.len() % 2 == 0 { self.white } else { self.black }
    }
}

fn split_moves(text: &str) -> Vec<String> {
    text.split_whitespace().map(|s| s.to_string()).collect()
}

const GAME_COLUMNS: &str = "id, channel_id, white, black, white_house, black_house, fen, moves, status, time_control, \
     per_move_secs, started_at, last_move_ts, message_id, result, winner, by_resignation, finished_at, \
     points_white, points_black, paid_out, why_nothing, draw_offer, nudged_white, nudged_black, draw_announced";

fn game_from(row: &rusqlite::Row) -> rusqlite::Result<Game> {
    Ok(Game {
        id: row.get(0)?,
        channel: row.get::<_, i64>(1)? as u64,
        white: row.get::<_, i64>(2)? as u64,
        black: row.get::<_, i64>(3)? as u64,
        white_house: row.get(4)?,
        black_house: row.get(5)?,
        fen: row.get(6)?,
        moves: split_moves(&row.get::<_, String>(7)?),
        status: row.get(8)?,
        time_control: row.get(9)?,
        per_move_secs: row.get(10)?,
        started_at: row.get(11)?,
        last_move_ts: row.get(12)?,
        message: row.get::<_, Option<i64>>(13)?.map(|m| m as u64),
        result: row.get(14)?,
        winner: row.get::<_, Option<i64>>(15)?.map(|w| w as u64),
        by_resignation: row.get::<_, i64>(16)? != 0,
        finished_at: row.get(17)?,
        points_white: row.get(18)?,
        points_black: row.get(19)?,
        paid_out: row.get::<_, i64>(20)? != 0,
        why_nothing: row.get(21)?,
        draw_offer: row.get::<_, Option<i64>>(22)?.map(|u| u as u64),
        nudged_white: row.get::<_, i64>(23)? != 0,
        nudged_black: row.get::<_, i64>(24)? != 0,
        draw_announced: row.get::<_, i64>(25)? != 0,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn start_game(
    conn: &Connection,
    channel: u64,
    white: u64,
    black: u64,
    white_house: &str,
    black_house: &str,
    fen: &str,
    time_control: &str,
    per_move_secs: i64,
    now: i64,
    day: &str,
) -> rusqlite::Result<Game> {
    conn.execute(
        "INSERT INTO games (channel_id, white, black, white_house, black_house, fen, time_control, per_move_secs,
                            started_at, last_move_ts, day)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, ?10)",
        params![
            channel as i64,
            white as i64,
            black as i64,
            white_house,
            black_house,
            fen,
            time_control,
            per_move_secs,
            now,
            day
        ],
    )?;
    let id = conn.last_insert_rowid();
    get_game(conn, id).ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn get_game(conn: &Connection, id: i64) -> Option<Game> {
    conn.query_row(&format!("SELECT {} FROM games WHERE id = ?1", GAME_COLUMNS), params![id], game_from)
        .optional()
        .ok()
        .flatten()
}

pub fn running_games(conn: &Connection) -> Vec<Game> {
    let Ok(mut stmt) = conn.prepare(&format!("SELECT {} FROM games WHERE status = 'running' ORDER BY id", GAME_COLUMNS))
    else {
        return Vec::new();
    };
    stmt.query_map([], game_from).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

/// One member's running games, oldest first.
pub fn games_of(conn: &Connection, user: u64) -> Vec<Game> {
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT {} FROM games WHERE status = 'running' AND (white = ?1 OR black = ?1) ORDER BY id",
        GAME_COLUMNS
    )) else {
        return Vec::new();
    };
    stmt.query_map(params![user as i64], game_from).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

/// Games finished but not yet paid or announced, for a restart to pick up.
pub fn unfinished_business(conn: &Connection) -> Vec<Game> {
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT {} FROM games WHERE status = 'done' AND paid_out = 0 ORDER BY id",
        GAME_COLUMNS
    )) else {
        return Vec::new();
    };
    stmt.query_map([], game_from).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

pub fn set_message(conn: &Connection, id: i64, message: Option<u64>) -> rusqlite::Result<()> {
    conn.execute("UPDATE games SET message_id = ?2 WHERE id = ?1", params![id, message.map(|m| m as i64)]).map(|_| ())
}

/// Adds one move. The old move list has to be exactly what the caller last saw,
/// so two moves arriving together can never both land.
///
/// A draw on offer is taken away - it was an offer about the position that has
/// just changed - but the nudge marks are NOT cleared: a side is tapped on the
/// shoulder at most once in a whole game, however long it runs.
pub fn play_move(
    conn: &Connection,
    id: i64,
    expected: &[String],
    san: &str,
    fen: &str,
    now: i64,
) -> rusqlite::Result<bool> {
    let was = expected.join(" ");
    let mut now_list = was.clone();
    if !now_list.is_empty() {
        now_list.push(' ');
    }
    now_list.push_str(san);
    let changed = conn.execute(
        "UPDATE games SET moves = ?2, fen = ?3, last_move_ts = ?4, draw_offer = NULL
         WHERE id = ?1 AND status = 'running' AND moves = ?5",
        params![id, now_list, fen, now, was],
    )?;
    Ok(changed > 0)
}

/// Ends a game. `false` when it had already ended.
pub fn finish_game(
    conn: &Connection,
    id: i64,
    result: &str,
    winner: Option<u64>,
    by_resignation: bool,
    now: i64,
) -> rusqlite::Result<bool> {
    let changed = conn.execute(
        "UPDATE games SET status = 'done', result = ?2, winner = ?3, by_resignation = ?4, finished_at = ?5,
                          draw_offer = NULL
         WHERE id = ?1 AND status = 'running'",
        params![id, result, winner.map(|w| w as i64), by_resignation as i64, now],
    )?;
    Ok(changed > 0)
}

pub fn record_payout(
    conn: &Connection,
    id: i64,
    white: i64,
    black: i64,
    why_nothing: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE games SET points_white = ?2, points_black = ?3, why_nothing = ?4, paid_out = 1 WHERE id = ?1",
        params![id, white, black, why_nothing],
    )
    .map(|_| ())
}

/// Puts a draw on offer from `user`, unless one is already open.
pub fn offer_draw(conn: &Connection, id: i64, user: u64) -> rusqlite::Result<bool> {
    let changed = conn.execute(
        "UPDATE games SET draw_offer = ?2, draw_announced = 0 WHERE id = ?1 AND status = 'running' AND draw_offer IS NULL",
        params![id, user as i64],
    )?;
    Ok(changed > 0)
}

/// Claims the job of telling the channel about an open draw offer. True once
/// per offer, whichever side it came from and whether it came from a button or
/// the board page.
pub fn announce_draw(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    let changed = conn.execute(
        "UPDATE games SET draw_announced = 1
         WHERE id = ?1 AND status = 'running' AND draw_offer IS NOT NULL AND draw_announced = 0",
        params![id],
    )?;
    Ok(changed > 0)
}

/// Takes a draw offer away and says whether one was there from someone else.
pub fn take_draw_offer(conn: &Connection, id: i64, from: u64) -> rusqlite::Result<bool> {
    let changed = conn.execute(
        "UPDATE games SET draw_offer = NULL WHERE id = ?1 AND status = 'running' AND draw_offer = ?2",
        params![id, from as i64],
    )?;
    Ok(changed > 0)
}

/// Notes that the player on `white`'s side has had their one nudge.
pub fn mark_nudged(conn: &Connection, id: i64, white: bool) -> rusqlite::Result<bool> {
    let column = if white { "nudged_white" } else { "nudged_black" };
    let changed = conn.execute(
        &format!("UPDATE games SET {0} = 1 WHERE id = ?1 AND status = 'running' AND {0} = 0", column),
        params![id],
    )?;
    Ok(changed > 0)
}

// --- the private board links -------------------------------------------------------------

/// Characters a link uses: no vowels, so no word can appear in one by accident.
const TOKEN_ALPHABET: &[u8] = b"bcdfghjkmnpqrstvwxyz23456789";
pub const TOKEN_LEN: usize = 24;

/// A fresh random link, from the caller's own random numbers.
pub fn make_token(mut roll: impl FnMut() -> f64) -> String {
    (0..TOKEN_LEN)
        .map(|_| {
            let n = (roll().clamp(0.0, 0.999_999) * TOKEN_ALPHABET.len() as f64) as usize;
            TOKEN_ALPHABET[n.min(TOKEN_ALPHABET.len() - 1)] as char
        })
        .collect()
}

/// The link for one player in one game, made once and then kept, so opening the
/// board twice gives the same address.
pub fn token_for(conn: &Connection, game_id: i64, user: u64, now: i64, roll: impl FnMut() -> f64) -> Option<String> {
    let existing: Option<String> = conn
        .query_row("SELECT token FROM tokens WHERE game_id = ?1 AND user_id = ?2", params![game_id, user as i64], |r| {
            r.get(0)
        })
        .optional()
        .ok()
        .flatten();
    if let Some(token) = existing {
        return Some(token);
    }
    let token = make_token(roll);
    conn.execute(
        "INSERT INTO tokens (token, game_id, user_id, made_at) VALUES (?1, ?2, ?3, ?4)",
        params![token, game_id, user as i64, now],
    )
    .ok()?;
    Some(token)
}

/// Which game and which player a link opens. Unknown links get nothing, and a
/// link only ever names its own side.
pub fn token_owner(conn: &Connection, token: &str) -> Option<(i64, u64)> {
    if token.len() != TOKEN_LEN || !token.bytes().all(|b| TOKEN_ALPHABET.contains(&b)) {
        return None;
    }
    conn.query_row("SELECT game_id, user_id FROM tokens WHERE token = ?1", params![token], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? as u64))
    })
    .optional()
    .ok()
    .flatten()
}

/// Throws away a finished game's links.
pub fn drop_tokens(conn: &Connection, game_id: i64) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM tokens WHERE game_id = ?1", params![game_id]).map(|_| ())
}

/// Everyone who won a game finished between two moments, with when they won
/// it. Draws are not wins and are left out.
pub fn wins_between(conn: &Connection, from: i64, until: i64) -> Vec<(u64, i64)> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT winner, finished_at FROM games
         WHERE status = 'done' AND winner IS NOT NULL AND finished_at >= ?1 AND finished_at < ?2
         ORDER BY finished_at, id",
    ) else {
        return Vec::new();
    };
    stmt.query_map(params![from, until], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)?)))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

// --- one pair, one payday ------------------------------------------------------------------

/// Claims the day for this pair. True when this game is the pair's first of the
/// day - and true again for the SAME game, so a replay after a restart reaches
/// the same answer and the ledger's own key stops it paying twice.
pub fn claim_pair_day(conn: &Connection, day: &str, a: u64, b: u64, game_id: i64) -> rusqlite::Result<bool> {
    let (low, high) = if a < b { (a, b) } else { (b, a) };
    conn.execute(
        "INSERT OR IGNORE INTO pair_days (day, low, high, game_id) VALUES (?1, ?2, ?3, ?4)",
        params![day, low as i64, high as i64, game_id],
    )?;
    let holder: Option<i64> = conn
        .query_row(
            "SELECT game_id FROM pair_days WHERE day = ?1 AND low = ?2 AND high = ?3",
            params![day, low as i64, high as i64],
            |r| r.get(0),
        )
        .optional()?;
    Ok(holder == Some(game_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().expect("memory db");
        init(&conn).expect("schema");
        conn
    }

    fn game(conn: &Connection) -> Game {
        start_game(conn, 5, 1, 2, "gryffindor", "slytherin", "startfen", "casual", 43_200, 1_000, "2026-09-16")
            .expect("a game")
    }

    #[test]
    fn a_game_starts_running_with_its_clock_wound() {
        let conn = conn();
        let g = game(&conn);
        assert!(g.running());
        assert_eq!((g.white, g.black), (1, 2));
        assert_eq!(g.last_move_ts, 1_000);
        assert_eq!(g.to_move(), 1, "white moves first");
        assert!(!g.same_house());
        assert!(g.has(1) && g.has(2) && !g.has(3));
        assert_eq!(g.other(1), 2);
        assert_eq!(games_of(&conn, 1).len(), 1);
        assert!(games_of(&conn, 9).is_empty());
    }

    #[test]
    fn two_moves_landing_together_only_one_lands() {
        let conn = conn();
        let g = game(&conn);
        assert!(play_move(&conn, g.id, &g.moves, "e4", "fen1", 1_100).unwrap());
        // The second caller still holds the move list from before, so it is refused.
        assert!(!play_move(&conn, g.id, &g.moves, "d4", "fen2", 1_101).unwrap());
        let after = get_game(&conn, g.id).unwrap();
        assert_eq!(after.moves, vec!["e4".to_string()]);
        assert_eq!(after.fen, "fen1");
        assert_eq!(after.last_move_ts, 1_100);
        assert_eq!(after.to_move(), 2, "black's turn now");
        assert!(play_move(&conn, g.id, &after.moves, "e5", "fen2", 1_200).unwrap());
        assert_eq!(get_game(&conn, g.id).unwrap().moves.len(), 2);
    }

    #[test]
    fn two_games_run_side_by_side_without_touching_each_other() {
        let conn = conn();
        let a = game(&conn);
        let b = start_game(&conn, 5, 3, 4, "hufflepuff", "slytherin", "startfen", "live", 180, 1_000, "2026-09-16").unwrap();
        assert_ne!(a.id, b.id);
        assert_eq!(running_games(&conn).len(), 2);
        // A move in one leaves the other exactly as it was.
        assert!(play_move(&conn, a.id, &a.moves, "e4", "fen-a", 1_100).unwrap());
        let (after_a, after_b) = (get_game(&conn, a.id).unwrap(), get_game(&conn, b.id).unwrap());
        assert_eq!(after_a.moves, vec!["e4".to_string()]);
        assert_eq!(after_a.last_move_ts, 1_100);
        assert_eq!(after_b, b, "the other game is untouched");
        // Each keeps its own card.
        set_message(&conn, a.id, Some(70)).unwrap();
        set_message(&conn, b.id, Some(71)).unwrap();
        assert_eq!(get_game(&conn, a.id).unwrap().message, Some(70));
        assert_eq!(get_game(&conn, b.id).unwrap().message, Some(71));
        // And finishing one leaves the other running.
        finish_game(&conn, a.id, "resign", Some(2), true, 1_200).unwrap();
        assert_eq!(running_games(&conn).iter().map(|g| g.id).collect::<Vec<_>>(), vec![b.id]);
        assert_eq!(games_of(&conn, 3).len(), 1);
        assert_eq!(games_of(&conn, 1).len(), 0, "a finished game is not still running");
    }

    #[test]
    fn a_game_can_only_finish_once() {
        let conn = conn();
        let g = game(&conn);
        assert!(finish_game(&conn, g.id, "resign", Some(2), true, 2_000).unwrap());
        assert!(!finish_game(&conn, g.id, "checkmate", Some(1), false, 2_001).unwrap(), "a second ending is refused");
        let done = get_game(&conn, g.id).unwrap();
        assert!(!done.running());
        assert_eq!((done.result.as_deref(), done.winner, done.by_resignation), (Some("resign"), Some(2), true));
        assert!(!play_move(&conn, g.id, &done.moves, "e4", "fen", 2_002).unwrap(), "and no more moves");
        assert!(running_games(&conn).is_empty());
        assert_eq!(unfinished_business(&conn).len(), 1, "still to be paid");
        record_payout(&conn, g.id, 0, 4, "").unwrap();
        assert!(unfinished_business(&conn).is_empty());
    }

    #[test]
    fn a_draw_offer_is_only_open_once_and_a_move_takes_it_away() {
        let conn = conn();
        let g = game(&conn);
        assert!(offer_draw(&conn, g.id, 1).unwrap());
        assert!(!offer_draw(&conn, g.id, 2).unwrap(), "one offer at a time");
        assert!(announce_draw(&conn, g.id).unwrap(), "the channel is told");
        assert!(!announce_draw(&conn, g.id).unwrap(), "but only once");
        assert_eq!(get_game(&conn, g.id).unwrap().draw_offer, Some(1));
        assert!(!take_draw_offer(&conn, g.id, 2).unwrap(), "only the offer that is really there");
        assert!(take_draw_offer(&conn, g.id, 1).unwrap());
        assert_eq!(get_game(&conn, g.id).unwrap().draw_offer, None);
        // A move clears an offer that was never answered.
        offer_draw(&conn, g.id, 2).unwrap();
        let g = get_game(&conn, g.id).unwrap();
        play_move(&conn, g.id, &g.moves, "e4", "fen", 1_100).unwrap();
        assert_eq!(get_game(&conn, g.id).unwrap().draw_offer, None);
    }

    #[test]
    fn each_side_is_nudged_once_in_a_whole_game() {
        let conn = conn();
        let g = game(&conn);
        assert!(mark_nudged(&conn, g.id, true).unwrap());
        assert!(!mark_nudged(&conn, g.id, true).unwrap(), "only once");
        assert!(mark_nudged(&conn, g.id, false).unwrap(), "the other side has its own");
        let g = get_game(&conn, g.id).unwrap();
        assert!(g.nudged_white && g.nudged_black);
        play_move(&conn, g.id, &g.moves, "e4", "fen", 1_100).unwrap();
        let moved = get_game(&conn, g.id).unwrap();
        assert!(moved.nudged_white && moved.nudged_black, "a move does not earn another nudge");
        assert!(!mark_nudged(&conn, g.id, true).unwrap());
    }

    #[test]
    fn a_link_opens_its_own_game_and_side_and_nothing_else() {
        let conn = conn();
        let g = game(&conn);
        let mut n = 0u64;
        let mut roll = || {
            n += 1;
            (n as f64 * 0.137) % 1.0
        };
        let white = token_for(&conn, g.id, 1, 1_000, &mut roll).expect("white's link");
        let black = token_for(&conn, g.id, 2, 1_000, &mut roll).expect("black's link");
        assert_ne!(white, black);
        assert_eq!(white.len(), TOKEN_LEN);
        assert_eq!(token_for(&conn, g.id, 1, 1_050, &mut roll).as_deref(), Some(white.as_str()), "the same link twice");
        assert_eq!(token_owner(&conn, &white), Some((g.id, 1)));
        assert_eq!(token_owner(&conn, &black), Some((g.id, 2)));
        assert_eq!(token_owner(&conn, "notatokenatallnotatoken"), None);
        assert_eq!(token_owner(&conn, &white.to_uppercase()), None, "case matters");
        assert_eq!(token_owner(&conn, ""), None);
        // A link dies with its game.
        drop_tokens(&conn, g.id).unwrap();
        assert_eq!(token_owner(&conn, &white), None);
        assert_eq!(token_owner(&conn, &black), None);
    }

    #[test]
    fn only_wins_inside_the_window_are_counted() {
        let conn = conn();
        let a = game(&conn);
        let b = game(&conn);
        let c = game(&conn);
        finish_game(&conn, a.id, "checkmate", Some(1), false, 5_000).unwrap();
        finish_game(&conn, b.id, "draw", None, false, 5_100).unwrap();
        finish_game(&conn, c.id, "resign", Some(2), true, 9_000).unwrap();
        assert_eq!(wins_between(&conn, 0, 6_000), vec![(1, 5_000)], "a draw has no winner");
        assert_eq!(wins_between(&conn, 0, 10_000), vec![(1, 5_000), (2, 9_000)]);
        assert!(wins_between(&conn, 10_000, 20_000).is_empty());
    }

    #[test]
    fn a_pair_claims_a_day_once_but_the_same_game_can_ask_again() {
        let conn = conn();
        assert!(claim_pair_day(&conn, "2026-09-16", 7, 9, 1).unwrap());
        assert!(claim_pair_day(&conn, "2026-09-16", 9, 7, 1).unwrap(), "the same game, asked again");
        assert!(!claim_pair_day(&conn, "2026-09-16", 7, 9, 2).unwrap(), "their second game today");
        assert!(claim_pair_day(&conn, "2026-09-17", 7, 9, 2).unwrap(), "tomorrow is a new day");
        assert!(claim_pair_day(&conn, "2026-09-16", 7, 8, 3).unwrap(), "a different pair");
    }

    #[test]
    fn a_challenge_is_answered_once_and_only_one_stands_per_pair() {
        let conn = conn();
        let c = create_challenge(&conn, 5, 1, 2, "casual", 1_000, 2_800).expect("a challenge");
        assert_eq!(c.status, ChallengeStatus::Open);
        set_challenge_message(&conn, c.id, 77).unwrap();
        assert_eq!(get_challenge(&conn, c.id).unwrap().message, Some(77));
        assert_eq!(open_between(&conn, 2, 1, 1_500).map(|c| c.id), Some(c.id), "either way round");
        assert_eq!(open_between(&conn, 2, 1, 3_000), None, "an expired one doesn't block");
        assert_eq!(open_between(&conn, 1, 3, 1_500), None);
        assert_eq!(open_challenges(&conn).len(), 1);
        assert!(close_challenge(&conn, c.id, ChallengeStatus::Accepted).unwrap());
        assert!(!close_challenge(&conn, c.id, ChallengeStatus::Declined).unwrap(), "already answered");
        assert_eq!(get_challenge(&conn, c.id).unwrap().status, ChallengeStatus::Accepted);
        assert!(open_challenges(&conn).is_empty());
        assert_eq!(open_between(&conn, 1, 2, 1_500), None, "answered, so a new one may be sent");
    }

    #[test]
    fn a_same_house_game_is_marked_as_one() {
        let conn = conn();
        let g = start_game(&conn, 5, 1, 2, "ravenclaw", "ravenclaw", "fen", "live", 180, 10, "2026-09-16").unwrap();
        assert!(g.same_house());
        let unsorted = start_game(&conn, 5, 3, 4, "", "", "fen", "live", 180, 10, "2026-09-16").unwrap();
        assert!(!unsorted.same_house(), "two unsorted members are not one house");
    }

    #[test]
    fn meta_remembers_the_card() {
        let conn = conn();
        assert_eq!(meta_get(&conn, "card"), None);
        meta_set(&conn, "card", "5:77").unwrap();
        meta_set(&conn, "card", "5:78").unwrap();
        assert_eq!(meta_get(&conn, "card").as_deref(), Some("5:78"));
    }
}
