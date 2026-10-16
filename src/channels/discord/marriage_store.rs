//! Who is married to whom, who adopted whom, and what they said about it.
//!
//! One file beside the others in `.runtime`, because none of this belongs in
//! the house ledger: a marriage pays nothing and must never be able to.
//!
//! THE SHAPE. A marriage is a row with the two ids SORTED, so "are these two
//! married" is one lookup rather than two and the same pair can never be
//! written twice. A parent-child link is a row with a direction. Both carry
//! when they happened, which is what the anniversary reads and what lets a
//! card say "married 4 days".
//!
//! Multiple spouses are allowed - the owner's call - so nothing here is
//! one-per-person. What IS refused is a loop: a cycle check, because somebody
//! will try to adopt their own great-grandparent within the hour.
use std::collections::{HashMap, HashSet};

use parking_lot::Mutex;
use rusqlite::{params, Connection};

static DB: std::sync::OnceLock<Mutex<Connection>> = std::sync::OnceLock::new();

pub fn db() -> Option<&'static Mutex<Connection>> {
    DB.get()
}

pub const SCHEMA: &str = "
    PRAGMA journal_mode=WAL;
    PRAGMA busy_timeout=5000;
    CREATE TABLE IF NOT EXISTS unions (
        low INTEGER NOT NULL, high INTEGER NOT NULL, since INTEGER NOT NULL,
        vows TEXT NOT NULL DEFAULT '', PRIMARY KEY (low, high));
    CREATE TABLE IF NOT EXISTS kin (
        parent INTEGER NOT NULL, child INTEGER NOT NULL, since INTEGER NOT NULL,
        PRIMARY KEY (parent, child));
    CREATE INDEX IF NOT EXISTS kin_child ON kin (child);
    CREATE TABLE IF NOT EXISTS gestures (
        id INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL, who INTEGER NOT NULL,
        whom INTEGER, ts INTEGER NOT NULL);
    CREATE INDEX IF NOT EXISTS gestures_who ON gestures (kind, who, ts);
    CREATE TABLE IF NOT EXISTS blocks (
        who INTEGER NOT NULL, blocked INTEGER NOT NULL, ts INTEGER NOT NULL,
        PRIMARY KEY (who, blocked));
    CREATE TABLE IF NOT EXISTS past (
        id INTEGER PRIMARY KEY AUTOINCREMENT, low INTEGER NOT NULL, high INTEGER NOT NULL,
        since INTEGER NOT NULL, until INTEGER NOT NULL, ended_by INTEGER NOT NULL);
";

pub fn open(workspace: &str) -> anyhow::Result<()> {
    if DB.get().is_some() {
        return Ok(());
    }
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    std::fs::create_dir_all(&dir)?;
    let conn = Connection::open(dir.join("marriage.db"))?;
    conn.execute_batch(SCHEMA)?;
    let _ = DB.set(Mutex::new(conn));
    Ok(())
}

/// The pair as it is stored: smallest id first, so one row means one marriage
/// however the two were named when it was made.
fn pair(a: u64, b: u64) -> (i64, i64) {
    if a <= b { (a as i64, b as i64) } else { (b as i64, a as i64) }
}

/// A marriage, as the cards and the page read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Union {
    pub other: u64,
    pub since: i64,
    pub vows: String,
}

pub fn marry(conn: &Connection, a: u64, b: u64, now: i64) -> rusqlite::Result<bool> {
    let (low, high) = pair(a, b);
    let done = conn.execute(
        "INSERT OR IGNORE INTO unions (low, high, since) VALUES (?1, ?2, ?3)",
        params![low, high, now],
    )?;
    Ok(done > 0)
}

pub fn married(conn: &Connection, a: u64, b: u64) -> bool {
    let (low, high) = pair(a, b);
    conn.query_row("SELECT 1 FROM unions WHERE low = ?1 AND high = ?2", params![low, high], |_| Ok(())).is_ok()
}

/// Everyone somebody is married to, oldest marriage first.
pub fn spouses(conn: &Connection, user: u64) -> Vec<Union> {
    let mut out = Vec::new();
    let mut stmt = match conn.prepare(
        "SELECT CASE WHEN low = ?1 THEN high ELSE low END AS other, since, vows \
         FROM unions WHERE low = ?1 OR high = ?1 ORDER BY since",
    ) {
        Ok(stmt) => stmt,
        Err(_) => return out,
    };
    if let Ok(rows) = stmt.query_map(params![user as i64], |r| {
        Ok(Union { other: r.get::<_, i64>(0)? as u64, since: r.get(1)?, vows: r.get(2)? })
    }) {
        out.extend(rows.flatten());
    }
    out
}

