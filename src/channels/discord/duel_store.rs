//! What Letter Duel keeps in `.runtime/duel.db`: the lobbies people join, the
//! games themselves, every turn taken, the private board links, and what each
//! game paid.
//!
//! The board, the bag and each player's rack are plain strings (see
//! [`super::duel_rules`]), so a whole position is four short columns and a game
//! can be drawn, or judged, without replaying anything.
//!
//! **Two presses can never both land.** Every turn is one
//! `UPDATE … WHERE status = 'running' AND turns = ?`: the number of turns taken
//! is the game's version, and the second press finds the version it expected is
//! gone and does nothing. Finishing a game works the same way, which is what
//! stops a result being paid or posted twice.
//!
//! The board links are random strings, one per player per game, and they are
//! deleted the moment the game ends: a link is only ever good for the game and
//! the seat it was made for.

use std::sync::OnceLock;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

pub const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS lobbies (
        id INTEGER PRIMARY KEY AUTOINCREMENT, channel_id INTEGER NOT NULL, message_id INTEGER,
        opened_by INTEGER NOT NULL DEFAULT 0, opened_at INTEGER NOT NULL, closes_at INTEGER NOT NULL,
        status TEXT NOT NULL DEFAULT 'open', game_id INTEGER);
    CREATE INDEX IF NOT EXISTS lobbies_status ON lobbies (status);
    CREATE TABLE IF NOT EXISTS lobby_seats (
        lobby_id INTEGER NOT NULL, user_id INTEGER NOT NULL, house TEXT NOT NULL DEFAULT '',
        joined_at INTEGER NOT NULL, PRIMARY KEY (lobby_id, user_id));
    CREATE TABLE IF NOT EXISTS games (
        id INTEGER PRIMARY KEY AUTOINCREMENT, channel_id INTEGER NOT NULL, message_id INTEGER,
        status TEXT NOT NULL DEFAULT 'running', board TEXT NOT NULL, bag TEXT NOT NULL,
        turn INTEGER NOT NULL DEFAULT 0, passes INTEGER NOT NULL DEFAULT 0, turns INTEGER NOT NULL DEFAULT 0,
        turn_secs INTEGER NOT NULL, started_at INTEGER NOT NULL, turn_started_at INTEGER NOT NULL,
        finished_at INTEGER, result TEXT, went_out INTEGER,
        paid_out INTEGER NOT NULL DEFAULT 0, why_nothing TEXT NOT NULL DEFAULT '',
        given_back INTEGER NOT NULL DEFAULT 0, day TEXT NOT NULL DEFAULT '');
    CREATE INDEX IF NOT EXISTS games_status ON games (status);
    CREATE TABLE IF NOT EXISTS players (
        game_id INTEGER NOT NULL, seat INTEGER NOT NULL, user_id INTEGER NOT NULL,
        house TEXT NOT NULL DEFAULT '', rack TEXT NOT NULL DEFAULT '',
        score INTEGER NOT NULL DEFAULT 0, adjust INTEGER NOT NULL DEFAULT 0,
        misses INTEGER NOT NULL DEFAULT 0, dropped INTEGER NOT NULL DEFAULT 0,
        points INTEGER NOT NULL DEFAULT 0, worth INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (game_id, seat));
    CREATE INDEX IF NOT EXISTS players_user ON players (user_id);
    CREATE TABLE IF NOT EXISTS plays (
        id INTEGER PRIMARY KEY AUTOINCREMENT, game_id INTEGER NOT NULL, seat INTEGER NOT NULL,
        kind TEXT NOT NULL, word TEXT NOT NULL DEFAULT '', score INTEGER NOT NULL DEFAULT 0,
        squares TEXT NOT NULL DEFAULT '', ts INTEGER NOT NULL);
    CREATE INDEX IF NOT EXISTS plays_game ON plays (game_id, id);
    CREATE TABLE IF NOT EXISTS tokens (
        token TEXT PRIMARY KEY, game_id INTEGER NOT NULL, user_id INTEGER NOT NULL, made_at INTEGER NOT NULL);
    CREATE INDEX IF NOT EXISTS tokens_game ON tokens (game_id);
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);";

pub fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)
}

pub fn open(workspace: &str) -> anyhow::Result<()> {
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    std::fs::create_dir_all(&dir)?;
    let conn = Connection::open(dir.join("duel.db"))?;
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

// --- the lobby ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LobbyStatus {
    Open,
    Started,
    CalledOff,
}

impl LobbyStatus {
    pub fn key(self) -> &'static str {
        match self {
            LobbyStatus::Open => "open",
            LobbyStatus::Started => "started",
            LobbyStatus::CalledOff => "called off",
        }
    }

    pub fn from_key(key: &str) -> LobbyStatus {
        match key {
            "started" => LobbyStatus::Started,
            "called off" => LobbyStatus::CalledOff,
            _ => LobbyStatus::Open,
        }
    }
}

