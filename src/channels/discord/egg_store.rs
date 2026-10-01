//! What the egg week keeps, in `.runtime/eggs.db`: one row per egg, the hunger
//! each egg has been assigned slot by slot, and any override a mod has typed.
//!
//! Three things make this a store rather than a calculation.
//!
//! **A seat.** Every egg gets a seat number when it is claimed, in the order
//! they are claimed. The seat is what the rotation is computed from (see
//! [`super::egg::pick`]), so it has to be the same tomorrow as it is today -
//! and it has to stay the same when somebody else leaves. A seat is never
//! reused and never renumbered.
//!
//! **A written-down hunger.** The rotation is a pure function of seat and slot,
//! so it could be recomputed every time. It is written down anyway, the first
//! time each (member, slot) is asked for, because it is what decides whether
//! somebody's points counted: if the craving count is ever changed mid-week the
//! answer to "what was I hungry for at 3pm on Tuesday" must not change with it.
//!
//! **The hatch.** A house and a dragon name, written once, at the hatch. The
//! dragon name is unique across the server by a unique index, not by hoping.

use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

pub const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS eggs (
        user_id INTEGER PRIMARY KEY,
        seat INTEGER NOT NULL,
        name TEXT NOT NULL DEFAULT '',
        claimed_ts INTEGER NOT NULL,
        hatch_ts INTEGER NOT NULL,
        house TEXT NOT NULL DEFAULT '',
        dragon TEXT NOT NULL DEFAULT '',
        hatched_ts INTEGER,
        frozen_ts INTEGER);
    CREATE UNIQUE INDEX IF NOT EXISTS eggs_seat ON eggs (seat);
    CREATE UNIQUE INDEX IF NOT EXISTS eggs_dragon ON eggs (dragon) WHERE dragon != '';
    CREATE TABLE IF NOT EXISTS cravings (
        user_id INTEGER NOT NULL, slot INTEGER NOT NULL, games TEXT NOT NULL,
        PRIMARY KEY (user_id, slot));
    CREATE TABLE IF NOT EXISTS overrides (
        slot INTEGER PRIMARY KEY, games TEXT NOT NULL, by_user INTEGER NOT NULL, ts INTEGER NOT NULL);
    CREATE TABLE IF NOT EXISTS toggles (user_id INTEGER PRIMARY KEY, ts INTEGER NOT NULL);
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);";

/// Added after the first release. Run on every start and allowed to fail: the
/// only way it fails is that the column is already there.
const LATER: &[&str] = &["ALTER TABLE eggs ADD COLUMN frozen_ts INTEGER"];

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

pub fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)?;
    for step in LATER {
        let _ = conn.execute(step, []);
    }
    Ok(())
}

pub fn open(workspace: &str) -> anyhow::Result<()> {
    if DB.get().is_some() {
        return Ok(());
    }
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    std::fs::create_dir_all(&dir)?;
    open_at(&dir.join("eggs.db"))
}

pub fn open_at(path: &Path) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
    init(&conn)?;
    let _ = DB.set(Mutex::new(conn));
    Ok(())
}

/// An in-memory store, for tests.
pub fn memory() -> Connection {
    let conn = Connection::open_in_memory().expect("memory db");
    init(&conn).expect("egg schema");
    conn
}

pub fn db() -> Option<&'static Mutex<Connection>> {
    DB.get()
}

// --- the eggs -------------------------------------------------------------------

/// One member's egg, before and after it opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Egg {
    pub user: u64,
    /// Where they sit in the rotation. Stable for the whole month.
    pub seat: i64,
    pub name: String,
    pub claimed_ts: i64,
    /// When this egg opens. Everybody who was here at the start shares one
    /// moment; a late claim gets its own, a week after the claim.
    pub hatch_ts: i64,
    /// Empty until the hatch.
    pub house: String,
    pub dragon: String,
    pub hatched_ts: Option<i64>,
    /// When they opted out, if they are out. Frozen, never deleted: the egg,
    /// the dragon, the house, the warmth and the cards are all still here, and
    /// opting back in resumes exactly where they left off.
    pub frozen_ts: Option<i64>,
}