/// Ends a marriage, keeping it in `past` so an ex is still a fact.
pub fn divorce(conn: &Connection, a: u64, b: u64, by: u64, now: i64) -> rusqlite::Result<bool> {
    let (low, high) = pair(a, b);
    let since: Option<i64> = conn
        .query_row("SELECT since FROM unions WHERE low = ?1 AND high = ?2", params![low, high], |r| r.get(0))
        .ok();
    let Some(since) = since else { return Ok(false) };
    conn.execute(
        "INSERT INTO past (low, high, since, until, ended_by) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![low, high, since, now, by as i64],
    )?;
    conn.execute("DELETE FROM unions WHERE low = ?1 AND high = ?2", params![low, high])?;
    Ok(true)
}

pub fn set_vows(conn: &Connection, a: u64, b: u64, vows: &str) -> rusqlite::Result<bool> {
    let (low, high) = pair(a, b);
    Ok(conn.execute("UPDATE unions SET vows = ?3 WHERE low = ?1 AND high = ?2", params![low, high, vows])? > 0)
}

pub fn adopt(conn: &Connection, parent: u64, child: u64, now: i64) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "INSERT OR IGNORE INTO kin (parent, child, since) VALUES (?1, ?2, ?3)",
        params![parent as i64, child as i64, now],
    )? > 0)
}

pub fn disown(conn: &Connection, parent: u64, child: u64) -> rusqlite::Result<bool> {
    Ok(conn.execute("DELETE FROM kin WHERE parent = ?1 AND child = ?2", params![parent as i64, child as i64])? > 0)
}

pub fn children(conn: &Connection, user: u64) -> Vec<u64> {
    ids(conn, "SELECT child FROM kin WHERE parent = ?1 ORDER BY since", user)
}

pub fn parents(conn: &Connection, user: u64) -> Vec<u64> {
    ids(conn, "SELECT parent FROM kin WHERE child = ?1 ORDER BY since", user)
}

fn ids(conn: &Connection, sql: &str, user: u64) -> Vec<u64> {
    let mut out = Vec::new();
    if let Ok(mut stmt) = conn.prepare(sql) {
        if let Ok(rows) = stmt.query_map(params![user as i64], |r| r.get::<_, i64>(0)) {
            out.extend(rows.flatten().map(|id| id as u64));
        }
    }
    out
}

/// Every parent-child link, for the cycle check and for the whole-server page.
pub fn all_kin(conn: &Connection) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    if let Ok(mut stmt) = conn.prepare("SELECT parent, child FROM kin") {
        if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64))) {
            out.extend(rows.flatten());
        }
    }
    out
}

pub fn all_unions(conn: &Connection) -> Vec<(u64, u64, i64)> {
    let mut out = Vec::new();
    if let Ok(mut stmt) = conn.prepare("SELECT low, high, since FROM unions ORDER BY since") {
        if let Ok(rows) = stmt.query_map([], |r| {
            Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64, r.get::<_, i64>(2)?))
        }) {
            out.extend(rows.flatten());
        }
    }
    out
}

/// Would `parent` adopting `child` make somebody their own ancestor?
///
/// Walks up from the would-be parent: if the child is already somewhere above
/// them, the link closes a loop and the tree stops being a tree. Somebody WILL
/// try this within the hour.
pub fn would_loop(kin: &[(u64, u64)], parent: u64, child: u64) -> bool {
    if parent == child {
        return true;
    }
    let mut up: HashMap<u64, Vec<u64>> = HashMap::new();
    for (p, c) in kin {
        up.entry(*c).or_default().push(*p);
    }
    let mut seen = HashSet::new();
    let mut stack = vec![parent];
    while let Some(at) = stack.pop() {
        if at == child {
            return true;
        }
        if !seen.insert(at) {
            continue;
        }
        stack.extend(up.get(&at).into_iter().flatten().copied());
    }
    false
}

