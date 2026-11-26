//! Stores temporary voice rooms, saved defaults and access lists.

use std::path::Path;
use std::sync::OnceLock;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, Row, params};

const ROOM_SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS rooms (
        channel_id INTEGER PRIMARY KEY CHECK (channel_id <> 0),
        guild_id INTEGER NOT NULL CHECK (guild_id <> 0),
        hub_id INTEGER NOT NULL CHECK (hub_id <> 0),
        owner_id INTEGER NOT NULL CHECK (owner_id <> 0)
    );
    CREATE INDEX IF NOT EXISTS rooms_owner ON rooms (guild_id, owner_id);
    CREATE INDEX IF NOT EXISTS rooms_hub ON rooms (hub_id);
    CREATE TABLE IF NOT EXISTS room_renames (
        channel_id INTEGER PRIMARY KEY REFERENCES rooms(channel_id) ON DELETE CASCADE,
        previous_ms INTEGER,
        latest_ms INTEGER,
        pending TEXT
    );
    CREATE TABLE IF NOT EXISTS suppressed_role_permissions (
        channel_id INTEGER NOT NULL REFERENCES rooms(channel_id) ON DELETE CASCADE,
        permission INTEGER NOT NULL CHECK (permission <> 0),
        role_id INTEGER NOT NULL CHECK (role_id <> 0),
        PRIMARY KEY (channel_id, permission, role_id)
    );
";
const PROFILE_SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS profiles (
        guild_id INTEGER NOT NULL CHECK (guild_id <> 0),
        owner_id INTEGER NOT NULL CHECK (owner_id <> 0),
        name TEXT,
        user_limit INTEGER CHECK (user_limit BETWEEN 0 AND 99),
        locked INTEGER CHECK (locked IN (0, 1)),
        hidden INTEGER CHECK (hidden IN (0, 1)),
        bitrate INTEGER CHECK (bitrate BETWEEN 8000 AND 384000),
        PRIMARY KEY (guild_id, owner_id)
    );
    CREATE TABLE IF NOT EXISTS trusted (
        guild_id INTEGER NOT NULL CHECK (guild_id <> 0),
        owner_id INTEGER NOT NULL CHECK (owner_id <> 0),
        user_id INTEGER NOT NULL CHECK (user_id <> 0 AND user_id <> owner_id),
        PRIMARY KEY (guild_id, owner_id, user_id)
    );
    CREATE TABLE IF NOT EXISTS bans (
        guild_id INTEGER NOT NULL CHECK (guild_id <> 0),
        owner_id INTEGER NOT NULL CHECK (owner_id <> 0),
        user_id INTEGER NOT NULL CHECK (user_id <> 0 AND user_id <> owner_id),
        PRIMARY KEY (guild_id, owner_id, user_id)
    );
    CREATE INDEX IF NOT EXISTS trusted_member ON trusted (guild_id, user_id);
    CREATE INDEX IF NOT EXISTS banned_member ON bans (guild_id, user_id);
    CREATE TABLE IF NOT EXISTS room_access_overwrites (
        channel_id INTEGER NOT NULL REFERENCES rooms(channel_id) ON DELETE CASCADE,
        user_id INTEGER NOT NULL CHECK (user_id <> 0),
        original_allow INTEGER NOT NULL,
        original_deny INTEGER NOT NULL,
        PRIMARY KEY (channel_id, user_id)
    );
";
const ROOM_COLUMNS: &str = "channel_id, guild_id, hub_id, owner_id";

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

/// Hold the connection lock for a query and release it before awaiting Discord.
pub fn db() -> Option<&'static Mutex<Connection>> {
    DB.get()
}

/// Opens or creates `<workspace>/.runtime/tempvc.db` once per process.
pub fn open(workspace: &str) -> anyhow::Result<()> {
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    open_at(&dir.join("tempvc.db"))
}

pub fn open_at(path: &Path) -> anyhow::Result<()> {
    if db().is_none() {
        let _ = DB.set(Mutex::new(connect(path)?));
    }
    Ok(())
}

