//! Stores temporary voice rooms, preferences, usage and scheduled events.

use std::path::Path;
use std::sync::OnceLock;

use chrono::{DateTime, Datelike, Months, NaiveDateTime, NaiveTime};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};

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
const STAGE3_SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS role_perks (
        guild_id INTEGER NOT NULL CHECK (guild_id <> 0),
        role_id INTEGER NOT NULL CHECK (role_id <> 0),
        user_limit INTEGER NOT NULL CHECK (user_limit BETWEEN 0 AND 99),
        bitrate INTEGER NOT NULL CHECK (bitrate BETWEEN 8000 AND 384000),
        PRIMARY KEY (guild_id, role_id)
    );
    CREATE TABLE IF NOT EXISTS hub_templates (
        guild_id INTEGER NOT NULL CHECK (guild_id <> 0),
        channel_id INTEGER NOT NULL CHECK (channel_id <> 0),
        name_pattern TEXT CHECK (length(name_pattern) BETWEEN 1 AND 100),
        user_limit INTEGER CHECK (user_limit BETWEEN 0 AND 99),
        bitrate INTEGER CHECK (bitrate BETWEEN 8000 AND 384000),
        category_id INTEGER CHECK (category_id <> 0),
        required_role_id INTEGER CHECK (required_role_id <> 0),
        PRIMARY KEY (guild_id, channel_id)
    );
    CREATE TABLE IF NOT EXISTS voice_stats (
        guild_id INTEGER NOT NULL CHECK (guild_id <> 0),
        user_id INTEGER NOT NULL CHECK (user_id <> 0),
        month INTEGER NOT NULL,
        time_ms INTEGER NOT NULL CHECK (time_ms >= 0),
        rooms_created INTEGER NOT NULL CHECK (rooms_created >= 0),
        PRIMARY KEY (guild_id, user_id, month)
    );
    CREATE TABLE IF NOT EXISTS schedules (
        id INTEGER PRIMARY KEY,
        guild_id INTEGER NOT NULL CHECK (guild_id <> 0),
        creator_id INTEGER NOT NULL CHECK (creator_id <> 0),
        run_at INTEGER NOT NULL,
        name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
        user_limit INTEGER CHECK (user_limit BETWEEN 0 AND 99),
        role_id INTEGER CHECK (role_id <> 0),
        hub_id INTEGER CHECK (hub_id <> 0),
        status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'open', 'missed', 'cancelled')),
        channel_id INTEGER UNIQUE CHECK (channel_id <> 0),
        opened_at INTEGER,
        announced INTEGER NOT NULL DEFAULT 0 CHECK (announced IN (0, 1)),
        CHECK ((status = 'open') = (channel_id IS NOT NULL)),
        CHECK ((status = 'open') = (opened_at IS NOT NULL))
    );
    CREATE INDEX IF NOT EXISTS schedules_due ON schedules (guild_id, status, run_at);
";
const ROOM_COLUMNS: &str = "channel_id, guild_id, hub_id, owner_id";
const SCHEDULE_COLUMNS: &str =
    "id, guild_id, creator_id, run_at, name, user_limit, role_id, hub_id, status, channel_id";

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
    let mut conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000; PRAGMA foreign_keys=ON;",
    )?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch(ROOM_SCHEMA)?;
    tx.execute_batch(PROFILE_SCHEMA)?;
    tx.execute_batch(STAGE3_SCHEMA)?;
    let has_rename_actor = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info('room_renames') WHERE name = 'actor_id')",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !has_rename_actor {
        tx.execute(
            "ALTER TABLE room_renames ADD COLUMN actor_id INTEGER CHECK (actor_id <> 0)",
            [],
        )?;
    }
    tx.commit()?;
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

fn optional_id(row: &Row<'_>, column: usize) -> rusqlite::Result<Option<u64>> {
    Ok(row.get::<_, Option<i64>>(column)?.map(|id| id as u64))
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

/// The two most recent renames in UTC Unix milliseconds, and the queued name and requester.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rename {
    pub previous_ms: Option<i64>,
    pub latest_ms: Option<i64>,
    pub pending: Option<String>,
    pub actor_id: Option<u64>,
}

