//! Activity counts behind /awards.
//!
//! Kept in its own SQLite file beside vizier.db: high-volume counters that
//! have nothing to do with conversations and must outlive a history reset.
//!
//! Each allowed channel remembers how far its history has been counted. On
//! startup the gap between that mark and the moment this run began is fetched
//! and counted - the whole history on the very first run, whatever was missed
//! while the bot was down on every run after. From that moment on messages are
//! counted as they arrive. The id ranges counted live are recorded, so a
//! restart part-way through an import never counts a message twice.
//!
//! Voice comes only from Dyno's channel log, which records every room -
//! including private and temporary ones the bot itself cannot see.

use std::sync::{Arc, LazyLock, OnceLock};
use std::time::Duration;

use chrono::{DateTime, FixedOffset, Timelike};
use parking_lot::Mutex;
use regex::Regex;
use rusqlite::{params, Connection};
use serenity::all::{ChannelId, GetMessages, Http, Message, MessageId};

const DISCORD_EPOCH_MS: i64 = 1_420_070_400_000;

static DB: OnceLock<Arc<Mutex<Connection>>> = OnceLock::new();
/// First message id that belongs to this run. Anything older is the import's.
static BOUNDARY: OnceLock<u64> = OnceLock::new();
/// The channels this run counts, for reporting import progress.
static CHANNELS: OnceLock<Vec<u64>> = OnceLock::new();

pub fn ist() -> FixedOffset {
    FixedOffset::east_opt(5 * 3600 + 30 * 60).expect("valid offset")
}

/// Calendar day and hour in India, where the server lives.
fn bucket(unix: i64) -> (String, u32) {
    let t = DateTime::from_timestamp(unix, 0)
        .unwrap_or_default()
        .with_timezone(&ist());
    (t.format("%Y-%m-%d").to_string(), t.hour())
}

fn snowflake_now() -> u64 {
    let ms = chrono::Utc::now().timestamp_millis();
    ((ms - DISCORD_EPOCH_MS) as u64) << 22
}

pub fn db() -> Option<Arc<Mutex<Connection>>> {
    DB.get().cloned()
}

/// How many allowed channels have had their whole history counted.
pub fn history_progress() -> (usize, usize) {
    let (Some(db), Some(channels)) = (DB.get(), CHANNELS.get()) else {
        return (0, 0);
    };
    let conn = db.lock();
    let done = channels
        .iter()
        .filter(|&&c| {
            conn.query_row("SELECT done FROM hist_marks WHERE channel_id = ?1", params![c as i64], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap_or(0)
                == 1
        })
        .count();
    (done, channels.len())
}

/// Whether Dyno's voice log has been read to the end at least once.
pub fn voice_caught_up() -> bool {
    DB.get().is_some_and(|db| {
        db.lock()
            .query_row("SELECT 1 FROM meta WHERE key = 'voice_log_caught_up'", [], |_| Ok(()))
            .is_ok()
    })
}

pub fn is_open() -> bool {
    DB.get().is_some()
}

pub fn open(workspace: &str, channels: &[u64]) -> anyhow::Result<()> {
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    std::fs::create_dir_all(&dir)?;
    let conn = Connection::open(dir.join("stats.db"))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA busy_timeout=5000;
         CREATE TABLE IF NOT EXISTS msg_counts (
             user_id INTEGER NOT NULL, channel_id INTEGER NOT NULL,
             day TEXT NOT NULL, hour INTEGER NOT NULL, count INTEGER NOT NULL,
             PRIMARY KEY (user_id, channel_id, day, hour)) WITHOUT ROWID;
         CREATE TABLE IF NOT EXISTS hist_marks (
             channel_id INTEGER PRIMARY KEY, upto INTEGER NOT NULL,
             done INTEGER NOT NULL DEFAULT 0);
         CREATE TABLE IF NOT EXISTS live_ranges (
             channel_id INTEGER NOT NULL, lo INTEGER NOT NULL, hi INTEGER NOT NULL,
             PRIMARY KEY (channel_id, lo));
         CREATE TABLE IF NOT EXISTS voice_events (
             msg_id INTEGER NOT NULL, user_id INTEGER NOT NULL, action TEXT NOT NULL,
             channel_id INTEGER NOT NULL, ts INTEGER NOT NULL,
             PRIMARY KEY (msg_id, user_id, action));
         CREATE INDEX IF NOT EXISTS voice_events_user_ts ON voice_events (user_id, ts);
         CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    )?;
    let boundary = snowflake_now();
    for &channel in channels {
        conn.execute(
            "INSERT OR IGNORE INTO live_ranges (channel_id, lo, hi) VALUES (?1, ?2, ?2)",
            params![channel as i64, boundary as i64],
        )?;
    }
    let _ = BOUNDARY.set(boundary);
    let _ = CHANNELS.set(channels.to_vec());
    let _ = DB.set(Arc::new(Mutex::new(conn)));
    Ok(())
}