impl Egg {
    /// True while the egg is still an egg, at `now`.
    pub fn waiting(&self, now: i64) -> bool {
        self.hatched_ts.is_none() && now < self.hatch_ts
    }

    /// True while they are opted out. Out means out: no pings, nothing on the
    /// hall, no growth and no points towards the month - and nothing lost.
    pub fn frozen(&self) -> bool {
        self.frozen_ts.is_some()
    }

    /// True once it has actually been opened by a hatch - not merely due.
    pub fn open(&self) -> bool {
        self.hatched_ts.is_some()
    }
}

const COLUMNS: &str = "user_id, seat, name, claimed_ts, hatch_ts, house, dragon, hatched_ts, frozen_ts";

fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Egg> {
    Ok(Egg {
        user: row.get::<_, i64>(0)? as u64,
        seat: row.get(1)?,
        name: row.get(2)?,
        claimed_ts: row.get(3)?,
        hatch_ts: row.get(4)?,
        house: row.get(5)?,
        dragon: row.get(6)?,
        hatched_ts: row.get(7)?,
        frozen_ts: row.get(8)?,
    })
}

/// The next free seat. Seats are handed out in order and never reused, so two
/// members claiming at the same moment can't share one.
pub fn next_seat(conn: &Connection) -> i64 {
    conn.query_row("SELECT COALESCE(MAX(seat), -1) + 1 FROM eggs", [], |r| r.get(0)).unwrap_or(0)
}

/// Gives somebody an egg, or leaves the one they have exactly as it is.
///
/// Claiming twice is not an error and must never move a hatch: the second call
/// is how a member who presses something again is handled, and how a restart
/// that re-reads the sign-up sheet is handled.
pub fn claim(conn: &Connection, user: u64, name: &str, hatch_ts: i64, now: i64) -> rusqlite::Result<Egg> {
    if let Some(found) = egg(conn, user)? {
        if !name.is_empty() && name != found.name {
            conn.execute("UPDATE eggs SET name = ?2 WHERE user_id = ?1", params![user as i64, name])?;
            return Ok(Egg { name: name.to_string(), ..found });
        }
        return Ok(found);
    }
    let seat = next_seat(conn);
    conn.execute(
        "INSERT INTO eggs (user_id, seat, name, claimed_ts, hatch_ts) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![user as i64, seat, name, now, hatch_ts],
    )?;
    Ok(Egg {
        user,
        seat,
        name: name.to_string(),
        claimed_ts: now,
        hatch_ts,
        house: String::new(),
        dragon: String::new(),
        hatched_ts: None,
        frozen_ts: None,
    })
}

pub fn egg(conn: &Connection, user: u64) -> rusqlite::Result<Option<Egg>> {
    conn.query_row(&format!("SELECT {} FROM eggs WHERE user_id = ?1", COLUMNS), params![user as i64], read).optional()
}

/// Every egg, in seat order.
/// Opting out: a date on the row and nothing else. Nothing is deleted, nothing
/// is forgotten, and the house is not given up.
pub fn freeze(conn: &Connection, user: u64, now: i64) -> rusqlite::Result<bool> {
    let changed = conn.execute(
        "UPDATE eggs SET frozen_ts = ?2 WHERE user_id = ?1 AND frozen_ts IS NULL",
        params![user as i64, now],
    )?;
    Ok(changed > 0)
}

/// Opting back in: the date comes off and they resume. The house, the dragon,
/// the warmth and the cards were never touched, so there is nothing to restore.
pub fn thaw(conn: &Connection, user: u64) -> rusqlite::Result<bool> {
    let changed =
        conn.execute("UPDATE eggs SET frozen_ts = NULL WHERE user_id = ?1 AND frozen_ts IS NOT NULL", params![user as i64])?;
    Ok(changed > 0)
}