/// Somebody waiting in a lobby.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seat {
    pub user: u64,
    pub house: String,
    pub joined_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lobby {
    pub id: i64,
    pub channel: u64,
    pub message: Option<u64>,
    pub opened_by: u64,
    pub opened_at: i64,
    pub closes_at: i64,
    pub status: LobbyStatus,
    pub game_id: Option<i64>,
    pub seats: Vec<Seat>,
}

impl Lobby {
    pub fn has(&self, user: u64) -> bool {
        self.seats.iter().any(|s| s.user == user)
    }

    pub fn players(&self) -> usize {
        self.seats.len()
    }
}

const LOBBY_COLUMNS: &str = "id, channel_id, message_id, opened_by, opened_at, closes_at, status, game_id";

fn lobby_from(row: &rusqlite::Row) -> rusqlite::Result<Lobby> {
    Ok(Lobby {
        id: row.get(0)?,
        channel: row.get::<_, i64>(1)? as u64,
        message: row.get::<_, Option<i64>>(2)?.map(|m| m as u64),
        opened_by: row.get::<_, i64>(3)? as u64,
        opened_at: row.get(4)?,
        closes_at: row.get(5)?,
        status: LobbyStatus::from_key(&row.get::<_, String>(6)?),
        game_id: row.get(7)?,
        seats: Vec::new(),
    })
}

fn seats_of(conn: &Connection, lobby: i64) -> Vec<Seat> {
    conn.prepare("SELECT user_id, house, joined_at FROM lobby_seats WHERE lobby_id = ?1 ORDER BY joined_at, user_id")
        .and_then(|mut q| {
            q.query_map(params![lobby], |r| {
                Ok(Seat { user: r.get::<_, i64>(0)? as u64, house: r.get(1)?, joined_at: r.get(2)? })
            })
            .map(|rows| rows.flatten().collect())
        })
        .unwrap_or_default()
}

