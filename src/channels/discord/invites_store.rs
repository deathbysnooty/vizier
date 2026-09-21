//! What invite tracking keeps, in its own small file in the runtime directory,
//! `invites.db`, so losing it never touches anything else.
//!
//! Two things. The invite snapshot: every guild invite the bot has seen, with
//! its use count as last read. An invite that disappears is kept with the time
//! it went, so a join that used up a single-use invite can still be matched to
//! it, and so the Invites page can still name who made it. And one row per
//! join: who, when, which invite and whose, and how sure the bot is. Both are
//! small and kept for good.

use std::path::Path;
use std::sync::OnceLock;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS invites (
        code TEXT PRIMARY KEY, inviter_id INTEGER, inviter_name TEXT NOT NULL DEFAULT '',
        channel_id INTEGER NOT NULL DEFAULT 0, channel_name TEXT NOT NULL DEFAULT '',
        uses INTEGER NOT NULL DEFAULT 0, max_uses INTEGER NOT NULL DEFAULT 0, max_age INTEGER NOT NULL DEFAULT 0,
        temporary INTEGER NOT NULL DEFAULT 0, created_ts INTEGER NOT NULL DEFAULT 0,
        first_seen_ts INTEGER NOT NULL, last_seen_ts INTEGER NOT NULL,
        gone_ts INTEGER, claimed INTEGER NOT NULL DEFAULT 0);
    CREATE TABLE IF NOT EXISTS joins (
        id INTEGER PRIMARY KEY AUTOINCREMENT, member_id INTEGER NOT NULL, member_name TEXT NOT NULL DEFAULT '',
        joined_ts INTEGER NOT NULL, how TEXT NOT NULL, code TEXT, inviter_id INTEGER,
        inviter_name TEXT NOT NULL DEFAULT '', candidates_json TEXT NOT NULL DEFAULT '[]', note TEXT NOT NULL DEFAULT '');
    CREATE INDEX IF NOT EXISTS joins_member ON joins (member_id, joined_ts);
    CREATE INDEX IF NOT EXISTS joins_inviter ON joins (inviter_id);
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
";

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

/// The shared connection. Hold the lock only for a query, never across an `.await`.
pub fn db() -> Option<&'static Mutex<Connection>> {
    DB.get()
}

/// Makes the tables, and notes the moment tracking began the first time only:
/// every join before it is one the bot cannot know about.
fn prepare(conn: &Connection, now: i64) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)?;
    conn.execute("INSERT OR IGNORE INTO meta (key, value) VALUES ('started_ts', ?1)", params![now.to_string()])?;
    Ok(())
}

/// Opens (or makes) `<workspace>/.runtime/invites.db`. Once per process.
pub fn open(workspace: &str) -> anyhow::Result<()> {
    if DB.get().is_some() {
        return Ok(());
    }
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    open_at(&dir.join("invites.db"))
}

pub fn open_at(path: &Path) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
    prepare(&conn, chrono::Utc::now().timestamp())?;
    let _ = DB.set(Mutex::new(conn));
    Ok(())
}

/// An in-memory store, for tests. Tracking "began" at `started_ts`.
pub fn open_memory(started_ts: i64) -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    prepare(&conn, started_ts)?;
    Ok(conn)
}

// --- meta --------------------------------------------------------------------------------

pub fn meta_get(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0)).optional()
}

pub fn meta_set(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// When tracking began: nothing before it is known.
pub fn started_ts(conn: &Connection) -> i64 {
    meta_get(conn, "started_ts").ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(0)
}

/// The server's vanity link and its use count as last read, if it has one.
pub fn vanity(conn: &Connection) -> Option<Vanity> {
    let code = meta_get(conn, "vanity_code").ok().flatten().filter(|c| !c.is_empty())?;
    let uses = meta_get(conn, "vanity_uses").ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(0);
    Some(Vanity { code, uses })
}

pub fn set_vanity(conn: &Connection, v: &Vanity) -> rusqlite::Result<()> {
    meta_set(conn, "vanity_code", &v.code)?;
    meta_set(conn, "vanity_uses", &v.uses.to_string())
}

// --- the snapshot ------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vanity {
    pub code: String,
    pub uses: u64,
}

