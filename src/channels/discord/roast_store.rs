//! `/ship`'s own small store, `.runtime/roast.db`: the last percentage a pair
//! scored and when, so the card can say which way it has moved since.
//!
//! That is all it keeps. The opt-out lives in the panel's settings
//! (`VIZIER_ROAST_OPTOUTS`), and nothing a model wrote is ever stored.

use std::sync::OnceLock;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

pub const SCHEMA: &str = "
    -- One row per pair, the lower id first, so a pair is the same pair
    -- whichever way round it was asked for.
    CREATE TABLE IF NOT EXISTS ship_scores (
        lo INTEGER NOT NULL, hi INTEGER NOT NULL, percent INTEGER NOT NULL, ts INTEGER NOT NULL,
        PRIMARY KEY (lo, hi)) WITHOUT ROWID;";

pub fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)
}

pub fn open(workspace: &str) -> anyhow::Result<()> {
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    std::fs::create_dir_all(&dir)?;
    let conn = Connection::open(dir.join("roast.db"))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
    init(&conn)?;
    let _ = DB.set(Mutex::new(conn));
    Ok(())
}

pub fn db() -> Option<&'static Mutex<Connection>> {
    DB.get()
}

/// An in-memory store, for the tests.
pub fn memory() -> Connection {
    let conn = Connection::open_in_memory().expect("in-memory sqlite");
    init(&conn).expect("schema");
    conn
}

/// The pair, lower id first.
pub fn key(a: u64, b: u64) -> (u64, u64) {
    (a.min(b), a.max(b))
}

/// What this pair scored last time, and when.
pub fn last(conn: &Connection, a: u64, b: u64) -> Option<(u8, i64)> {
    let (lo, hi) = key(a, b);
    conn.query_row("SELECT percent, ts FROM ship_scores WHERE lo = ?1 AND hi = ?2", params![lo as i64, hi as i64], |r| {
        Ok((r.get::<_, i64>(0)?.clamp(0, 100) as u8, r.get::<_, i64>(1)?))
    })
    .optional()
    .ok()
    .flatten()
}

/// Remembers what they scored now.
pub fn record(conn: &Connection, a: u64, b: u64, percent: u8, ts: i64) -> rusqlite::Result<()> {
    let (lo, hi) = key(a, b);
    conn.execute(
        "INSERT INTO ship_scores (lo, hi, percent, ts) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(lo, hi) DO UPDATE SET percent = excluded.percent, ts = excluded.ts",
        params![lo as i64, hi as i64, percent as i64, ts],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pair_is_the_same_pair_whichever_way_round_it_is_asked() {
        let conn = memory();
        assert_eq!(key(9, 4), key(4, 9));
        assert!(last(&conn, 4, 9).is_none(), "nothing scored yet");
        record(&conn, 4, 9, 61, 1_000).unwrap();
        assert_eq!(last(&conn, 4, 9), Some((61, 1_000)));
        assert_eq!(last(&conn, 9, 4), Some((61, 1_000)), "and the other way round");
        // Scoring again replaces it rather than piling up.
        record(&conn, 9, 4, 55, 2_000).unwrap();
        assert_eq!(last(&conn, 4, 9), Some((55, 2_000)));
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM ship_scores", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
    }
}