pub fn open_lobby(conn: &Connection, channel: u64, by: u64, now: i64, closes_at: i64) -> rusqlite::Result<Lobby> {
    conn.execute(
        "INSERT INTO lobbies (channel_id, opened_by, opened_at, closes_at) VALUES (?1, ?2, ?3, ?4)",
        params![channel as i64, by as i64, now, closes_at],
    )?;
    get_lobby(conn, conn.last_insert_rowid()).ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn get_lobby(conn: &Connection, id: i64) -> Option<Lobby> {
    let mut lobby = conn
        .query_row(&format!("SELECT {} FROM lobbies WHERE id = ?1", LOBBY_COLUMNS), params![id], lobby_from)
        .optional()
        .ok()
        .flatten()?;
    lobby.seats = seats_of(conn, lobby.id);
    Some(lobby)
}

/// The lobby still open in a channel, if there is one.
pub fn live_lobby(conn: &Connection, channel: u64) -> Option<Lobby> {
    let id: i64 = conn
        .query_row(
            "SELECT id FROM lobbies WHERE status = 'open' AND channel_id = ?1 ORDER BY id DESC LIMIT 1",
            params![channel as i64],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten()?;
    get_lobby(conn, id)
}

/// Every lobby still open, whichever channel it is in.
pub fn open_lobbies(conn: &Connection) -> Vec<Lobby> {
    let ids: Vec<i64> = conn
        .prepare("SELECT id FROM lobbies WHERE status = 'open' ORDER BY id")
        .and_then(|mut q| q.query_map([], |r| r.get(0)).map(|rows| rows.flatten().collect()))
        .unwrap_or_default();
    ids.into_iter().filter_map(|id| get_lobby(conn, id)).collect()
}

pub fn set_lobby_message(conn: &Connection, id: i64, message: Option<u64>) -> rusqlite::Result<()> {
    conn.execute("UPDATE lobbies SET message_id = ?2 WHERE id = ?1", params![id, message.map(|m| m as i64)]).map(|_| ())
}

/// Takes a seat. `false` when the lobby has closed, is full, or they are
/// already in it — the caller says which by looking at the lobby it read.
pub fn join_lobby(conn: &Connection, lobby: i64, user: u64, house: &str, now: i64, max: usize) -> rusqlite::Result<bool> {
    let open: bool = conn
        .query_row("SELECT status = 'open' FROM lobbies WHERE id = ?1", params![lobby], |r| r.get(0))
        .optional()?
        .unwrap_or(false);
    if !open {
        return Ok(false);
    }
    let taken: i64 = conn.query_row("SELECT COUNT(*) FROM lobby_seats WHERE lobby_id = ?1", params![lobby], |r| r.get(0))?;
    if taken as usize >= max {
        return Ok(false);
    }
    let rows = conn.execute(
        "INSERT OR IGNORE INTO lobby_seats (lobby_id, user_id, house, joined_at) VALUES (?1, ?2, ?3, ?4)",
        params![lobby, user as i64, house, now],
    )?;
    Ok(rows == 1)
}

/// Gives a seat up again.
pub fn leave_lobby(conn: &Connection, lobby: i64, user: u64) -> rusqlite::Result<bool> {
    let rows = conn.execute(
        "DELETE FROM lobby_seats WHERE lobby_id = ?1 AND user_id = ?2
         AND EXISTS (SELECT 1 FROM lobbies WHERE id = ?1 AND status = 'open')",
        params![lobby, user as i64],
    )?;
    Ok(rows == 1)
}

/// Closes a lobby. Only the first call finds it open, so a countdown running
/// out and a lobby filling up can never both start a game.
pub fn close_lobby(conn: &Connection, id: i64, status: LobbyStatus, game: Option<i64>) -> rusqlite::Result<bool> {
    let rows = conn.execute(
        "UPDATE lobbies SET status = ?2, game_id = ?3 WHERE id = ?1 AND status = 'open'",
        params![id, status.key(), game],
    )?;
    Ok(rows == 1)
}

/// Notes which game a closed lobby became, once the game exists. The lobby is
/// claimed first and the game made after, so this is a second step.
pub fn set_lobby_game(conn: &Connection, id: i64, game: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE lobbies SET game_id = ?2 WHERE id = ?1", params![id, game]).map(|_| ())
}

/// Hands every open lobby the time the bot was away, so a restart never calls
/// one off that people were still joining.
pub fn give_lobbies_time_back(conn: &Connection, seconds: i64) -> rusqlite::Result<usize> {
    conn.execute("UPDATE lobbies SET closes_at = closes_at + ?1 WHERE status = 'open'", params![seconds])
}

// --- the game ------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Player {
    pub seat: usize,
    pub user: u64,
    pub house: String,
    pub rack: String,
    /// What they have scored from the board.
    pub score: i64,
    /// What the leftover tiles did to it at the end, plus or minus.
    pub adjust: i64,
    /// Turns they let run out.
    pub misses: i64,
    pub dropped: bool,
    /// House points the ledger actually paid.
    pub points: i64,
    /// What they were worth before the daily limit — their duel points.
    pub worth: i64,
}

impl Player {
    /// What they finished on.
    pub fn total(&self) -> i64 {
        self.score + self.adjust
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Game {
    pub id: i64,
    pub channel: u64,
    pub message: Option<u64>,
    pub status: String,
    pub board: String,
    pub bag: String,
    pub turn: usize,
    /// Turns in a row that scored nothing.
    pub passes: i64,
    /// How many turns have been taken — and the game's version number.
    pub turns: i64,
    pub turn_secs: i64,
    pub started_at: i64,
    pub turn_started_at: i64,
    pub finished_at: Option<i64>,
    pub result: Option<String>,
    pub went_out: Option<usize>,
    pub paid_out: bool,
    pub why_nothing: String,
    pub given_back: i64,
    pub day: String,
    pub players: Vec<Player>,
}

impl Game {
    pub fn running(&self) -> bool {
        self.status == "running"
    }

    pub fn has(&self, user: u64) -> bool {
        self.players.iter().any(|p| p.user == user)
    }

    pub fn seat_of(&self, user: u64) -> Option<usize> {
        self.players.iter().find(|p| p.user == user).map(|p| p.seat)
    }

    pub fn player(&self, seat: usize) -> Option<&Player> {
        self.players.iter().find(|p| p.seat == seat)
    }

    /// The seats still playing, in order.
    pub fn playing(&self) -> Vec<usize> {
        self.players.iter().filter(|p| !p.dropped).map(|p| p.seat).collect()
    }

    /// Whose turn it is, when somebody is still playing.
    pub fn to_move(&self) -> Option<u64> {
        self.player(self.turn).filter(|p| !p.dropped).map(|p| p.user)
    }

    /// The seat after this one that is still playing. `None` when nobody is.
    pub fn next_seat(&self, from: usize) -> Option<usize> {
        let seats = self.playing();
        if seats.is_empty() {
            return None;
        }
        let after = seats.iter().find(|s| **s > from).copied();
        after.or_else(|| seats.first().copied())
    }

    /// How many tiles are still to be drawn.
    pub fn bag_left(&self) -> usize {
        self.bag.chars().count()
    }
}

const GAME_COLUMNS: &str = "id, channel_id, message_id, status, board, bag, turn, passes, turns, turn_secs,
     started_at, turn_started_at, finished_at, result, went_out, paid_out, why_nothing, given_back, day";

fn game_from(row: &rusqlite::Row) -> rusqlite::Result<Game> {
    Ok(Game {
        id: row.get(0)?,
        channel: row.get::<_, i64>(1)? as u64,
        message: row.get::<_, Option<i64>>(2)?.map(|m| m as u64),
        status: row.get(3)?,
        board: row.get(4)?,
        bag: row.get(5)?,
        turn: row.get::<_, i64>(6)? as usize,
        passes: row.get(7)?,
        turns: row.get(8)?,
        turn_secs: row.get(9)?,
        started_at: row.get(10)?,
        turn_started_at: row.get(11)?,
        finished_at: row.get(12)?,
        result: row.get(13)?,
        went_out: row.get::<_, Option<i64>>(14)?.map(|s| s as usize),
        paid_out: row.get::<_, i64>(15)? != 0,
        why_nothing: row.get(16)?,
        given_back: row.get(17)?,
        day: row.get(18)?,
        players: Vec::new(),
    })
}

fn players_of(conn: &Connection, game: i64) -> Vec<Player> {
    conn.prepare(
        "SELECT seat, user_id, house, rack, score, adjust, misses, dropped, points, worth
         FROM players WHERE game_id = ?1 ORDER BY seat",
    )
    .and_then(|mut q| {
        q.query_map(params![game], |r| {
            Ok(Player {
                seat: r.get::<_, i64>(0)? as usize,
                user: r.get::<_, i64>(1)? as u64,
                house: r.get(2)?,
                rack: r.get(3)?,
                score: r.get(4)?,
                adjust: r.get(5)?,
                misses: r.get(6)?,
                dropped: r.get::<_, i64>(7)? != 0,
                points: r.get(8)?,
                worth: r.get(9)?,
            })
        })
        .map(|rows| rows.flatten().collect())
    })
    .unwrap_or_default()
}

/// Starts a game. `seats` is who is playing, in seat order, with the house each
/// counts as being in and the rack they were dealt.
pub fn start_game(
    conn: &Connection,
    channel: u64,
    seats: &[(u64, String, String)],
    board: &str,
    bag: &str,
    turn_secs: i64,
    now: i64,
    day: &str,
) -> rusqlite::Result<Game> {
    conn.execute(
        "INSERT INTO games (channel_id, board, bag, turn_secs, started_at, turn_started_at, day)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6)",
        params![channel as i64, board, bag, turn_secs, now, day],
    )?;
    let id = conn.last_insert_rowid();
    for (seat, (user, house, rack)) in seats.iter().enumerate() {
        conn.execute(
            "INSERT INTO players (game_id, seat, user_id, house, rack) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, seat as i64, *user as i64, house, rack],
        )?;
    }
    get_game(conn, id).ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn get_game(conn: &Connection, id: i64) -> Option<Game> {
    let mut game = conn
        .query_row(&format!("SELECT {} FROM games WHERE id = ?1", GAME_COLUMNS), params![id], game_from)
        .optional()
        .ok()
        .flatten()?;
    game.players = players_of(conn, game.id);
    Some(game)
}

/// The game running in a channel, if there is one.
pub fn live_game(conn: &Connection, channel: u64) -> Option<Game> {
    let id: i64 = conn
        .query_row(
            "SELECT id FROM games WHERE status = 'running' AND channel_id = ?1 ORDER BY id DESC LIMIT 1",
            params![channel as i64],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten()?;
    get_game(conn, id)
}

pub fn running_games(conn: &Connection) -> Vec<Game> {
    let ids: Vec<i64> = conn
        .prepare("SELECT id FROM games WHERE status = 'running' ORDER BY id")
        .and_then(|mut q| q.query_map([], |r| r.get(0)).map(|rows| rows.flatten().collect()))
        .unwrap_or_default();
    ids.into_iter().filter_map(|id| get_game(conn, id)).collect()
}

/// Games that have ended but have not been paid and announced yet.
pub fn unfinished_business(conn: &Connection) -> Vec<Game> {
    let ids: Vec<i64> = conn
        .prepare("SELECT id FROM games WHERE status = 'over' AND paid_out = 0 ORDER BY id")
        .and_then(|mut q| q.query_map([], |r| r.get(0)).map(|rows| rows.flatten().collect()))
        .unwrap_or_default();
    ids.into_iter().filter_map(|id| get_game(conn, id)).collect()
}

pub fn set_message(conn: &Connection, game: i64, message: Option<u64>) -> rusqlite::Result<()> {
    conn.execute("UPDATE games SET message_id = ?2 WHERE id = ?1", params![game, message.map(|m| m as i64)]).map(|_| ())
}

/// What a turn did, for the history the card reads back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    pub seat: usize,
    pub kind: String,
    pub word: String,
    pub score: i64,
    /// The squares covered, as numbers separated by commas.
    pub squares: String,
    pub ts: i64,
}

impl Turn {
    /// The squares the play covered, for the picture's highlight.
    pub fn covered(&self) -> Vec<usize> {
        self.squares.split(',').filter_map(|s| s.trim().parse::<usize>().ok()).collect()
    }
}

/// The turns a game has had, oldest first.
pub fn turns_of(conn: &Connection, game: i64) -> Vec<Turn> {
    conn.prepare("SELECT seat, kind, word, score, squares, ts FROM plays WHERE game_id = ?1 ORDER BY id")
        .and_then(|mut q| {
            q.query_map(params![game], |r| {
                Ok(Turn {
                    seat: r.get::<_, i64>(0)? as usize,
                    kind: r.get(1)?,
                    word: r.get(2)?,
                    score: r.get(3)?,
                    squares: r.get(4)?,
                    ts: r.get(5)?,
                })
            })
            .map(|rows| rows.flatten().collect())
        })
        .unwrap_or_default()
}

/// The last turn taken, which is what the card shows.
pub fn last_turn(conn: &Connection, game: i64) -> Option<Turn> {
    turns_of(conn, game).pop()
}

/// Everything one turn changes, written as one thing.
pub struct Written<'a> {
    pub game: i64,
    /// The number of turns the caller read, so a turn that crossed with
    /// another does nothing at all.
    pub expect: i64,
    pub seat: usize,
    pub kind: &'a str,
    pub word: &'a str,
    pub score: i64,
    pub squares: String,
    /// The board afterwards; unchanged for a pass or an exchange.
    pub board: &'a str,
    pub bag: &'a str,
    pub rack: &'a str,
    /// Whose turn it is next.
    pub next: usize,
    /// Turns in a row that have scored nothing, afterwards.
    pub passes: i64,
    pub now: i64,
}

/// Writes a turn: the board, the bag, the player's rack and score, whose turn
/// it is next, and a line in the history. `false` when the game had already
/// moved on, which is how two presses landing together come to one turn.
pub fn take_turn(conn: &Connection, t: Written) -> rusqlite::Result<bool> {
    let rows = conn.execute(
        "UPDATE games SET board = ?2, bag = ?3, turn = ?4, passes = ?5, turns = turns + 1, turn_started_at = ?6
         WHERE id = ?1 AND status = 'running' AND turns = ?7",
        params![t.game, t.board, t.bag, t.next as i64, t.passes, t.now, t.expect],
    )?;
    if rows != 1 {
        return Ok(false);
    }
    conn.execute(
        "UPDATE players SET rack = ?3, score = score + ?4 WHERE game_id = ?1 AND seat = ?2",
        params![t.game, t.seat as i64, t.rack, t.score],
    )?;
    conn.execute(
        "INSERT INTO plays (game_id, seat, kind, word, score, squares, ts) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![t.game, t.seat as i64, t.kind, t.word, t.score, t.squares, t.now],
    )?;
    Ok(true)
}

/// Notes that a player let a turn run out. Returns how many they have missed.
pub fn miss_turn(conn: &Connection, game: i64, seat: usize) -> rusqlite::Result<i64> {
    conn.execute("UPDATE players SET misses = misses + 1 WHERE game_id = ?1 AND seat = ?2", params![game, seat as i64])?;
    conn.query_row("SELECT misses FROM players WHERE game_id = ?1 AND seat = ?2", params![game, seat as i64], |r| r.get(0))
}

/// Takes a player out of a game. Their tiles go back in the bag, because a
/// game the rest play on must not be short of letters.
pub fn drop_player(conn: &Connection, game: i64, seat: usize, bag: &str) -> rusqlite::Result<bool> {
    let rows = conn.execute(
        "UPDATE players SET dropped = 1, rack = '' WHERE game_id = ?1 AND seat = ?2 AND dropped = 0",
        params![game, seat as i64],
    )?;
    if rows != 1 {
        return Ok(false);
    }
    conn.execute("UPDATE games SET bag = ?2 WHERE id = ?1", params![game, bag])?;
    conn.execute(
        "INSERT INTO plays (game_id, seat, kind, word, score, squares, ts) VALUES (?1, ?2, 'dropped', '', 0, '', ?3)",
        params![game, seat as i64, chrono::Utc::now().timestamp()],
    )?;
    Ok(true)
}

/// Moves the turn on without anything else changing, for a seat that has just
/// been dropped.
pub fn hand_over(conn: &Connection, game: i64, next: usize, now: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE games SET turn = ?2, turn_started_at = ?3 WHERE id = ?1", params![game, next as i64, now]).map(|_| ())
}

/// Ends a game. Only the first call finds it running, so a last tile and a
/// clock running out can never both post a result.
pub fn finish_game(conn: &Connection, game: i64, result: &str, went_out: Option<usize>, now: i64) -> rusqlite::Result<bool> {
    let rows = conn.execute(
        "UPDATE games SET status = 'over', result = ?2, went_out = ?3, finished_at = ?4 WHERE id = ?1 AND status = 'running'",
        params![game, result, went_out.map(|s| s as i64), now],
    )?;
    Ok(rows == 1)
}

/// Writes what the leftover tiles did to one player's score.
pub fn set_adjust(conn: &Connection, game: i64, seat: usize, adjust: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE players SET adjust = ?3 WHERE game_id = ?1 AND seat = ?2", params![game, seat as i64, adjust])
        .map(|_| ())
}

/// Writes what one player earned: what the ledger paid, and what they were
/// worth before the daily limit — the duel points, which nothing caps.
pub fn record_points(conn: &Connection, game: i64, seat: usize, points: i64, worth: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE players SET points = ?3, worth = ?4 WHERE game_id = ?1 AND seat = ?2",
        params![game, seat as i64, points, worth],
    )
    .map(|_| ())
}

/// Marks a game settled, so it is never paid or announced twice.
pub fn record_payout(conn: &Connection, game: i64, why_nothing: &str) -> rusqlite::Result<()> {
    conn.execute("UPDATE games SET paid_out = 1, why_nothing = ?2 WHERE id = ?1", params![game, why_nothing]).map(|_| ())
}

/// Hands every running game the time the bot was away, so nobody's turn runs
/// out while the machine was off.
pub fn give_time_back(conn: &Connection, seconds: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE games SET turn_started_at = turn_started_at + ?1, given_back = given_back + ?1 WHERE status = 'running'",
        params![seconds],
    )
}

// --- the private board links ---------------------------------------------------------------

const TOKEN_ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyz23456789";
pub const TOKEN_LEN: usize = 22;

fn make_token(roll: &mut impl FnMut() -> f64) -> String {
    (0..TOKEN_LEN)
        .map(|_| {
            let n = (roll().clamp(0.0, 0.999_999) * TOKEN_ALPHABET.len() as f64) as usize;
            TOKEN_ALPHABET[n.min(TOKEN_ALPHABET.len() - 1)] as char
        })
        .collect()
}

/// One player's link for one game, made the first time it is asked for and the
/// same one every time after.
pub fn token_for(conn: &Connection, game: i64, user: u64, now: i64, mut roll: impl FnMut() -> f64) -> Option<String> {
    if let Some(token) = conn
        .query_row("SELECT token FROM tokens WHERE game_id = ?1 AND user_id = ?2", params![game, user as i64], |r| r.get(0))
        .optional()
        .ok()
        .flatten()
    {
        return Some(token);
    }
    let token = make_token(&mut roll);
    conn.execute(
        "INSERT INTO tokens (token, game_id, user_id, made_at) VALUES (?1, ?2, ?3, ?4)",
        params![token, game, user as i64, now],
    )
    .ok()?;
    Some(token)
}

/// Whose link this is, and for which game.
pub fn token_owner(conn: &Connection, token: &str) -> Option<(i64, u64)> {
    conn.query_row("SELECT game_id, user_id FROM tokens WHERE token = ?1", params![token], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? as u64))
    })
    .optional()
    .ok()
    .flatten()
}