/// One guild invite as Discord describes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Invite {
    pub code: String,
    /// None for an invite Discord names no creator for (a widget's, say).
    pub inviter_id: Option<u64>,
    pub inviter_name: String,
    pub channel_id: u64,
    pub channel_name: String,
    pub uses: u64,
    /// 0 is no limit.
    pub max_uses: u64,
    /// Seconds from creation; 0 is never.
    pub max_age: u64,
    pub temporary: bool,
    pub created_ts: i64,
}

impl Invite {
    /// A single-use invite, or one this next use would have finished.
    pub fn at_its_limit(&self) -> bool {
        self.max_uses > 0 && self.uses + 1 >= self.max_uses
    }

    /// When it runs out on its own, if ever.
    pub fn expires_ts(&self) -> Option<i64> {
        (self.max_age > 0 && self.created_ts > 0).then(|| self.created_ts + self.max_age as i64)
    }
}

/// An invite as kept: what it was when last seen, and when it went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stored {
    pub invite: Invite,
    pub first_seen_ts: i64,
    pub last_seen_ts: i64,
    pub gone_ts: Option<i64>,
    /// A join has already been put down to it after it went.
    pub claimed: bool,
}

const INVITE_COLUMNS: &str =
    "code, inviter_id, inviter_name, channel_id, channel_name, uses, max_uses, max_age, temporary, created_ts, first_seen_ts, last_seen_ts, gone_ts, claimed";

fn stored_row(r: &rusqlite::Row) -> rusqlite::Result<Stored> {
    Ok(Stored {
        invite: Invite {
            code: r.get(0)?,
            inviter_id: r.get::<_, Option<i64>>(1)?.map(|v| v as u64),
            inviter_name: r.get(2)?,
            channel_id: r.get::<_, i64>(3)? as u64,
            channel_name: r.get(4)?,
            uses: r.get::<_, i64>(5)?.max(0) as u64,
            max_uses: r.get::<_, i64>(6)?.max(0) as u64,
            max_age: r.get::<_, i64>(7)?.max(0) as u64,
            temporary: r.get::<_, i64>(8)? != 0,
            created_ts: r.get(9)?,
        },
        first_seen_ts: r.get(10)?,
        last_seen_ts: r.get(11)?,
        gone_ts: r.get(12)?,
        claimed: r.get::<_, i64>(13)? != 0,
    })
}

/// Adds or refreshes one invite as live now. A creator or channel name already
/// known is not wiped by a later read that lacks it.
pub fn upsert(conn: &Connection, i: &Invite, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO invites (code, inviter_id, inviter_name, channel_id, channel_name, uses, max_uses, max_age, temporary, created_ts, first_seen_ts, last_seen_ts, gone_ts, claimed)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11, NULL, 0)
         ON CONFLICT(code) DO UPDATE SET
            inviter_id = COALESCE(excluded.inviter_id, invites.inviter_id),
            inviter_name = CASE WHEN excluded.inviter_name = '' THEN invites.inviter_name ELSE excluded.inviter_name END,
            channel_id = CASE WHEN excluded.channel_id = 0 THEN invites.channel_id ELSE excluded.channel_id END,
            channel_name = CASE WHEN excluded.channel_name = '' THEN invites.channel_name ELSE excluded.channel_name END,
            uses = excluded.uses, max_uses = excluded.max_uses, max_age = excluded.max_age,
            temporary = excluded.temporary,
            created_ts = CASE WHEN excluded.created_ts = 0 THEN invites.created_ts ELSE excluded.created_ts END,
            last_seen_ts = excluded.last_seen_ts, gone_ts = NULL, claimed = 0",
        params![
            i.code,
            i.inviter_id.map(|v| v as i64),
            i.inviter_name,
            i.channel_id as i64,
            i.channel_name,
            i.uses as i64,
            i.max_uses as i64,
            i.max_age as i64,
            i.temporary as i64,
            i.created_ts,
            now,
        ],
    )?;
    Ok(())
}

/// An invite is gone (deleted, expired or used up). Kept, with when.
pub fn mark_gone(conn: &Connection, code: &str, now: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE invites SET gone_ts = ?2 WHERE code = ?1 AND gone_ts IS NULL", params![code, now])?;
    Ok(())
}

/// Makes the snapshot exactly `live`: each refreshed, and anything live before
/// that isn't in it any more marked gone.
pub fn replace_snapshot(conn: &Connection, live: &[Invite], now: i64) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    for i in live {
        upsert(&tx, i, now)?;
    }
    let codes: std::collections::HashSet<&str> = live.iter().map(|i| i.code.as_str()).collect();
    for s in active(&tx)? {
        if !codes.contains(s.invite.code.as_str()) {
            mark_gone(&tx, &s.invite.code, now)?;
        }
    }
    tx.commit()
}