/// When this member last pressed either button, if they ever have.
pub fn toggled_at(conn: &Connection, user: u64) -> Option<i64> {
    conn.query_row("SELECT ts FROM toggles WHERE user_id = ?1", params![user as i64], |r| r.get(0)).optional().ok().flatten()
}

pub fn note_toggle(conn: &Connection, user: u64, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO toggles (user_id, ts) VALUES (?1, ?2) ON CONFLICT(user_id) DO UPDATE SET ts = excluded.ts",
        params![user as i64, now],
    )
    .map(|_| ())
}

pub fn all(conn: &Connection) -> Vec<Egg> {
    conn.prepare(&format!("SELECT {} FROM eggs ORDER BY seat", COLUMNS))
        .and_then(|mut s| s.query_map([], read)?.collect())
        .unwrap_or_default()
}

/// Everyone whose egg has not been opened yet. Somebody who is opted out is
/// left out: the hatch does not deal a house to a member who is not playing.
pub fn unhatched(conn: &Connection) -> Vec<Egg> {
    all(conn).into_iter().filter(|e| !e.open() && !e.frozen()).collect()
}

/// Everybody in the month right now: an egg, and not opted out. What the hall,
/// the leaderboard and the hourly post are drawn from.
pub fn playing(conn: &Connection) -> Vec<Egg> {
    all(conn).into_iter().filter(|e| !e.frozen()).collect()
}

/// Dragon names already taken, so a second hatch can't hand one out twice.
pub fn dragons(conn: &Connection) -> HashSet<String> {
    all(conn).into_iter().map(|e| e.dragon).filter(|d| !d.is_empty()).collect()
}

/// Opens one egg: a house, a dragon, and the moment it happened. Refuses to
/// touch an egg that has already been opened, so a second `/hatch` is a no-op
/// rather than a re-roll.
pub fn hatch(conn: &Connection, user: u64, house: &str, dragon: &str, now: i64) -> rusqlite::Result<bool> {
    let changed = conn.execute(
        "UPDATE eggs SET house = ?2, dragon = ?3, hatched_ts = ?4 WHERE user_id = ?1 AND hatched_ts IS NULL",
        params![user as i64, house, dragon, now],
    )?;
    Ok(changed > 0)
}

/// Whether the month's one big hatch has already been run.
pub fn hatched_at(conn: &Connection) -> Option<i64> {
    meta_get(conn, "hatched_at").and_then(|v| v.parse().ok())
}

pub fn mark_hatched(conn: &Connection, now: i64) -> rusqlite::Result<()> {
    meta_set(conn, "hatched_at", &now.to_string())
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

// --- the hunger ------------------------------------------------------------------

/// What one member was hungry for in one slot, if it has been written down.
pub fn craving(conn: &Connection, user: u64, slot: i64) -> Option<Vec<String>> {
    conn.query_row(
        "SELECT games FROM cravings WHERE user_id = ?1 AND slot = ?2",
        params![user as i64, slot],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
    .map(|raw| raw.split(',').filter(|p| !p.is_empty()).map(String::from).collect())
}

/// Writes one slot's hunger down, the first time it is asked for. Writing it
/// again never changes it: what somebody was hungry for at the time is what
/// their points were judged against, and that can't be rewritten later.
pub fn set_craving(conn: &Connection, user: u64, slot: i64, games: &[&str]) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO cravings (user_id, slot, games) VALUES (?1, ?2, ?3)",
        params![user as i64, slot, games.join(",")],
    )
    .map(|_| ())
}