pub fn rename(conn: &Connection, channel_id: u64) -> rusqlite::Result<Option<Rename>> {
    conn.query_row(
        "SELECT previous_ms, latest_ms, pending, actor_id FROM room_renames WHERE channel_id = ?1",
        [channel_id as i64],
        |row| {
            Ok(Rename {
                previous_ms: row.get(0)?,
                latest_ms: row.get(1)?,
                pending: row.get(2)?,
                actor_id: optional_id(row, 3)?,
            })
        },
    )
    .optional()
}

pub fn save_rename(conn: &Connection, channel_id: u64, rename: &Rename) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO room_renames (channel_id, previous_ms, latest_ms, pending, actor_id) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(channel_id) DO UPDATE SET previous_ms = excluded.previous_ms,
             latest_ms = excluded.latest_ms, pending = excluded.pending, actor_id = excluded.actor_id",
        params![
            channel_id as i64,
            rename.previous_ms,
            rename.latest_ms,
            rename.pending,
            rename.actor_id.map(|id| id as i64),
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HubTemplate {
    pub channel_id: u64,
    pub name_pattern: Option<String>,
    pub limit: Option<u32>,
    pub bitrate: Option<u32>,
    pub category_id: Option<u64>,
    pub required_role_id: Option<u64>,
}

fn hub_template_row(row: &Row<'_>) -> rusqlite::Result<HubTemplate> {
    Ok(HubTemplate {
        channel_id: row.get::<_, i64>(0)? as u64,
        name_pattern: row.get(1)?,
        limit: row.get(2)?,
        bitrate: row.get(3)?,
        category_id: optional_id(row, 4)?,
        required_role_id: optional_id(row, 5)?,
    })
}

pub fn hub_template(
    conn: &Connection,
    guild_id: u64,
    channel_id: u64,
) -> rusqlite::Result<Option<HubTemplate>> {
    conn.query_row(
        "SELECT channel_id, name_pattern, user_limit, bitrate, category_id, required_role_id
         FROM hub_templates WHERE guild_id = ?1 AND channel_id = ?2",
        params![guild_id as i64, channel_id as i64],
        hub_template_row,
    )
    .optional()
}

pub fn hub_templates(conn: &Connection, guild_id: u64) -> rusqlite::Result<Vec<HubTemplate>> {
    let mut stmt = conn.prepare_cached(
        "SELECT channel_id, name_pattern, user_limit, bitrate, category_id, required_role_id
         FROM hub_templates WHERE guild_id = ?1 ORDER BY channel_id",
    )?;
    stmt.query_map([guild_id as i64], hub_template_row)?
        .collect()
}

pub fn save_hub_template(
    conn: &Connection,
    guild_id: u64,
    hub: &HubTemplate,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO hub_templates (guild_id, channel_id, name_pattern, user_limit, bitrate, category_id, required_role_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (guild_id, channel_id) DO UPDATE SET name_pattern = excluded.name_pattern,
             user_limit = excluded.user_limit, bitrate = excluded.bitrate,
             category_id = excluded.category_id, required_role_id = excluded.required_role_id",
        params![
            guild_id as i64,
            hub.channel_id as i64,
            hub.name_pattern,
            hub.limit,
            hub.bitrate,
            hub.category_id.map(|id| id as i64),
            hub.required_role_id.map(|id| id as i64),
        ],
    )?;
    Ok(())
}

pub fn remove_hub_template(
    conn: &Connection,
    guild_id: u64,
    channel_id: u64,
) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "DELETE FROM hub_templates WHERE guild_id = ?1 AND channel_id = ?2",
        params![guild_id as i64, channel_id as i64],
    )? != 0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RolePerk {
    pub role_id: u64,
    pub limit: u32,
    pub bitrate: u32,
}