fn add_message(conn: &Connection, user: u64, channel: u64, unix: i64) -> rusqlite::Result<()> {
    let (day, hour) = bucket(unix);
    conn.execute(
        "INSERT INTO msg_counts (user_id, channel_id, day, hour, count) VALUES (?1, ?2, ?3, ?4, 1)
         ON CONFLICT (user_id, channel_id, day, hour) DO UPDATE SET count = count + 1",
        params![user as i64, channel as i64, day, hour],
    )?;
    Ok(())
}

/// Count a message the moment it arrives. The caller has already dropped
/// bots, DMs and channels outside the allowlist.
pub fn count_live(msg: &Message) {
    let (Some(db), Some(&boundary)) = (DB.get(), BOUNDARY.get()) else {
        return;
    };
    let id = msg.id.get();
    if id < boundary {
        return;
    }
    let channel = msg.channel_id.get();
    let conn = db.lock();
    let result = add_message(&conn, msg.author.id.get(), channel, msg.timestamp.unix_timestamp())
        .and_then(|_| {
            conn.execute(
                "UPDATE live_ranges SET hi = max(hi, ?1) WHERE channel_id = ?2 AND lo = ?3",
                params![id as i64, channel as i64, boundary as i64],
            )
            .map(|_| ())
        });
    if let Err(err) = result {
        tracing::warn!("stats: could not count message {}: {}", id, err);
    }
}

fn load_marks(conn: &Connection, channel: u64, boundary: u64) -> rusqlite::Result<(u64, Vec<(u64, u64)>)> {
    let upto: i64 = conn
        .query_row(
            "SELECT upto FROM hist_marks WHERE channel_id = ?1",
            params![channel as i64],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let mut stmt = conn.prepare("SELECT lo, hi FROM live_ranges WHERE channel_id = ?1 AND lo < ?2")?;
    let ranges = stmt
        .query_map(params![channel as i64, boundary as i64], |r| {
            Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64))
        })?
        .flatten()
        .collect();
    Ok((upto as u64, ranges))
}

/// One page of history, counted and marked in a single transaction so a crash
/// between pages can neither lose nor repeat one.
fn write_page(
    conn: &mut Connection,
    channel: u64,
    page: &[Message],
    boundary: u64,
    earlier: &[(u64, u64)],
    upto: u64,
    finished: bool,
) -> rusqlite::Result<u64> {
    let tx = conn.transaction()?;
    let mut counted = 0;
    for m in page {
        let id = m.id.get();
        if id >= boundary || m.author.bot {
            continue;
        }
        // Counted live by an earlier run while the bot was up.
        if earlier.iter().any(|&(lo, hi)| id >= lo && id <= hi) {
            continue;
        }
        add_message(&tx, m.author.id.get(), channel, m.timestamp.unix_timestamp())?;
        counted += 1;
    }
    tx.execute(
        "INSERT INTO hist_marks (channel_id, upto, done) VALUES (?1, ?2, ?3)
         ON CONFLICT (channel_id) DO UPDATE SET upto = excluded.upto, done = max(done, excluded.done)",
        params![channel as i64, upto as i64, finished as i64],
    )?;
    if finished {
        // Everything before this run is now counted, so the old live ranges
        // have nothing left to protect.
        tx.execute(
            "DELETE FROM live_ranges WHERE channel_id = ?1 AND lo < ?2",
            params![channel as i64, boundary as i64],
        )?;
    }
    tx.commit()?;
    Ok(counted)
}

/// Count everything in `channel` from its mark up to the start of this run.
pub async fn catch_up(http: Arc<Http>, channel: u64) {
    let (Some(db), Some(&boundary)) = (DB.get(), BOUNDARY.get()) else {
        return;
    };
    let (mut upto, earlier) = match load_marks(&db.lock(), channel, boundary) {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!("stats: cannot read marks for channel {}: {}", channel, err);
            return;
        }
    };
    if upto + 1 >= boundary {
        return;
    }
    let started = std::time::Instant::now();
    let (mut pages, mut counted, mut failures) = (0u32, 0u64, 0u32);
    loop {
        let builder = GetMessages::new().after(MessageId::new(upto.max(1))).limit(100);
        let mut page = match ChannelId::new(channel).messages(&*http, builder).await {
            Ok(p) => {
                failures = 0;
                p
            }
            Err(err) => {
                let text = err.to_string();
                if text.contains("Missing Access") || text.contains("Missing Permissions") {
                    tracing::warn!("stats: no access to history of channel {}: {}", channel, text);
                    // Nothing readable will ever come from it; do not leave
                    // the progress note waiting on it forever.
                    let _ = db.lock().execute(
                        "INSERT INTO hist_marks (channel_id, upto, done) VALUES (?1, ?2, 1)
                         ON CONFLICT (channel_id) DO UPDATE SET done = 1",
                        params![channel as i64, upto as i64],
                    );
                    return;
                }
                failures += 1;
                if failures > 8 {
                    tracing::warn!("stats: giving up on channel {} for this run: {}", channel, text);
                    return;
                }
                tokio::time::sleep(Duration::from_secs(5 * failures as u64)).await;
                continue;
            }
        };
        page.sort_by_key(|m| m.id.get());
        let last = page.last().map(|m| m.id.get());
        // A short page means nothing else exists before this run began.
        let finished = page.len() < 100 || last.is_none_or(|l| l >= boundary);
        let new_upto = if finished { boundary - 1 } else { last.unwrap_or(upto) };
        let written = write_page(&mut db.lock(), channel, &page, boundary, &earlier, new_upto, finished);
        match written {
            Ok(n) => counted += n,
            Err(err) => {
                tracing::warn!("stats: write failed for channel {}: {}", channel, err);
                return;
            }
        }
        upto = new_upto;
        pages += 1;
        if pages % 50 == 0 {
            tracing::info!("stats: channel {} - {} pages, {} messages so far", channel, pages, counted);
        }
        if finished {
            break;
        }
    }
    tracing::info!(
        "stats: channel {} caught up - {} messages counted in {:?}",
        channel,
        counted,
        started.elapsed()
    );
}