/// A mod's override for one slot: everybody is hungry for these instead.
pub fn override_for(conn: &Connection, slot: i64) -> Option<Vec<String>> {
    conn.query_row("SELECT games FROM overrides WHERE slot = ?1", params![slot], |r| r.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
        .map(|raw| raw.split(',').filter(|p| !p.is_empty()).map(String::from).collect())
}

pub fn set_override(conn: &Connection, slot: i64, games: &[&str], by: u64, now: i64) -> rusqlite::Result<()> {
    if games.is_empty() {
        conn.execute("DELETE FROM overrides WHERE slot = ?1", params![slot])?;
        return Ok(());
    }
    conn.execute(
        "INSERT INTO overrides (slot, games, by_user, ts) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(slot) DO UPDATE SET games = excluded.games, by_user = excluded.by_user, ts = excluded.ts",
        params![slot, games.join(","), by as i64, now],
    )?;
    // An override replaces what was written down for that slot, for everybody:
    // the written row is the record of what counted, so it has to follow.
    conn.execute("DELETE FROM cravings WHERE slot = ?1", params![slot])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seat_is_handed_out_once_and_never_moves() {
        let conn = memory();
        let a = claim(&conn, 1, "Zoya", 500, 100).unwrap();
        let b = claim(&conn, 2, "Kabir", 500, 101).unwrap();
        assert_eq!((a.seat, b.seat), (0, 1));
        // Claiming again gives back the same egg - the same seat, the same hatch.
        let again = claim(&conn, 1, "Zoya", 9_999, 900).unwrap();
        assert_eq!((again.seat, again.hatch_ts, again.claimed_ts), (0, 500, 100), "a second claim moves nothing");
        // A late arrival gets the next seat, not a recycled one.
        assert_eq!(claim(&conn, 3, "Ira", 1_000, 200).unwrap().seat, 2);
        assert_eq!(next_seat(&conn), 3);
        // A changed display name follows, and nothing else does.
        let renamed = claim(&conn, 1, "Zo", 9_999, 900).unwrap();
        assert_eq!((renamed.name.as_str(), renamed.seat, renamed.hatch_ts), ("Zo", 0, 500));
    }

    #[test]
    fn an_egg_opens_once_and_a_dragon_name_is_never_handed_out_twice() {
        let conn = memory();
        claim(&conn, 1, "Zoya", 500, 100).unwrap();
        claim(&conn, 2, "Kabir", 500, 100).unwrap();
        assert!(egg(&conn, 1).unwrap().unwrap().waiting(400));
        assert!(hatch(&conn, 1, "gryffindor", "Vhagaryx", 500).unwrap());
        let opened = egg(&conn, 1).unwrap().unwrap();
        assert!(opened.open() && !opened.waiting(600));
        assert_eq!((opened.house.as_str(), opened.dragon.as_str()), ("gryffindor", "Vhagaryx"));
        // A second hatch leaves it exactly as it is.
        assert!(!hatch(&conn, 1, "slytherin", "Something Else", 900).unwrap());
        assert_eq!(egg(&conn, 1).unwrap().unwrap().house, "gryffindor");
        // And the same dragon can't be given to somebody else.
        assert!(hatch(&conn, 2, "slytherin", "Vhagaryx", 500).is_err(), "one dragon, one rider");
        assert_eq!(dragons(&conn), HashSet::from(["Vhagaryx".to_string()]));
        assert_eq!(unhatched(&conn).len(), 1);
    }

    #[test]
    fn a_written_hunger_is_the_record_and_an_override_replaces_the_slot() {
        let conn = memory();
        set_craving(&conn, 7, 42, &["anagram", "quiz"]).unwrap();
        assert_eq!(craving(&conn, 7, 42), Some(vec!["anagram".into(), "quiz".into()]));
        // Writing again does not rewrite history.
        set_craving(&conn, 7, 42, &["geo", "cat"]).unwrap();
        assert_eq!(craving(&conn, 7, 42), Some(vec!["anagram".into(), "quiz".into()]));
        assert_eq!(craving(&conn, 7, 43), None);
        assert_eq!(override_for(&conn, 42), None);
        set_override(&conn, 42, &["movie"], 99, 1_000).unwrap();
        assert_eq!(override_for(&conn, 42), Some(vec!["movie".into()]));
        assert_eq!(craving(&conn, 7, 42), None, "the override clears what the slot had written down");
        set_override(&conn, 42, &[], 99, 1_100).unwrap();
        assert_eq!(override_for(&conn, 42), None, "an empty override is no override");
    }

    #[test]
    fn the_whole_thing_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/eggs.db");
        {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let conn = Connection::open(&path).unwrap();
            init(&conn).unwrap();
            claim(&conn, 11, "Ira", 777, 1).unwrap();
            set_craving(&conn, 11, 3, &["koto"]).unwrap();
            mark_hatched(&conn, 777).unwrap();
        }
        let conn = Connection::open(&path).unwrap();
        init(&conn).unwrap();
        assert_eq!(egg(&conn, 11).unwrap().map(|e| (e.seat, e.hatch_ts)), Some((0, 777)));
        assert_eq!(craving(&conn, 11, 3), Some(vec!["koto".into()]));
        assert_eq!(hatched_at(&conn), Some(777));
    }
}