pub fn role_perks(conn: &Connection, guild_id: u64) -> rusqlite::Result<Vec<RolePerk>> {
    let mut stmt = conn.prepare_cached(
        "SELECT role_id, user_limit, bitrate FROM role_perks WHERE guild_id = ?1 ORDER BY role_id",
    )?;
    stmt.query_map([guild_id as i64], |row| {
        Ok(RolePerk {
            role_id: row.get::<_, i64>(0)? as u64,
            limit: row.get(1)?,
            bitrate: row.get(2)?,
        })
    })?
    .collect()
}

pub fn save_role_perk(conn: &Connection, guild_id: u64, perk: &RolePerk) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO role_perks (guild_id, role_id, user_limit, bitrate) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (guild_id, role_id) DO UPDATE SET user_limit = excluded.user_limit,
             bitrate = excluded.bitrate",
        params![
            guild_id as i64,
            perk.role_id as i64,
            perk.limit,
            perk.bitrate
        ],
    )?;
    Ok(())
}

pub fn remove_role_perk(conn: &Connection, guild_id: u64, role_id: u64) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "DELETE FROM role_perks WHERE guild_id = ?1 AND role_id = ?2",
        params![guild_id as i64, role_id as i64],
    )? != 0)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VoiceStats {
    pub time_ms: u64,
    pub rooms_created: u64,
}

const IST_OFFSET_MS: i64 = 19_800_000;

fn ist_time(ms: i64) -> rusqlite::Result<NaiveDateTime> {
    ms.checked_add(IST_OFFSET_MS)
        .and_then(DateTime::from_timestamp_millis)
        .map(|local| local.naive_utc())
        .ok_or(rusqlite::Error::IntegralValueOutOfRange(0, ms))
}

fn month_of(date: NaiveDateTime) -> i64 {
    i64::from(date.year()) * 100 + i64::from(date.month())
}

pub fn month_key(now_ms: i64) -> rusqlite::Result<i64> {
    Ok(month_of(ist_time(now_ms)?))
}

fn accumulate_stats(
    conn: &Connection,
    guild_id: u64,
    user_id: u64,
    month: i64,
    time_ms: i64,
    rooms_created: u32,
) -> rusqlite::Result<()> {
    conn.prepare_cached(
        "INSERT INTO voice_stats (guild_id, user_id, month, time_ms, rooms_created)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(guild_id, user_id, month) DO UPDATE SET
             time_ms = voice_stats.time_ms + excluded.time_ms,
             rooms_created = voice_stats.rooms_created + excluded.rooms_created",
    )?
    .execute(params![
        guild_id as i64,
        user_id as i64,
        month,
        time_ms,
        rooms_created
    ])?;
    Ok(())
}

pub fn record_room_created(
    conn: &Connection,
    guild_id: u64,
    user_id: u64,
    now_ms: i64,
) -> rusqlite::Result<()> {
    accumulate_stats(conn, guild_id, user_id, month_key(now_ms)?, 0, 1)
}

/// Credits [start_ms, end_ms) and assigns each part to its India-time calendar month.
pub fn add_voice_time(
    conn: &Connection,
    guild_id: u64,
    user_id: u64,
    mut start_ms: i64,
    end_ms: i64,
) -> rusqlite::Result<()> {
    if start_ms >= end_ms {
        return Ok(());
    }
    // Each flush either records all its months or can be retried as a unit.
    let tx = conn
        .is_autocommit()
        .then(|| conn.unchecked_transaction())
        .transpose()?;
    let conn = tx.as_deref().unwrap_or(conn);
    let last_month = month_key(end_ms - 1)?;
    while start_ms < end_ms {
        let local = ist_time(start_ms)?;
        let month = month_of(local);
        let end = if month == last_month {
            end_ms
        } else {
            local
                .date()
                .with_day(1)
                .and_then(|date| date.checked_add_months(Months::new(1)))
                .ok_or(rusqlite::Error::IntegralValueOutOfRange(0, start_ms))?
                .and_time(NaiveTime::MIN)
                .and_utc()
                .timestamp_millis()
                - IST_OFFSET_MS
        };
        accumulate_stats(conn, guild_id, user_id, month, end - start_ms, 0)?;
        start_ms = end;
    }
    if let Some(tx) = tx {
        tx.commit()?;
    }
    Ok(())
}