static VOICE_EVENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)<@!?(\d+)>\s+(joined|left)\s+voice channel\s+<#(\d+)>").expect("regex")
});
static VOICE_SWITCH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)<@!?(\d+)>[\s*]+switched voice channels?[\s*`]+<#(\d+)>[\s*`]*(?:-+>|→)[\s*`]*<#(\d+)>")
        .expect("regex")
});

/// One Dyno log line: (user, joined | left | switched, room). For a switch
/// the room is where they went.
fn parse_voice_line(text: &str) -> Option<(u64, &'static str, u64)> {
    if let Some(c) = VOICE_SWITCH.captures(text) {
        return Some((c[1].parse().ok()?, "switched", c[3].parse().ok()?));
    }
    let c = VOICE_EVENT.captures(text)?;
    let action = if c[2].eq_ignore_ascii_case("joined") { "joined" } else { "left" };
    Some((c[1].parse().ok()?, action, c[3].parse().ok()?))
}

fn write_voice_page(conn: &mut Connection, page: &[Message], upto: u64) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    for m in page {
        // Only a logging bot's embeds are records - anyone can type the words.
        if !m.author.bot {
            continue;
        }
        for e in &m.embeds {
            let Some(text) = e.description.as_deref() else { continue };
            let Some((user, action, room)) = parse_voice_line(text) else { continue };
            let ts = e.timestamp.unwrap_or(m.timestamp).unix_timestamp();
            tx.execute(
                "INSERT OR IGNORE INTO voice_events (msg_id, user_id, action, channel_id, ts)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![m.id.get() as i64, user as i64, action, room as i64, ts],
            )?;
        }
    }
    tx.execute(
        "INSERT INTO meta (key, value) VALUES ('voice_log_upto', ?1)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        params![upto.to_string()],
    )?;
    tx.commit()
}

/// Read Dyno's voice log from where we left off, then keep following it.
pub async fn follow_voice_log(http: Arc<Http>, log_channel: u64) {
    let Some(db) = DB.get() else { return };
    let mut upto: u64 = db
        .lock()
        .query_row("SELECT value FROM meta WHERE key = 'voice_log_upto'", [], |r| r.get::<_, String>(0))
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    // Whether the end has ever been reached, not whether reading ever began:
    // a restart part-way through the first import must still announce it, or
    // /awards keeps saying the voice history is loading forever.
    let mut announced = voice_caught_up();
    let mut pages = 0u32;
    loop {
        let builder = GetMessages::new().after(MessageId::new(upto.max(1))).limit(100);
        match ChannelId::new(log_channel).messages(&*http, builder).await {
            Ok(mut page) => {
                page.sort_by_key(|m| m.id.get());
                if let Some(last) = page.last().map(|m| m.id.get()) {
                    let written = write_voice_page(&mut db.lock(), &page, last);
                    match written {
                        Ok(()) => upto = last,
                        Err(err) => {
                            tracing::warn!("stats: voice log write failed: {}", err);
                            tokio::time::sleep(Duration::from_secs(60)).await;
                            continue;
                        }
                    }
                    pages += 1;
                    if !announced && pages % 100 == 0 {
                        tracing::info!("stats: voice log import - {} pages", pages);
                    }
                }
                if page.len() == 100 {
                    continue;
                }
                if !announced {
                    tracing::info!("stats: voice log caught up after {} pages", pages);
                    let _ = db
                        .lock()
                        .execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('voice_log_caught_up', '1')", []);
                    announced = true;
                }
            }
            Err(err) => tracing::warn!("stats: reading the voice log failed: {}", err),
        }
        tokio::time::sleep(Duration::from_secs(120)).await;
    }
}