pub(super) fn connect(path: &Path) -> anyhow::Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000; PRAGMA foreign_keys=ON;",
    )?;
    conn.execute_batch(ROOM_SCHEMA)?;
    conn.execute_batch(PROFILE_SCHEMA)?;
    Ok(conn)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Room {
    pub channel_id: u64,
    pub guild_id: u64,
    pub hub_id: u64,
    pub owner_id: u64,
}

fn room_row(row: &Row<'_>) -> rusqlite::Result<Room> {
    Ok(Room {
        channel_id: row.get::<_, i64>(0)? as u64,
        guild_id: row.get::<_, i64>(1)? as u64,
        hub_id: row.get::<_, i64>(2)? as u64,
        owner_id: row.get::<_, i64>(3)? as u64,
    })
}

pub fn insert(conn: &Connection, room: &Room) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO rooms (channel_id, guild_id, hub_id, owner_id) VALUES (?1, ?2, ?3, ?4)",
        params![
            room.channel_id as i64,
            room.guild_id as i64,
            room.hub_id as i64,
            room.owner_id as i64,
        ],
    )?;
    Ok(())
}

pub fn room(conn: &Connection, channel_id: u64) -> rusqlite::Result<Option<Room>> {
    conn.query_row(
        &format!("SELECT {ROOM_COLUMNS} FROM rooms WHERE channel_id = ?1"),
        [channel_id as i64],
        room_row,
    )
    .optional()
}

/// Transfers can make a member the owner of several rooms. Find the oldest.
pub fn owned_room(
    conn: &Connection,
    guild_id: u64,
    owner_id: u64,
) -> rusqlite::Result<Option<Room>> {
    conn.query_row(
        &format!("SELECT {ROOM_COLUMNS} FROM rooms WHERE guild_id = ?1 AND owner_id = ?2 ORDER BY channel_id LIMIT 1"),
        params![guild_id as i64, owner_id as i64],
        room_row,
    )
    .optional()
}

pub fn owned_rooms(conn: &Connection, guild_id: u64, owner_id: u64) -> rusqlite::Result<Vec<Room>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {ROOM_COLUMNS} FROM rooms WHERE guild_id = ?1 AND owner_id = ?2 ORDER BY channel_id"
    ))?;
    stmt.query_map(params![guild_id as i64, owner_id as i64], room_row)?
        .collect()
}

pub fn rooms_listing_member(
    conn: &Connection,
    guild_id: u64,
    user_id: u64,
) -> rusqlite::Result<Vec<Room>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {ROOM_COLUMNS} FROM rooms WHERE guild_id = ?1 AND owner_id IN (
            SELECT owner_id FROM trusted WHERE guild_id = ?1 AND user_id = ?2
            UNION SELECT owner_id FROM bans WHERE guild_id = ?1 AND user_id = ?2
         ) ORDER BY channel_id"
    ))?;
    stmt.query_map(params![guild_id as i64, user_id as i64], room_row)?
        .collect()
}

pub fn guild_rooms(conn: &Connection, guild_id: u64) -> rusqlite::Result<Vec<Room>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {ROOM_COLUMNS} FROM rooms WHERE guild_id = ?1"
    ))?;
    stmt.query_map([guild_id as i64], room_row)?.collect()
}

pub fn count_for_hub(conn: &Connection, hub_id: u64) -> rusqlite::Result<usize> {
    conn.query_row(
        "SELECT COUNT(*) FROM rooms WHERE hub_id = ?1",
        [hub_id as i64],
        |row| {
            let count: i64 = row.get(0)?;
            usize::try_from(count).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, count))
        },
    )
}

pub fn set_owner(
    conn: &Connection,
    channel_id: u64,
    old_owner_id: u64,
    new_owner_id: u64,
) -> rusqlite::Result<bool> {
    let updated = conn.execute(
        "UPDATE rooms SET owner_id = ?1 WHERE channel_id = ?2 AND owner_id = ?3",
        params![new_owner_id as i64, channel_id as i64, old_owner_id as i64],
    )?;
    Ok(updated != 0)
}