pub fn stats(
    conn: &Connection,
    guild_id: u64,
    user_id: u64,
    month: Option<i64>,
) -> rusqlite::Result<VoiceStats> {
    conn.query_row(
        "SELECT COALESCE(SUM(time_ms), 0), COALESCE(SUM(rooms_created), 0) FROM voice_stats
         WHERE guild_id = ?1 AND user_id = ?2 AND (?3 IS NULL OR month = ?3)",
        params![guild_id as i64, user_id as i64, month],
        |row| {
            Ok(VoiceStats {
                time_ms: row.get::<_, i64>(0)? as u64,
                rooms_created: row.get::<_, i64>(1)? as u64,
            })
        },
    )
}

pub fn top_stats(
    conn: &Connection,
    guild_id: u64,
    month: Option<i64>,
) -> rusqlite::Result<Vec<(u64, VoiceStats)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT user_id, SUM(time_ms) AS duration, SUM(rooms_created) FROM voice_stats
         WHERE guild_id = ?1 AND (?2 IS NULL OR month = ?2) GROUP BY user_id
         HAVING duration > 0 ORDER BY duration DESC, user_id LIMIT 10",
    )?;
    stmt.query_map(params![guild_id as i64, month], |row| {
        Ok((
            row.get::<_, i64>(0)? as u64,
            VoiceStats {
                time_ms: row.get::<_, i64>(1)? as u64,
                rooms_created: row.get::<_, i64>(2)? as u64,
            },
        ))
    })?
    .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScheduleStatus {
    Pending,
    Open,
    Missed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Schedule {
    pub id: i64,
    pub guild_id: u64,
    pub creator_id: u64,
    /// UTC Unix seconds.
    pub run_at: i64,
    pub name: String,
    pub limit: Option<u32>,
    pub role_id: Option<u64>,
    pub hub_id: Option<u64>,
    pub status: ScheduleStatus,
    pub channel_id: Option<u64>,
}

pub struct NewSchedule<'a> {
    pub guild_id: u64,
    pub creator_id: u64,
    pub run_at: i64,
    pub name: &'a str,
    pub limit: Option<u32>,
    pub role_id: Option<u64>,
    pub hub_id: Option<u64>,
}

fn schedule_row(row: &Row<'_>) -> rusqlite::Result<Schedule> {
    let status = match row.get::<_, String>(8)?.as_str() {
        "pending" => ScheduleStatus::Pending,
        "open" => ScheduleStatus::Open,
        "missed" => ScheduleStatus::Missed,
        "cancelled" => ScheduleStatus::Cancelled,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    Ok(Schedule {
        id: row.get(0)?,
        guild_id: row.get::<_, i64>(1)? as u64,
        creator_id: row.get::<_, i64>(2)? as u64,
        run_at: row.get(3)?,
        name: row.get(4)?,
        limit: row.get(5)?,
        role_id: optional_id(row, 6)?,
        hub_id: optional_id(row, 7)?,
        status,
        channel_id: optional_id(row, 9)?,
    })
}

pub fn add_schedule(conn: &Connection, schedule: &NewSchedule<'_>) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO schedules (guild_id, creator_id, run_at, name, user_limit, role_id, hub_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            schedule.guild_id as i64,
            schedule.creator_id as i64,
            schedule.run_at,
            schedule.name,
            schedule.limit,
            schedule.role_id.map(|id| id as i64),
            schedule.hub_id.map(|id| id as i64),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn list_schedules(conn: &Connection, guild_id: u64) -> rusqlite::Result<Vec<Schedule>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {SCHEDULE_COLUMNS} FROM schedules WHERE guild_id = ?1
         ORDER BY status <> 'pending', CASE WHEN status = 'pending' THEN run_at END, run_at DESC, id"
    ))?;
    stmt.query_map([guild_id as i64], schedule_row)?.collect()
}