/// The live snapshot.
pub fn active(conn: &Connection) -> rusqlite::Result<Vec<Stored>> {
    let mut stmt = conn.prepare_cached(&format!("SELECT {} FROM invites WHERE gone_ts IS NULL ORDER BY code", INVITE_COLUMNS))?;
    let rows = stmt.query_map([], stored_row)?;
    rows.collect()
}

/// Invites that went at or after `since` and no join has been put down to yet.
pub fn recently_gone(conn: &Connection, since: i64) -> rusqlite::Result<Vec<Stored>> {
    let mut stmt =
        conn.prepare_cached(&format!("SELECT {} FROM invites WHERE gone_ts IS NOT NULL AND gone_ts >= ?1 AND claimed = 0 ORDER BY code", INVITE_COLUMNS))?;
    let rows = stmt.query_map(params![since], stored_row)?;
    rows.collect()
}

pub fn claim(conn: &Connection, code: &str) -> rusqlite::Result<()> {
    conn.execute("UPDATE invites SET claimed = 1 WHERE code = ?1", params![code])?;
    Ok(())
}

/// Every invite ever seen, live ones first, then newest.
pub fn all_invites(conn: &Connection) -> rusqlite::Result<Vec<Stored>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {} FROM invites ORDER BY (gone_ts IS NOT NULL), COALESCE(gone_ts, 0) DESC, uses DESC, code",
        INVITE_COLUMNS
    ))?;
    let rows = stmt.query_map([], stored_row)?;
    rows.collect()
}

// --- joins ---------------------------------------------------------------------------------------

/// How sure the bot is of the invite a member joined through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum How {
    /// Exactly one invite's count went up.
    Sure,
    /// Nothing went up, but a single-use (or nearly used up) invite disappeared.
    Likely,
    /// The server's vanity link's count went up.
    Vanity,
    /// More than one went up: someone else joined at the same moment.
    Unsure,
    /// Nothing the bot can see changed.
    Unknown,
}

impl How {
    pub fn key(self) -> &'static str {
        match self {
            How::Sure => "sure",
            How::Likely => "likely",
            How::Vanity => "vanity",
            How::Unsure => "unsure",
            How::Unknown => "unknown",
        }
    }

    pub fn from_key(key: &str) -> How {
        match key {
            "sure" => How::Sure,
            "likely" => How::Likely,
            "vanity" => How::Vanity,
            "unsure" => How::Unsure,
            _ => How::Unknown,
        }
    }

    /// Whether the invite's creator gets the credit for this join.
    pub fn credits_inviter(self) -> bool {
        matches!(self, How::Sure | How::Likely)
    }
}