pub fn delete(conn: &Connection, channel_id: u64) -> rusqlite::Result<bool> {
    let removed = conn.execute(
        "DELETE FROM rooms WHERE channel_id = ?1",
        [channel_id as i64],
    )?;
    Ok(removed != 0)
}

/// A missing field inherits the room's configured default. Bitrate is in bits per second.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    pub name: Option<String>,
    pub limit: Option<u32>,
    pub locked: Option<bool>,
    pub hidden: Option<bool>,
    pub bitrate: Option<u32>,
}

pub fn profile(
    conn: &Connection,
    guild_id: u64,
    owner_id: u64,
) -> rusqlite::Result<Option<Profile>> {
    conn.query_row(
        "SELECT name, user_limit, locked, hidden, bitrate
         FROM profiles WHERE guild_id = ?1 AND owner_id = ?2",
        params![guild_id as i64, owner_id as i64],
        |row| {
            Ok(Profile {
                name: row.get(0)?,
                limit: row.get(1)?,
                locked: row.get(2)?,
                hidden: row.get(3)?,
                bitrate: row.get(4)?,
            })
        },
    )
    .optional()
}

pub fn save_profile(
    conn: &Connection,
    guild_id: u64,
    owner_id: u64,
    profile: &Profile,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO profiles (guild_id, owner_id, name, user_limit, locked, hidden, bitrate)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (guild_id, owner_id) DO UPDATE SET
             name = COALESCE(excluded.name, profiles.name),
             user_limit = COALESCE(excluded.user_limit, profiles.user_limit),
             locked = COALESCE(excluded.locked, profiles.locked),
             hidden = COALESCE(excluded.hidden, profiles.hidden),
             bitrate = COALESCE(excluded.bitrate, profiles.bitrate)",
        params![
            guild_id as i64,
            owner_id as i64,
            profile.name,
            profile.limit,
            profile.locked,
            profile.hidden,
            profile.bitrate,
        ],
    )?;
    Ok(())
}

pub fn clear_profile(conn: &Connection, guild_id: u64, owner_id: u64) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM profiles WHERE guild_id = ?1 AND owner_id = ?2",
        params![guild_id as i64, owner_id as i64],
    )?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Trusted,
    Banned,
}

impl Access {
    fn table(self) -> &'static str {
        match self {
            Self::Trusted => "trusted",
            Self::Banned => "bans",
        }
    }

    fn opposite(self) -> Self {
        match self {
            Self::Trusted => Self::Banned,
            Self::Banned => Self::Trusted,
        }
    }
}

pub fn access_list(
    conn: &Connection,
    guild_id: u64,
    owner_id: u64,
    access: Access,
) -> rusqlite::Result<Vec<u64>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT user_id FROM {} WHERE guild_id = ?1 AND owner_id = ?2 ORDER BY user_id",
        access.table()
    ))?;
    stmt.query_map(params![guild_id as i64, owner_id as i64], |row| {
        Ok(row.get::<_, i64>(0)? as u64)
    })?
    .collect()
}

pub fn is_banned(
    conn: &Connection,
    guild_id: u64,
    owner_id: u64,
    user_id: u64,
) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM bans WHERE guild_id = ?1 AND owner_id = ?2 AND user_id = ?3)",
        params![guild_id as i64, owner_id as i64, user_id as i64],
        |row| row.get(0),
    )
}

/// A member can be trusted or banned by a particular owner. Adding one replaces the other.
pub fn set_access(
    conn: &Connection,
    guild_id: u64,
    owner_id: u64,
    user_id: u64,
    access: Access,
    enabled: bool,
) -> rusqlite::Result<()> {
    let ids = params![guild_id as i64, owner_id as i64, user_id as i64];
    let removed = if enabled { access.opposite() } else { access };
    let tx = (enabled && conn.is_autocommit())
        .then(|| conn.unchecked_transaction())
        .transpose()?;
    let conn = tx.as_deref().unwrap_or(conn);
    if enabled {
        conn.execute(
            &format!(
                "INSERT INTO {} (guild_id, owner_id, user_id) VALUES (?1, ?2, ?3)
                 ON CONFLICT (guild_id, owner_id, user_id) DO NOTHING",
                access.table()
            ),
            ids,
        )?;
    }
    conn.execute(
        &format!(
            "DELETE FROM {} WHERE guild_id = ?1 AND owner_id = ?2 AND user_id = ?3",
            removed.table()
        ),
        ids,
    )?;
    if let Some(tx) = tx {
        tx.commit()?;
    }
    Ok(())
}