pub fn due_schedules(
    conn: &Connection,
    guild_id: u64,
    now: i64,
) -> rusqlite::Result<Vec<Schedule>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {SCHEDULE_COLUMNS} FROM schedules WHERE guild_id = ?1 AND status = 'pending'
         AND run_at <= ?2 ORDER BY run_at, id"
    ))?;
    stmt.query_map(params![guild_id as i64, now], schedule_row)?
        .collect()
}

pub fn cancel_schedule(
    conn: &Connection,
    guild_id: u64,
    id: i64,
    creator: Option<u64>,
) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "UPDATE schedules SET status = 'cancelled' WHERE guild_id = ?1 AND id = ?2
         AND status = 'pending' AND (?3 IS NULL OR creator_id = ?3)",
        params![guild_id as i64, id, creator.map(|id| id as i64)],
    )? != 0)
}

pub fn mark_schedule_missed(conn: &Connection, guild_id: u64, id: i64) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "UPDATE schedules SET status = 'missed' WHERE guild_id = ?1 AND id = ?2 AND status = 'pending'",
        params![guild_id as i64, id],
    )? != 0)
}

/// Call in the same transaction that records a freshly-created Discord channel.
pub fn mark_schedule_open(
    conn: &Connection,
    guild_id: u64,
    id: i64,
    channel_id: u64,
    opened_at: i64,
) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "UPDATE schedules SET status = 'open', channel_id = ?3, opened_at = ?4
         WHERE guild_id = ?1 AND id = ?2 AND status = 'pending'
         AND EXISTS (SELECT 1 FROM rooms WHERE rooms.channel_id = ?3 AND rooms.guild_id = ?1
                     AND rooms.owner_id = schedules.creator_id)",
        params![guild_id as i64, id, channel_id as i64, opened_at],
    )? != 0)
}

pub fn room_schedule_opened_at(
    conn: &Connection,
    guild_id: u64,
    channel_id: u64,
) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT opened_at FROM schedules WHERE guild_id = ?1 AND channel_id = ?2 AND status = 'open'",
        params![guild_id as i64, channel_id as i64],
        |row| row.get(0),
    )
    .optional()
}

pub fn unannounced_schedules(
    conn: &Connection,
    guild_id: u64,
    min_opened_at: i64,
) -> rusqlite::Result<Vec<Schedule>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {SCHEDULE_COLUMNS} FROM schedules WHERE guild_id = ?1 AND status = 'open'
         AND announced = 0 AND opened_at >= ?2
         AND EXISTS (SELECT 1 FROM rooms WHERE rooms.channel_id = schedules.channel_id)
         ORDER BY opened_at, id"
    ))?;
    stmt.query_map(params![guild_id as i64, min_opened_at], schedule_row)?
        .collect()
}