/// When somebody last did this kind of thing, for the cooldowns.
pub fn last_gesture(conn: &Connection, kind: &str, who: u64) -> Option<i64> {
    conn.query_row(
        "SELECT ts FROM gestures WHERE kind = ?1 AND who = ?2 ORDER BY ts DESC LIMIT 1",
        params![kind, who as i64],
        |r| r.get(0),
    )
    .ok()
}

pub fn note_gesture(conn: &Connection, kind: &str, who: u64, whom: Option<u64>, now: i64) {
    let _ = conn.execute(
        "INSERT INTO gestures (kind, who, whom, ts) VALUES (?1, ?2, ?3, ?4)",
        params![kind, who as i64, whom.map(|w| w as i64), now],
    );
}

/// `refuser` does not want `unwanted` proposing to them again.
///
/// Named at both ends on purpose: an order that can be read two ways is how a
/// block ends up protecting the wrong person.
pub fn refuse(conn: &Connection, refuser: u64, unwanted: u64, now: i64) {
    let _ = conn.execute(
        "INSERT OR IGNORE INTO blocks (who, blocked, ts) VALUES (?1, ?2, ?3)",
        params![refuser as i64, unwanted as i64, now],
    );
}

/// Whether `asker` is barred from proposing to `target`.
pub fn is_refused(conn: &Connection, asker: u64, target: u64) -> bool {
    conn.query_row(
        "SELECT 1 FROM blocks WHERE who = ?1 AND blocked = ?2",
        params![target as i64, asker as i64],
        |_| Ok(()),
    )
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_test() -> Connection {
        let conn = Connection::open_in_memory().expect("memory db");
        conn.execute_batch(SCHEMA).expect("schema");
        conn
    }

    #[test]
    fn a_marriage_is_one_row_whichever_way_round_it_is_named() {
        let conn = open_test();
        assert!(marry(&conn, 20, 10, 100).expect("married"));
        assert!(married(&conn, 10, 20) && married(&conn, 20, 10), "both ways");
        assert!(!marry(&conn, 10, 20, 200).expect("again"), "the same pair cannot marry twice");
        assert_eq!(spouses(&conn, 10).len(), 1);
    }

    #[test]
    fn several_spouses_are_allowed_and_come_back_oldest_first() {
        let conn = open_test();
        marry(&conn, 1, 2, 300).unwrap();
        marry(&conn, 1, 3, 100).unwrap();
        marry(&conn, 1, 4, 200).unwrap();
        assert_eq!(spouses(&conn, 1).iter().map(|u| u.other).collect::<Vec<_>>(), vec![3, 4, 2]);
        // And each of them is married to exactly the one person so far.
        assert_eq!(spouses(&conn, 3).len(), 1);
    }

    #[test]
    fn a_divorce_keeps_the_marriage_as_a_fact() {
        let conn = open_test();
        marry(&conn, 1, 2, 100).unwrap();
        assert!(divorce(&conn, 2, 1, 2, 500).expect("divorced"));
        assert!(!married(&conn, 1, 2));
        assert!(!divorce(&conn, 1, 2, 1, 600).expect("again"), "nothing left to end");
        let (since, until, by): (i64, i64, i64) = conn
            .query_row("SELECT since, until, ended_by FROM past", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .expect("the past row");
        assert_eq!((since, until, by), (100, 500, 2), "who ended it, and when it ran");
    }

    #[test]
    fn nobody_can_become_their_own_ancestor() {
        // 1 adopted 2, 2 adopted 3. Now 3 tries to adopt 1.
        let kin = vec![(1, 2), (2, 3)];
        assert!(would_loop(&kin, 3, 1), "that would make 1 their own great-grandchild");
        assert!(would_loop(&kin, 1, 1), "nor their own parent");
        assert!(!would_loop(&kin, 3, 4), "an unrelated adoption is fine");
        assert!(!would_loop(&kin, 4, 1), "and so is adopting upwards into a new line");
    }

    #[test]
    fn a_refusal_stops_one_person_and_nobody_else() {
        let conn = open_test();
        // 1 does not want 2 asking again.
        refuse(&conn, 1, 2, 100);
        assert!(is_refused(&conn, 2, 1), "2 cannot propose to 1");
        assert!(!is_refused(&conn, 1, 2), "but 1 can still propose to 2");
        assert!(!is_refused(&conn, 3, 1), "and 3 is unaffected");
    }
}