/// Member IDs and the allow/deny bits to restore after removing managed room access.
pub fn room_access(conn: &Connection, channel_id: u64) -> rusqlite::Result<Vec<(u64, u64, u64)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT user_id, original_allow, original_deny
         FROM room_access_overwrites WHERE channel_id = ?1 ORDER BY user_id",
    )?;
    stmt.query_map([channel_id as i64], |row| {
        Ok((
            row.get::<_, i64>(0)? as u64,
            row.get::<_, i64>(1)? as u64,
            row.get::<_, i64>(2)? as u64,
        ))
    })?
    .collect()
}

pub fn set_room_access(
    conn: &Connection,
    channel_id: u64,
    user_id: u64,
    baseline: Option<(u64, u64)>,
) -> rusqlite::Result<()> {
    if let Some((allow, deny)) = baseline {
        conn.execute(
            "INSERT INTO room_access_overwrites (channel_id, user_id, original_allow, original_deny)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (channel_id, user_id) DO UPDATE SET
                 original_allow = excluded.original_allow,
                 original_deny = excluded.original_deny",
            params![channel_id as i64, user_id as i64, allow as i64, deny as i64],
        )?;
    } else {
        conn.execute(
            "DELETE FROM room_access_overwrites WHERE channel_id = ?1 AND user_id = ?2",
            params![channel_id as i64, user_id as i64],
        )?;
    }
    Ok(())
}

/// The two most recent renames in UTC Unix milliseconds, and the queued name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rename {
    pub previous_ms: Option<i64>,
    pub latest_ms: Option<i64>,
    pub pending: Option<String>,
}

pub fn rename(conn: &Connection, channel_id: u64) -> rusqlite::Result<Option<Rename>> {
    conn.query_row(
        "SELECT previous_ms, latest_ms, pending FROM room_renames WHERE channel_id = ?1",
        [channel_id as i64],
        |row| {
            Ok(Rename {
                previous_ms: row.get(0)?,
                latest_ms: row.get(1)?,
                pending: row.get(2)?,
            })
        },
    )
    .optional()
}

pub fn save_rename(conn: &Connection, channel_id: u64, rename: &Rename) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO room_renames (channel_id, previous_ms, latest_ms, pending) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(channel_id) DO UPDATE SET previous_ms = excluded.previous_ms,
             latest_ms = excluded.latest_ms, pending = excluded.pending",
        params![
            channel_id as i64,
            rename.previous_ms,
            rename.latest_ms,
            rename.pending,
        ],
    )?;
    Ok(())
}

/// Roles whose explicit allows were removed to make a room restriction effective.
pub fn load_suppressed_roles(
    conn: &Connection,
    channel_id: u64,
    permission: u64,
) -> rusqlite::Result<Vec<u64>> {
    let mut stmt = conn.prepare_cached(
        "SELECT role_id FROM suppressed_role_permissions WHERE channel_id = ?1 AND permission = ?2",
    )?;
    stmt.query_map(params![channel_id as i64, permission as i64], |row| {
        Ok(row.get::<_, i64>(0)? as u64)
    })?
    .collect()
}