/// One invite (or the vanity link) a join might have come through.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    /// The invite code; for the vanity link, its code too.
    pub code: String,
    #[serde(default)]
    pub inviter_id: Option<u64>,
    #[serde(default)]
    pub inviter_name: String,
    #[serde(default)]
    pub vanity: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewJoin {
    pub member_id: u64,
    pub member_name: String,
    pub joined_ts: i64,
    pub how: How,
    pub code: Option<String>,
    pub inviter_id: Option<u64>,
    pub inviter_name: String,
    /// When unsure: every invite it might have been.
    pub candidates: Vec<Candidate>,
    /// Anything worth saying, like the invite list couldn't be read.
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Join {
    pub id: i64,
    pub new: NewJoin,
}

pub fn add_join(conn: &Connection, j: &NewJoin) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO joins (member_id, member_name, joined_ts, how, code, inviter_id, inviter_name, candidates_json, note)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            j.member_id as i64,
            j.member_name,
            j.joined_ts,
            j.how.key(),
            j.code,
            j.inviter_id.map(|v| v as i64),
            j.inviter_name,
            serde_json::to_string(&j.candidates).unwrap_or_else(|_| "[]".into()),
            j.note,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

const JOIN_COLUMNS: &str = "id, member_id, member_name, joined_ts, how, code, inviter_id, inviter_name, candidates_json, note";

fn join_row(r: &rusqlite::Row) -> rusqlite::Result<Join> {
    Ok(Join {
        id: r.get(0)?,
        new: NewJoin {
            member_id: r.get::<_, i64>(1)? as u64,
            member_name: r.get(2)?,
            joined_ts: r.get(3)?,
            how: How::from_key(&r.get::<_, String>(4)?),
            code: r.get(5)?,
            inviter_id: r.get::<_, Option<i64>>(6)?.map(|v| v as u64),
            inviter_name: r.get(7)?,
            candidates: serde_json::from_str(&r.get::<_, String>(8)?).unwrap_or_default(),
            note: r.get(9)?,
        },
    })
}

/// One member's joins, newest first.
pub fn joins_of(conn: &Connection, member: u64) -> rusqlite::Result<Vec<Join>> {
    let mut stmt = conn.prepare_cached(&format!("SELECT {} FROM joins WHERE member_id = ?1 ORDER BY joined_ts DESC, id DESC", JOIN_COLUMNS))?;
    let rows = stmt.query_map(params![member as i64], join_row)?;
    rows.collect()
}

/// Every join tracked, newest first.
pub fn all_joins(conn: &Connection) -> rusqlite::Result<Vec<Join>> {
    let mut stmt = conn.prepare_cached(&format!("SELECT {} FROM joins ORDER BY joined_ts DESC, id DESC", JOIN_COLUMNS))?;
    let rows = stmt.query_map([], join_row)?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inv(code: &str, uses: u64, max_uses: u64) -> Invite {
        Invite { code: code.into(), inviter_id: Some(7), inviter_name: "Kabir".into(), channel_id: 21, channel_name: "general".into(), uses, max_uses, ..Default::default() }
    }

    #[test]
    fn tracking_remembers_when_it_began_and_only_the_first_time() {
        let conn = open_memory(1_000).unwrap();
        assert_eq!(started_ts(&conn), 1_000);
        prepare(&conn, 9_999).unwrap();
        assert_eq!(started_ts(&conn), 1_000, "a restart is not a new start");
    }

    #[test]
    fn the_snapshot_keeps_what_went_and_brings_back_what_returns() {
        let conn = open_memory(0).unwrap();
        replace_snapshot(&conn, &[inv("a", 1, 0), inv("b", 0, 1)], 10).unwrap();
        assert_eq!(active(&conn).unwrap().len(), 2);
        replace_snapshot(&conn, &[inv("a", 2, 0)], 20).unwrap();
        let live: Vec<String> = active(&conn).unwrap().into_iter().map(|s| s.invite.code).collect();
        assert_eq!(live, vec!["a"]);
        let gone = recently_gone(&conn, 15).unwrap();
        assert_eq!((gone.len(), gone[0].invite.code.as_str(), gone[0].gone_ts), (1, "b", Some(20)));
        claim(&conn, "b").unwrap();
        assert!(recently_gone(&conn, 15).unwrap().is_empty(), "a claimed invite is not offered twice");
        // Still listed on the page, gone last.
        let all = all_invites(&conn).unwrap();
        assert_eq!(all.iter().map(|s| s.invite.code.as_str()).collect::<Vec<_>>(), vec!["a", "b"]);
        // A read with no creator does not wipe the one already known.
        upsert(&conn, &Invite { inviter_id: None, inviter_name: String::new(), ..inv("a", 3, 0) }, 30).unwrap();
        let a = &active(&conn).unwrap()[0];
        assert_eq!((a.invite.inviter_id, a.invite.uses), (Some(7), 3));
    }

    #[test]
    fn joins_round_trip_with_their_candidates() {
        let conn = open_memory(0).unwrap();
        let j = NewJoin {
            member_id: 5,
            member_name: "Zoya".into(),
            joined_ts: 100,
            how: How::Unsure,
            code: None,
            inviter_id: None,
            inviter_name: String::new(),
            candidates: vec![Candidate { code: "a".into(), inviter_id: Some(7), inviter_name: "Kabir".into(), vanity: false }],
            note: String::new(),
        };
        let id = add_join(&conn, &j).unwrap();
        let back = joins_of(&conn, 5).unwrap();
        assert_eq!(back, vec![Join { id, new: j }]);
        assert_eq!(all_joins(&conn).unwrap().len(), 1);
        set_vanity(&conn, &Vanity { code: "mlci".into(), uses: 4 }).unwrap();
        assert_eq!(vanity(&conn), Some(Vanity { code: "mlci".into(), uses: 4 }));
    }
}