#[cfg(test)]
mod freeze_tests {
    use super::*;

    #[test]
    fn opting_out_freezes_everything_and_opting_back_in_resumes_it() {
        let conn = memory();
        claim(&conn, 1, "Zoya", 500, 100).unwrap();
        hatch(&conn, 1, "gryffindor", "Vhagaryx", 500).unwrap();
        let before = egg(&conn, 1).unwrap().unwrap();
        assert!(!before.frozen());

        assert!(freeze(&conn, 1, 900).unwrap());
        let out = egg(&conn, 1).unwrap().unwrap();
        assert!(out.frozen() && out.frozen_ts == Some(900));
        // Nothing was given up: the egg, the house, the dragon and the seat are
        // all exactly as they were.
        assert_eq!(
            (out.seat, out.house.as_str(), out.dragon.as_str(), out.hatched_ts, out.claimed_ts),
            (before.seat, before.house.as_str(), before.dragon.as_str(), before.hatched_ts, before.claimed_ts)
        );
        assert!(!freeze(&conn, 1, 1_000).unwrap(), "freezing twice changes nothing");
        assert_eq!(egg(&conn, 1).unwrap().unwrap().frozen_ts, Some(900), "and does not move the date");

        // While out they are nobody the month counts.
        assert!(playing(&conn).is_empty());
        assert!(unhatched(&conn).is_empty());

        assert!(thaw(&conn, 1).unwrap());
        let back = egg(&conn, 1).unwrap().unwrap();
        assert!(!back.frozen());
        assert_eq!(back, before, "they resume exactly what they left - they do not restart");
        assert!(!thaw(&conn, 1).unwrap(), "thawing twice changes nothing");
        assert_eq!(playing(&conn).len(), 1);
    }

    #[test]
    fn an_unhatched_member_who_is_out_is_not_dealt_a_house() {
        let conn = memory();
        claim(&conn, 1, "Zoya", 500, 100).unwrap();
        claim(&conn, 2, "Kabir", 500, 100).unwrap();
        freeze(&conn, 2, 200).unwrap();
        assert_eq!(unhatched(&conn).iter().map(|e| e.user).collect::<Vec<_>>(), vec![1]);
        assert_eq!(playing(&conn).iter().map(|e| e.user).collect::<Vec<_>>(), vec![1]);
        assert_eq!(all(&conn).len(), 2, "but they are still on the books");
    }

    #[test]
    fn the_last_press_is_remembered_so_the_toggle_can_be_slowed_down() {
        let conn = memory();
        assert_eq!(toggled_at(&conn, 7), None);
        note_toggle(&conn, 7, 1_000).unwrap();
        assert_eq!(toggled_at(&conn, 7), Some(1_000));
        note_toggle(&conn, 7, 5_000).unwrap();
        assert_eq!(toggled_at(&conn, 7), Some(5_000), "the newest press is the one that counts");
        assert_eq!(toggled_at(&conn, 8), None, "and it is per member");
    }
}