pub fn drop_tokens(conn: &Connection, game: i64) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM tokens WHERE game_id = ?1", params![game]).map(|_| ())
}

// --- the duel points board ---------------------------------------------------------------

/// One person's duel points over a stretch of days.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tally {
    pub user: u64,
    pub points: i64,
    pub games: i64,
    /// The best single game they had, for the tie-break and the line.
    pub best: i64,
}

/// The board between two days, ranked: most points first, then most games, then
/// the best game. Only finished games count.
pub fn tally_between(conn: &Connection, from: &str, to: &str) -> Vec<Tally> {
    conn.prepare(
        "SELECT p.user_id, COALESCE(SUM(p.worth), 0), COUNT(*), COALESCE(MAX(p.score + p.adjust), 0)
         FROM players p JOIN games g ON g.id = p.game_id
         WHERE g.status = 'over' AND g.day >= ?1 AND g.day <= ?2
         GROUP BY p.user_id",
    )
    .and_then(|mut q| {
        q.query_map(params![from, to], |r| {
            Ok(Tally { user: r.get::<_, i64>(0)? as u64, points: r.get(1)?, games: r.get(2)?, best: r.get(3)? })
        })
        .map(|rows| rows.flatten().collect::<Vec<Tally>>())
    })
    .map(|mut rows| {
        rows.sort_by(|a, b| b.points.cmp(&a.points).then(b.games.cmp(&a.games)).then(b.best.cmp(&a.best)).then(a.user.cmp(&b.user)));
        rows
    })
    .unwrap_or_default()
}