pub fn save_suppressed_roles(
    conn: &Connection,
    channel_id: u64,
    permission: u64,
    roles: &[u64],
) -> rusqlite::Result<()> {
    let tx = conn
        .is_autocommit()
        .then(|| conn.unchecked_transaction())
        .transpose()?;
    let conn = tx.as_deref().unwrap_or(conn);
    conn.execute(
        "DELETE FROM suppressed_role_permissions WHERE channel_id = ?1 AND permission = ?2",
        params![channel_id as i64, permission as i64],
    )?;
    {
        let mut insert = conn.prepare_cached(
            "INSERT INTO suppressed_role_permissions (channel_id, permission, role_id) VALUES (?1, ?2, ?3)
             ON CONFLICT (channel_id, permission, role_id) DO NOTHING",
        )?;
        for role in roles {
            insert.execute(params![channel_id as i64, permission as i64, *role as i64])?;
        }
    }
    if let Some(tx) = tx {
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rooms_survive_reopen_and_support_owner_updates_and_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".runtime").join("tempvc.db");
        let conn = connect(&path).unwrap();
        let first = Room {
            channel_id: 1,
            guild_id: 10,
            hub_id: 20,
            owner_id: 30,
        };
        let second = Room {
            channel_id: 2,
            ..first
        };

        assert_eq!(room(&conn, first.channel_id).unwrap(), None);
        insert(&conn, &first).unwrap();
        insert(&conn, &second).unwrap();
        assert!(insert(&conn, &first).is_err());
        assert_eq!(guild_rooms(&conn, first.guild_id).unwrap().len(), 2);
        assert!(guild_rooms(&conn, 11).unwrap().is_empty());
        assert_eq!(
            owned_rooms(&conn, first.guild_id, first.owner_id).unwrap(),
            vec![first, second]
        );
        assert_eq!(count_for_hub(&conn, first.hub_id).unwrap(), 2);
        assert_eq!(count_for_hub(&conn, 21).unwrap(), 0);
        assert_eq!(
            owned_room(&conn, first.guild_id, first.owner_id).unwrap(),
            Some(first)
        );
        assert_eq!(owned_room(&conn, 11, first.owner_id).unwrap(), None);

        assert!(set_owner(&conn, first.channel_id, first.owner_id, 31).unwrap());
        assert!(!set_owner(&conn, first.channel_id, first.owner_id, 32).unwrap());
        drop(conn);

        let conn = connect(&path).unwrap();
        let transferred = Room {
            owner_id: 31,
            ..first
        };
        assert_eq!(room(&conn, first.channel_id).unwrap(), Some(transferred));
        assert_eq!(
            owned_room(&conn, first.guild_id, first.owner_id).unwrap(),
            Some(second)
        );
        assert_eq!(
            owned_rooms(&conn, first.guild_id, first.owner_id).unwrap(),
            vec![second]
        );
        assert!(delete(&conn, first.channel_id).unwrap());
        assert!(!delete(&conn, first.channel_id).unwrap());
        assert_eq!(guild_rooms(&conn, first.guild_id).unwrap(), vec![second]);
        assert_eq!(count_for_hub(&conn, first.hub_id).unwrap(), 1);
    }

    #[test]
    fn stage_one_database_gains_profiles_without_losing_existing_rooms() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tempvc.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(ROOM_SCHEMA).unwrap();
        let managed = Room {
            channel_id: 1,
            guild_id: 10,
            hub_id: 20,
            owner_id: 30,
        };
        insert(&conn, &managed).unwrap();
        let queued = Rename {
            pending: Some("New name".into()),
            ..Rename::default()
        };
        save_rename(&conn, managed.channel_id, &queued).unwrap();
        drop(conn);

        let conn = connect(&path).unwrap();
        assert_eq!(room(&conn, managed.channel_id).unwrap(), Some(managed));
        assert_eq!(rename(&conn, managed.channel_id).unwrap(), Some(queued));
        assert_eq!(
            profile(&conn, managed.guild_id, managed.owner_id).unwrap(),
            None
        );
        for access in [Access::Trusted, Access::Banned] {
            assert!(
                access_list(&conn, managed.guild_id, managed.owner_id, access)
                    .unwrap()
                    .is_empty()
            );
        }
        assert!(room_access(&conn, managed.channel_id).unwrap().is_empty());
    }

    #[test]
    fn profiles_patch_only_supplied_fields_and_can_be_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tempvc.db");
        let conn = connect(&path).unwrap();
        let initial = Profile {
            name: Some("{user}'s room {count}".into()),
            limit: Some(10),
            locked: Some(true),
            ..Profile::default()
        };
        save_profile(&conn, 10, 30, &initial).unwrap();
        assert_eq!(profile(&conn, 10, 30).unwrap(), Some(initial.clone()));
        save_profile(
            &conn,
            10,
            30,
            &Profile {
                limit: Some(0),
                locked: Some(false),
                hidden: Some(true),
                bitrate: Some(64_000),
                ..Profile::default()
            },
        )
        .unwrap();
        let expected = Profile {
            limit: Some(0),
            locked: Some(false),
            hidden: Some(true),
            bitrate: Some(64_000),
            ..initial
        };
        drop(conn);

        let conn = connect(&path).unwrap();
        assert_eq!(profile(&conn, 10, 30).unwrap(), Some(expected));
        assert_eq!(profile(&conn, 11, 30).unwrap(), None);
        assert_eq!(profile(&conn, 10, 31).unwrap(), None);
        clear_profile(&conn, 10, 30).unwrap();
        clear_profile(&conn, 10, 30).unwrap();
        assert_eq!(profile(&conn, 10, 30).unwrap(), None);
    }

    #[test]
    fn access_lists_persist_and_replacing_access_is_scoped_to_one_owner_and_guild() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tempvc.db");
        let conn = connect(&path).unwrap();
        for (guild, owner) in [(10, 30), (11, 30), (10, 31)] {
            set_access(&conn, guild, owner, 40, Access::Trusted, true).unwrap();
        }
        set_access(&conn, 10, 30, 40, Access::Trusted, true).unwrap();
        assert_eq!(access_list(&conn, 10, 30, Access::Trusted).unwrap(), [40]);
        assert!(!is_banned(&conn, 10, 30, 40).unwrap());
        set_access(&conn, 10, 30, 40, Access::Banned, true).unwrap();
        assert!(
            access_list(&conn, 10, 30, Access::Trusted)
                .unwrap()
                .is_empty()
        );
        drop(conn);

        let conn = connect(&path).unwrap();
        assert!(is_banned(&conn, 10, 30, 40).unwrap());
        assert_eq!(access_list(&conn, 10, 30, Access::Banned).unwrap(), [40]);
        assert_eq!(access_list(&conn, 11, 30, Access::Trusted).unwrap(), [40]);
        assert_eq!(access_list(&conn, 10, 31, Access::Trusted).unwrap(), [40]);
        let room = Room {
            channel_id: 1,
            guild_id: 10,
            hub_id: 20,
            owner_id: 30,
        };
        let trusted_room = Room {
            channel_id: 2,
            owner_id: 31,
            ..room
        };
        insert(&conn, &room).unwrap();
        insert(&conn, &trusted_room).unwrap();
        insert(
            &conn,
            &Room {
                channel_id: 3,
                guild_id: 11,
                ..room
            },
        )
        .unwrap();
        assert_eq!(
            rooms_listing_member(&conn, 10, 40).unwrap(),
            [room, trusted_room]
        );
        assert!(rooms_listing_member(&conn, 10, 99).unwrap().is_empty());
        set_access(&conn, 10, 30, 40, Access::Trusted, true).unwrap();
        assert!(!is_banned(&conn, 10, 30, 40).unwrap());
        set_access(&conn, 10, 30, 40, Access::Trusted, false).unwrap();
        assert!(
            access_list(&conn, 10, 30, Access::Trusted)
                .unwrap()
                .is_empty()
        );
        set_access(&conn, 10, 30, 41, Access::Banned, true).unwrap();
        clear_profile(&conn, 10, 30).unwrap();
        assert!(is_banned(&conn, 10, 30, 41).unwrap());
        set_access(&conn, 10, 30, 41, Access::Banned, false).unwrap();
        assert!(!is_banned(&conn, 10, 30, 41).unwrap());
    }

    #[test]
    fn room_access_baselines_survive_restart_and_are_removed_with_room() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tempvc.db");
        let conn = connect(&path).unwrap();
        let managed = Room {
            channel_id: 1,
            guild_id: 10,
            hub_id: 20,
            owner_id: 30,
        };
        let connect_bit = 1 << 20;
        let view_bit = 1 << 10;
        insert(&conn, &managed).unwrap();
        set_room_access(&conn, managed.channel_id, 40, Some((connect_bit, 0))).unwrap();
        set_room_access(&conn, managed.channel_id, 40, Some((0, view_bit))).unwrap();
        set_room_access(&conn, managed.channel_id, 41, Some((0, 0))).unwrap();
        drop(conn);

        let conn = connect(&path).unwrap();
        assert_eq!(
            room_access(&conn, managed.channel_id).unwrap(),
            [(40, 0, view_bit), (41, 0, 0)]
        );
        set_room_access(&conn, managed.channel_id, 41, None).unwrap();
        assert_eq!(
            room_access(&conn, managed.channel_id).unwrap(),
            [(40, 0, view_bit)]
        );
        assert!(delete(&conn, managed.channel_id).unwrap());
        assert!(room_access(&conn, managed.channel_id).unwrap().is_empty());
    }

    #[test]
    fn rename_queue_survives_reopen_and_is_removed_with_room() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tempvc.db");
        let conn = connect(&path).unwrap();
        let managed = Room {
            channel_id: 1,
            guild_id: 10,
            hub_id: 20,
            owner_id: 30,
        };
        insert(&conn, &managed).unwrap();
        assert_eq!(rename(&conn, managed.channel_id).unwrap(), None);
        let initial = Rename {
            latest_ms: Some(20),
            ..Rename::default()
        };
        save_rename(&conn, managed.channel_id, &initial).unwrap();
        let queued = Rename {
            previous_ms: Some(20),
            latest_ms: Some(30),
            pending: Some("New name".into()),
        };
        save_rename(&conn, managed.channel_id, &queued).unwrap();
        drop(conn);

        let conn = connect(&path).unwrap();
        assert_eq!(
            rename(&conn, managed.channel_id).unwrap(),
            Some(queued.clone())
        );
        assert!(delete(&conn, managed.channel_id).unwrap());
        assert_eq!(rename(&conn, managed.channel_id).unwrap(), None);
        assert!(save_rename(&conn, managed.channel_id, &queued).is_err());
    }

    #[test]
    fn role_suppression_persists_by_room_and_permission() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tempvc.db");
        let conn = connect(&path).unwrap();
        let channel = 1;
        let connect_bit = 1 << 20;
        let view_bit = 1 << 10;
        insert(
            &conn,
            &Room {
                channel_id: channel,
                guild_id: 10,
                hub_id: 20,
                owner_id: 30,
            },
        )
        .unwrap();
        save_suppressed_roles(&conn, channel, connect_bit, &[40, 41]).unwrap();
        save_suppressed_roles(&conn, channel, connect_bit, &[42]).unwrap();
        save_suppressed_roles(&conn, channel, view_bit, &[43]).unwrap();
        assert!(save_suppressed_roles(&conn, channel, connect_bit, &[44, 0]).is_err());
        drop(conn);

        let conn = connect(&path).unwrap();
        assert_eq!(
            load_suppressed_roles(&conn, channel, connect_bit).unwrap(),
            vec![42]
        );
        assert_eq!(
            load_suppressed_roles(&conn, channel, view_bit).unwrap(),
            vec![43]
        );
        assert!(
            load_suppressed_roles(&conn, 2, connect_bit)
                .unwrap()
                .is_empty()
        );
        save_suppressed_roles(&conn, channel, connect_bit, &[]).unwrap();
        assert!(
            load_suppressed_roles(&conn, channel, connect_bit)
                .unwrap()
                .is_empty()
        );
        assert!(delete(&conn, channel).unwrap());
        assert!(
            load_suppressed_roles(&conn, channel, view_bit)
                .unwrap()
                .is_empty()
        );
    }
}