pub fn mark_schedule_announced(conn: &Connection, guild_id: u64, id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE schedules SET announced = 1 WHERE guild_id = ?1 AND id = ?2 AND status = 'open'",
        params![guild_id as i64, id],
    )?;
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
        conn.execute(
            "INSERT INTO room_renames (channel_id, pending) VALUES (?1, ?2)",
            params![managed.channel_id as i64, queued.pending],
        )
        .unwrap();
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
            actor_id: Some(31),
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

    #[test]
    fn hub_templates_and_perks_survive_reopen_and_are_guild_scoped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tempvc.db");
        let conn = connect(&path).unwrap();
        let hub = HubTemplate {
            channel_id: 20,
            name_pattern: Some("{user} 🎧".into()),
            limit: Some(0),
            bitrate: Some(96_000),
            category_id: Some(31),
            required_role_id: Some(41),
        };
        let perk = RolePerk {
            role_id: 41,
            limit: 22,
            bitrate: 160_000,
        };
        assert_eq!(hub_template(&conn, 10, 20).unwrap(), None);
        save_hub_template(&conn, 10, &hub).unwrap();
        save_role_perk(&conn, 10, &perk).unwrap();
        drop(conn);

        let conn = connect(&path).unwrap();
        assert_eq!(hub_template(&conn, 10, 20).unwrap(), Some(hub.clone()));
        assert_eq!(hub_templates(&conn, 10).unwrap(), [hub]);
        assert_eq!(role_perks(&conn, 10).unwrap(), [perk]);
        assert!(hub_templates(&conn, 11).unwrap().is_empty());
        assert!(role_perks(&conn, 11).unwrap().is_empty());
        assert!(!remove_hub_template(&conn, 11, 20).unwrap());
        assert!(!remove_role_perk(&conn, 11, 41).unwrap());

        let updated_hub = HubTemplate {
            channel_id: 20,
            limit: Some(12),
            ..HubTemplate::default()
        };
        let updated_perk = RolePerk { limit: 44, ..perk };
        save_hub_template(&conn, 10, &updated_hub).unwrap();
        save_role_perk(&conn, 10, &updated_perk).unwrap();
        assert_eq!(hub_templates(&conn, 10).unwrap(), [updated_hub]);
        assert_eq!(role_perks(&conn, 10).unwrap(), [updated_perk]);
        assert!(remove_hub_template(&conn, 10, 20).unwrap());
        assert!(remove_role_perk(&conn, 10, 41).unwrap());
        assert!(hub_templates(&conn, 10).unwrap().is_empty());
        assert!(role_perks(&conn, 10).unwrap().is_empty());
    }

    #[test]
    fn usage_accumulates_across_ist_months_and_sorts_top_ten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tempvc.db");
        let conn = connect(&path).unwrap();
        let new_year = DateTime::parse_from_rfc3339("2027-01-01T00:00:00+05:30")
            .unwrap()
            .timestamp_millis();
        assert_eq!(month_key(new_year - 1).unwrap(), 202612);
        assert_eq!(month_key(new_year).unwrap(), 202701);

        add_voice_time(&conn, 10, 30, new_year - 250, new_year + 750).unwrap();
        add_voice_time(&conn, 10, 30, new_year + 750, new_year + 1_750).unwrap();
        add_voice_time(&conn, 10, 30, new_year + 750, new_year).unwrap();
        record_room_created(&conn, 10, 30, new_year - 1).unwrap();
        record_room_created(&conn, 10, 30, new_year).unwrap();
        record_room_created(&conn, 10, 45, new_year).unwrap();
        add_voice_time(&conn, 11, 30, new_year, new_year + 60_000).unwrap();
        for user_id in 31..42 {
            add_voice_time(&conn, 10, user_id, new_year, new_year + 1_750).unwrap();
        }
        drop(conn);

        let conn = connect(&path).unwrap();
        assert_eq!(
            stats(&conn, 10, 30, None).unwrap(),
            VoiceStats {
                time_ms: 2_000,
                rooms_created: 2,
            },
        );
        assert_eq!(
            stats(&conn, 10, 30, Some(202612)).unwrap(),
            VoiceStats {
                time_ms: 250,
                rooms_created: 1,
            },
        );
        assert_eq!(
            stats(&conn, 10, 30, Some(202701)).unwrap(),
            VoiceStats {
                time_ms: 1_750,
                rooms_created: 1,
            },
        );
        assert_eq!(stats(&conn, 10, 43, None).unwrap(), VoiceStats::default());
        assert_eq!(top_stats(&conn, 10, Some(202612)).unwrap().len(), 1);
        let top = top_stats(&conn, 10, Some(202701)).unwrap();
        assert_eq!(
            top.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            (30..40).collect::<Vec<_>>(),
        );
        assert_eq!(
            top_stats(&conn, 10, None).unwrap()[0],
            (
                30,
                VoiceStats {
                    time_ms: 2_000,
                    rooms_created: 2
                }
            ),
        );
    }

    #[test]
    fn schedule_state_and_announcement_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tempvc.db");
        let conn = connect(&path).unwrap();
        let schedule = NewSchedule {
            guild_id: 10,
            creator_id: 30,
            run_at: 1_000,
            name: "Game night",
            limit: Some(12),
            role_id: Some(40),
            hub_id: Some(20),
        };
        let opened = add_schedule(&conn, &schedule).unwrap();
        let missed = add_schedule(
            &conn,
            &NewSchedule {
                run_at: 900,
                ..schedule
            },
        )
        .unwrap();
        let cancelled = add_schedule(
            &conn,
            &NewSchedule {
                run_at: 1_100,
                ..schedule
            },
        )
        .unwrap();
        assert_eq!(
            due_schedules(&conn, 10, 1_000)
                .unwrap()
                .iter()
                .map(|event| event.id)
                .collect::<Vec<_>>(),
            [missed, opened],
        );
        assert!(due_schedules(&conn, 11, 1_200).unwrap().is_empty());
        assert!(!cancel_schedule(&conn, 10, cancelled, Some(31)).unwrap());
        assert!(!cancel_schedule(&conn, 11, cancelled, None).unwrap());
        assert!(cancel_schedule(&conn, 10, cancelled, Some(30)).unwrap());
        assert!(mark_schedule_missed(&conn, 10, missed).unwrap());
        assert!(!mark_schedule_missed(&conn, 10, cancelled).unwrap());
        assert!(!mark_schedule_open(&conn, 10, opened, 50, 1_010).unwrap());
        drop(conn);

        let mut conn = connect(&path).unwrap();
        assert_eq!(due_schedules(&conn, 10, 1_200).unwrap()[0].id, opened);
        let managed = Room {
            channel_id: 50,
            guild_id: 10,
            hub_id: 20,
            owner_id: 30,
        };
        let tx = conn.transaction().unwrap();
        insert(&tx, &managed).unwrap();
        assert!(!mark_schedule_open(&tx, 10, cancelled, 50, 1_010).unwrap());
        assert!(mark_schedule_open(&tx, 10, opened, 50, 1_010).unwrap());
        tx.rollback().unwrap();
        assert_eq!(room(&conn, 50).unwrap(), None);
        assert_eq!(room_schedule_opened_at(&conn, 10, 50).unwrap(), None);

        let tx = conn.transaction().unwrap();
        insert(&tx, &managed).unwrap();
        assert!(mark_schedule_open(&tx, 10, opened, 50, 1_010).unwrap());
        tx.commit().unwrap();
        assert!(!mark_schedule_open(&conn, 10, opened, 50, 1_020).unwrap());
        assert!(!cancel_schedule(&conn, 10, opened, None).unwrap());
        drop(conn);

        let conn = connect(&path).unwrap();
        let listing = list_schedules(&conn, 10).unwrap();
        assert_eq!(
            listing.iter().map(|event| event.status).collect::<Vec<_>>(),
            [
                ScheduleStatus::Cancelled,
                ScheduleStatus::Open,
                ScheduleStatus::Missed
            ],
        );
        assert_eq!(listing[1].channel_id, Some(50));
        assert_eq!(listing[1].role_id, Some(40));
        assert_eq!(listing[1].hub_id, Some(20));
        assert_eq!(room_schedule_opened_at(&conn, 10, 50).unwrap(), Some(1_010));
        assert!(unannounced_schedules(&conn, 10, 1_011).unwrap().is_empty());
        assert_eq!(
            unannounced_schedules(&conn, 10, 1_010).unwrap(),
            [listing[1].clone()],
        );
        mark_schedule_announced(&conn, 10, opened).unwrap();
        assert!(unannounced_schedules(&conn, 10, 0).unwrap().is_empty());
        delete(&conn, 50).unwrap();
        assert_eq!(list_schedules(&conn, 10).unwrap(), listing);
    }
}