/// One day's board.
pub fn day_tally(conn: &Connection, day: &str) -> Vec<Tally> {
    tally_between(conn, day, day)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn memory() -> Connection {
        let conn = Connection::open_in_memory().expect("a database");
        init(&conn).expect("the schema");
        conn
    }

    fn three(conn: &Connection) -> Game {
        let seats = [
            (1u64, "ravenclaw".to_string(), "CATSDOG".to_string()),
            (2u64, "gryffindor".to_string(), "HATPINS".to_string()),
            (3u64, String::new(), "WORDIES".to_string()),
        ];
        start_game(conn, 55, &seats, &super::super::duel_rules::empty_board(), "ABCDEFGH", 90, 1_000, "2026-09-17").expect("a game")
    }

    #[test]
    fn a_game_keeps_its_seats_racks_and_turn() {
        let conn = memory();
        let game = three(&conn);
        assert!(game.running());
        assert_eq!(game.players.len(), 3);
        assert_eq!(game.turn, 0);
        assert_eq!(game.to_move(), Some(1));
        assert_eq!(game.seat_of(2), Some(1));
        assert_eq!(game.seat_of(9), None);
        assert!(game.has(3) && !game.has(9));
        assert_eq!(game.player(2).map(|p| p.rack.as_str()), Some("WORDIES"));
        assert_eq!(game.bag_left(), 8);
        assert_eq!(game.next_seat(0), Some(1));
        assert_eq!(game.next_seat(2), Some(0), "the turn comes round again");
    }

    #[test]
    fn two_presses_landing_together_take_one_turn() {
        let conn = memory();
        let game = three(&conn);
        let write = |expect: i64| Written {
            game: game.id,
            expect,
            seat: 0,
            kind: "word",
            word: "CAT",
            score: 10,
            squares: "112,113,114".into(),
            board: "x",
            bag: "ABCDE",
            rack: "SDOG",
            next: 1,
            passes: 0,
            now: 1_100,
        };
        assert!(take_turn(&conn, write(0)).unwrap(), "the first press lands");
        assert!(!take_turn(&conn, write(0)).unwrap(), "the second press finds the game has moved on");
        let after = get_game(&conn, game.id).expect("still there");
        assert_eq!(after.turns, 1);
        assert_eq!(after.turn, 1);
        assert_eq!(after.player(0).map(|p| p.score), Some(10));
        assert_eq!(after.player(0).map(|p| p.rack.as_str()), Some("SDOG"));
        assert_eq!(turns_of(&conn, game.id).len(), 1);
        assert_eq!(last_turn(&conn, game.id).map(|t| t.covered()), Some(vec![112, 113, 114]));
    }

    #[test]
    fn a_player_who_keeps_missing_is_taken_out_and_the_turn_moves_on() {
        let conn = memory();
        let game = three(&conn);
        assert_eq!(miss_turn(&conn, game.id, 1).unwrap(), 1);
        assert_eq!(miss_turn(&conn, game.id, 1).unwrap(), 2);
        assert_eq!(miss_turn(&conn, game.id, 1).unwrap(), 3);
        assert!(drop_player(&conn, game.id, 1, "ABCDEFGHHATPINS").unwrap());
        assert!(!drop_player(&conn, game.id, 1, "ABCDEFGH").unwrap(), "only once");
        let after = get_game(&conn, game.id).expect("still there");
        assert!(after.player(1).map(|p| p.dropped).unwrap_or(false));
        assert_eq!(after.player(1).map(|p| p.rack.as_str()), Some(""), "their tiles went back");
        assert_eq!(after.bag_left(), 15);
        assert_eq!(after.playing(), vec![0, 2]);
        assert_eq!(after.next_seat(0), Some(2), "the empty seat is stepped over");
        assert_eq!(after.next_seat(2), Some(0));
    }

    #[test]
    fn a_game_is_only_finished_once() {
        let conn = memory();
        let game = three(&conn);
        assert!(finish_game(&conn, game.id, "played out", Some(0), 2_000).unwrap());
        assert!(!finish_game(&conn, game.id, "passed out", None, 2_100).unwrap(), "the second call does nothing");
        let after = get_game(&conn, game.id).expect("still there");
        assert!(!after.running());
        assert_eq!(after.result.as_deref(), Some("played out"));
        assert_eq!(after.went_out, Some(0));
        assert_eq!(after.finished_at, Some(2_000));
        assert!(!after.paid_out);
        assert_eq!(unfinished_business(&conn).len(), 1);
        record_payout(&conn, game.id, "").unwrap();
        assert!(unfinished_business(&conn).is_empty());
    }

    #[test]
    fn a_link_belongs_to_one_player_of_one_game_and_dies_with_it() {
        let conn = memory();
        let game = three(&conn);
        let mut n: f64 = 0.0;
        let mut roll = || {
            n += 0.061;
            n.fract()
        };
        let first = token_for(&conn, game.id, 1, 1_000, &mut roll).expect("a link");
        let second = token_for(&conn, game.id, 2, 1_000, &mut roll).expect("a link");
        assert_ne!(first, second);
        assert_eq!(first.chars().count(), TOKEN_LEN);
        assert_eq!(token_for(&conn, game.id, 1, 1_050, &mut roll).as_deref(), Some(first.as_str()), "the same link twice");
        assert_eq!(token_owner(&conn, &first), Some((game.id, 1)));
        assert_eq!(token_owner(&conn, "nonsense"), None);
        drop_tokens(&conn, game.id).unwrap();
        assert_eq!(token_owner(&conn, &first), None);
    }

    #[test]
    fn a_restart_hands_back_the_time_it_was_away() {
        let conn = memory();
        let game = three(&conn);
        assert_eq!(give_time_back(&conn, 600).unwrap(), 1);
        let after = get_game(&conn, game.id).expect("still there");
        assert_eq!(after.turn_started_at, 1_600);
        assert_eq!(after.given_back, 600);
        // A finished game gets nothing.
        finish_game(&conn, game.id, "played out", None, 2_000).unwrap();
        assert_eq!(give_time_back(&conn, 600).unwrap(), 0);
    }

    // --- the lobby ----------------------------------------------------------------

    #[test]
    fn a_lobby_fills_up_and_only_closes_once() {
        let conn = memory();
        let lobby = open_lobby(&conn, 55, 1, 1_000, 1_120).expect("a lobby");
        assert_eq!(lobby.players(), 0);
        assert!(join_lobby(&conn, lobby.id, 1, "ravenclaw", 1_001, 4).unwrap());
        assert!(!join_lobby(&conn, lobby.id, 1, "ravenclaw", 1_002, 4).unwrap(), "nobody joins twice");
        assert!(join_lobby(&conn, lobby.id, 2, "", 1_003, 4).unwrap());
        assert!(join_lobby(&conn, lobby.id, 3, "", 1_004, 4).unwrap());
        assert!(!join_lobby(&conn, lobby.id, 9, "", 1_005, 3).unwrap(), "a full lobby takes nobody else");
        let full = get_lobby(&conn, lobby.id).expect("still there");
        assert_eq!(full.seats.iter().map(|s| s.user).collect::<Vec<_>>(), vec![1, 2, 3], "in the order they joined");
        assert!(full.has(2) && !full.has(9));
        assert_eq!(live_lobby(&conn, 55).map(|l| l.id), Some(lobby.id));
        assert_eq!(open_lobbies(&conn).len(), 1);

        assert!(leave_lobby(&conn, lobby.id, 3).unwrap());
        assert!(!leave_lobby(&conn, lobby.id, 3).unwrap());
        assert_eq!(get_lobby(&conn, lobby.id).map(|l| l.players()), Some(2));

        assert!(close_lobby(&conn, lobby.id, LobbyStatus::Started, None).unwrap());
        assert!(!close_lobby(&conn, lobby.id, LobbyStatus::CalledOff, None).unwrap(), "a lobby closes once");
        // The game is made after the lobby is claimed, so its number is
        // written on afterwards.
        set_lobby_game(&conn, lobby.id, 7).unwrap();
        let closed = get_lobby(&conn, lobby.id).expect("still there");
        assert_eq!(closed.status, LobbyStatus::Started);
        assert_eq!(closed.game_id, Some(7));
        assert!(live_lobby(&conn, 55).is_none());
        assert!(!join_lobby(&conn, lobby.id, 9, "", 1_100, 4).unwrap(), "a closed lobby takes nobody");
    }

    #[test]
    fn a_lobby_is_given_the_time_back_too() {
        let conn = memory();
        let lobby = open_lobby(&conn, 55, 1, 1_000, 1_120).expect("a lobby");
        assert_eq!(give_lobbies_time_back(&conn, 300).unwrap(), 1);
        assert_eq!(get_lobby(&conn, lobby.id).map(|l| l.closes_at), Some(1_420));
    }

    // --- the points board ---------------------------------------------------------

    #[test]
    fn the_duel_points_board_counts_what_people_were_worth_not_what_was_paid() {
        let conn = memory();
        let game = three(&conn);
        finish_game(&conn, game.id, "played out", Some(0), 2_000).unwrap();
        // The ledger paid the first player nothing (their limit was full) but
        // they were worth four, and that is what the board counts.
        record_points(&conn, game.id, 0, 0, 4).unwrap();
        record_points(&conn, game.id, 1, 2, 2).unwrap();
        record_points(&conn, game.id, 2, 1, 1).unwrap();
        set_adjust(&conn, game.id, 0, 12).unwrap();
        let board = day_tally(&conn, "2026-09-17");
        assert_eq!(board.iter().map(|t| (t.user, t.points)).collect::<Vec<_>>(), vec![(1, 4), (2, 2), (3, 1)]);
        assert_eq!(board[0].games, 1);
        assert_eq!(board[0].best, 12, "what they finished the game on");
        assert!(day_tally(&conn, "2026-09-18").is_empty());
        assert_eq!(tally_between(&conn, "2026-09-01", "2026-09-31").len(), 3);
    }

    #[test]
    fn a_game_still_running_is_on_nobodys_board() {
        let conn = memory();
        let game = three(&conn);
        record_points(&conn, game.id, 0, 4, 4).unwrap();
        assert!(day_tally(&conn, "2026-09-17").is_empty(), "only a finished game counts");
        assert_eq!(live_game(&conn, 55).map(|g| g.id), Some(game.id));
        assert_eq!(running_games(&conn).len(), 1);
    }
}
