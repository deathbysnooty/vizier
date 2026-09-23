//! The nightly sheet `/ship` scores from: one small, plain row of numbers per
//! member, rebuilt once a day from the message log and the activity store.
//!
//! Numbers only. No AI is ever asked about a sheet, nothing anybody said is kept
//! in one, and nothing on one is ever shown to the model or printed on a card
//! beyond the plain counts `/ship` already showed. A sheet says *when* somebody
//! is about and *where* and *how much of their replying goes to whom* - never
//! what they are like.
//!
//! It exists because a raw count of replies is a poor signal. To turn "200
//! replies" into "a fifth of everything they say" the score needs each person's
//! own totals, and working those out inside a `/ship` would mean reading the
//! whole log twice on every command. So they are worked out once a night, for
//! members who have been about in the last [`WINDOW_DAYS`] days, a few at a time
//! so nothing stalls the bot.
//!
//! The store, `.runtime/ship_sheets.db`, is a cache and nothing else: every
//! number in it can be counted again from the log. Delete the file and the next
//! night builds it back; until then `/ship` simply has less to go on and each
//! pair's own number shows through instead. Which is exactly what it does for a
//! member who is too new or too quiet to have a sheet at all.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::{DateTime, Timelike};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serenity::all::Context;

use super::control;
use super::ship_score::Side;

/// How far back a sheet looks, and how far back a member counts as active.
pub const WINDOW_DAYS: i64 = 60;
const WINDOW_MS: i64 = WINDOW_DAYS * 86_400_000;
/// Messages of theirs one sheet reads at most, newest first.
const SHEET_ROWS: usize = 20_000;
/// Below this many messages in the window there is nothing worth counting, and
/// the member is skipped: their ships fall back to the pair's own number.
pub const MIN_MESSAGES: i64 = 30;
/// A sheet this fresh is left alone, so a run that is repeated does nothing.
pub const FRESH_SECS: i64 = 20 * 3600;
/// Two messages this close together are the same burst.
const BURST_MS: i64 = 60_000;
/// How much of a sheet is kept, so the store stays small.
const TOP_CHANNELS: usize = 12;
const TOP_PARTNERS: usize = 25;
/// Between members, so a rebuild never holds the bot up.
const BREATH: Duration = Duration::from_millis(250);
/// How often the job looks to see whether tonight's run is due.
const CHECK_EVERY: Duration = Duration::from_secs(1800);
/// Nothing runs until the bot has been up a few minutes.
const SETTLE: Duration = Duration::from_secs(240);

// --- settings ------------------------------------------------------------------------------------

/// Off stops the nightly rebuild. Sheets already built are still used - they
/// just stop being refreshed.
pub fn sheets_on() -> bool {
    control::on("VIZIER_SHIP_SHEETS", true)
}

/// The hour of the India day the rebuild runs at.
pub fn build_hour() -> u32 {
    control::number("VIZIER_SHIP_SHEET_HOUR", 4).clamp(0, 23) as u32
}

/// Members rebuilt in one night's run.
pub fn batch() -> usize {
    control::number("VIZIER_SHIP_SHEET_BATCH", 120).clamp(0, 5_000) as usize
}

// --- the sheet -----------------------------------------------------------------------------------

/// One member's numbers. Everything on it is a count of something the bot saw.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Sheet {
    pub user_id: u64,
    /// When it was built.
    pub built_ts: i64,
    /// When they are about, by hour of the India day.
    #[serde(default)]
    pub hours: [u32; 24],
    /// Messages per channel, busiest first.
    #[serde(default)]
    pub channels: Vec<(u64, u32)>,
    /// The games they play, by name.
    #[serde(default)]
    pub games: Vec<String>,
    /// How chatty: messages in the window.
    pub messages: i64,
    /// Their average message, in characters.
    pub avg_len: u32,
    /// Out of a hundred messages, how many came within a minute of their own
    /// last one - whether they talk in bursts or in single lines.
    pub burst: u32,
    /// Every reply they sent, to anyone.
    pub replies_sent: i64,
    /// Every minute they spent in voice, with anyone.
    pub vc_minutes: i64,
    /// Replies they sent to each person, most first.
    #[serde(default)]
    pub reply_partners: Vec<(u64, u32)>,
    /// Minutes in voice with each person, most first.
    #[serde(default)]
    pub vc_partners: Vec<(u64, i64)>,
}

impl Sheet {
    /// How many of their replies went to one person.
    pub fn replies_to(&self, other: u64) -> i64 {
        self.reply_partners.iter().find(|(id, _)| *id == other).map(|(_, n)| *n as i64).unwrap_or(0)
    }

    /// How many minutes they spent in voice with one person.
    pub fn vc_with(&self, other: u64) -> i64 {
        self.vc_partners.iter().find(|(id, _)| *id == other).map(|(_, n)| *n).unwrap_or(0)
    }

    /// The sheet as the score reads it, against one particular other member.
    pub fn side(&self, other: u64) -> Side {
        Side {
            replies_sent: self.replies_sent,
            replies_to_them: self.replies_to(other),
            vc_minutes: self.vc_minutes,
            vc_with_them: self.vc_with(other),
            messages: self.messages,
            hours: self.hours,
            channels: self.channels.clone(),
            games: self.games.clone(),
        }
    }
}

/// One of their messages, as a sheet reads it: no text, only its shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogRow {
    pub channel_id: u64,
    pub created_ms: i64,
    /// The message's length in characters.
    pub len: usize,
    /// Whoever they were replying to, when it was somebody else.
    pub reply_author: Option<u64>,
}

/// The hour of the India day a moment falls in.
fn ist_hour(ms: i64) -> Option<usize> {
    DateTime::from_timestamp(ms.div_euclid(1000), 0).map(|t| t.with_timezone(&super::stats::ist()).hour() as usize)
}

/// Keeps the biggest few of a tally, biggest first, ties broken by id so the
/// same counts always make the same sheet.
fn top<V: Copy + Ord>(tally: HashMap<u64, V>, keep: usize) -> Vec<(u64, V)> {
    let mut out: Vec<(u64, V)> = tally.into_iter().collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out.truncate(keep);
    out
}

/// One member's sheet, from their own messages and their voice time. Pure: the
/// tests hand it rows rather than a database.
pub fn build(user: u64, rows: &[LogRow], games: Vec<String>, vc_minutes: i64, vc_partners: Vec<(u64, i64)>, now: i64) -> Sheet {
    let mut hours = [0u32; 24];
    let mut channels: HashMap<u64, u32> = HashMap::new();
    let mut partners: HashMap<u64, u32> = HashMap::new();
    let (mut chars, mut replies, mut bursts) = (0u64, 0i64, 0u32);
    let mut ordered: Vec<&LogRow> = rows.iter().collect();
    ordered.sort_by_key(|r| r.created_ms);
    let mut previous: Option<i64> = None;
    for row in &ordered {
        if let Some(h) = ist_hour(row.created_ms) {
            hours[h] = hours[h].saturating_add(1);
        }
        *channels.entry(row.channel_id).or_insert(0) += 1;
        chars = chars.saturating_add(row.len as u64);
        if let Some(to) = row.reply_author.filter(|to| *to != user) {
            replies += 1;
            *partners.entry(to).or_insert(0) += 1;
        }
        if previous.is_some_and(|last| row.created_ms - last <= BURST_MS) {
            bursts += 1;
        }
        previous = Some(row.created_ms);
    }
    let messages = ordered.len() as i64;
    let mut games: Vec<String> = games.into_iter().filter(|g| !g.trim().is_empty()).collect();
    games.sort();
    games.dedup();
    Sheet {
        user_id: user,
        built_ts: now,
        hours,
        channels: top(channels, TOP_CHANNELS),
        games,
        messages,
        avg_len: if messages > 0 { (chars / messages as u64).min(u32::MAX as u64) as u32 } else { 0 },
        burst: if messages > 0 { ((bursts as u64 * 100) / messages as u64) as u32 } else { 0 },
        replies_sent: replies,
        vc_minutes,
        reply_partners: top(partners, TOP_PARTNERS),
        vc_partners: {
            let mut v: Vec<(u64, i64)> = vc_partners.into_iter().filter(|(id, mins)| *id != user && *mins > 0).collect();
            v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            v.truncate(TOP_PARTNERS);
            v
        },
    }
}

// --- the store ------------------------------------------------------------------------------------

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

pub const SCHEMA: &str = "
    -- One row per member. Everything in it can be counted again from the log,
    -- so losing this file costs one night, never any history.
    CREATE TABLE IF NOT EXISTS sheets (
        user_id INTEGER PRIMARY KEY, built_ts INTEGER NOT NULL, sheet_json TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);";

pub fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)
}

pub fn open(workspace: &str) -> anyhow::Result<()> {
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    std::fs::create_dir_all(&dir)?;
    let conn = Connection::open(dir.join("ship_sheets.db"))?;
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

pub fn get(conn: &Connection, user: u64) -> Option<Sheet> {
    conn.query_row("SELECT sheet_json FROM sheets WHERE user_id = ?1", params![user as i64], |r| r.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str(&json).ok())
}

pub fn save(conn: &Connection, sheet: &Sheet) -> anyhow::Result<()> {
    let json = serde_json::to_string(sheet)?;
    conn.execute(
        "INSERT INTO sheets (user_id, built_ts, sheet_json) VALUES (?1, ?2, ?3)
         ON CONFLICT(user_id) DO UPDATE SET built_ts = excluded.built_ts, sheet_json = excluded.sheet_json",
        params![sheet.user_id as i64, sheet.built_ts, json],
    )?;
    Ok(())
}

/// When each member's sheet was last built.
pub fn built_at(conn: &Connection) -> HashMap<u64, i64> {
    let Ok(mut stmt) = conn.prepare("SELECT user_id, built_ts FROM sheets") else { return HashMap::new() };
    stmt.query_map([], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)?))).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

pub fn count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM sheets", [], |r| r.get(0)).unwrap_or(0)
}

pub fn meta_get(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0)).optional().ok().flatten()
}

pub fn meta_set(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute("INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![key, value])?;
    Ok(())
}

/// Both sheets of a pair, as the score reads them. A member with no sheet reads
/// as nothing known, which leaves the pair's own number showing rather than
/// guessing at one.
pub fn sides(a: u64, b: u64) -> (Side, Side, Option<i64>, Option<i64>) {
    let Some(db) = db() else { return (Side::default(), Side::default(), None, None) };
    let conn = db.lock();
    let (sa, sb) = (get(&conn, a), get(&conn, b));
    (
        sa.as_ref().map(|s| s.side(b)).unwrap_or_default(),
        sb.as_ref().map(|s| s.side(a)).unwrap_or_default(),
        sa.map(|s| s.built_ts),
        sb.map(|s| s.built_ts),
    )
}

// --- which members are worth a sheet, and when ------------------------------------------------------

/// Whether tonight's run is due: the India hour has come round and the last run
/// was long enough ago. Claimed before the work starts, so a run that dies
/// cannot repeat every half hour.
pub fn due(now: i64, hour_now: u32, wanted: u32, last_run: Option<i64>) -> bool {
    if hour_now != wanted {
        return false;
    }
    match last_run {
        Some(then) => now - then >= FRESH_SECS,
        None => true,
    }
}

/// Who gets rebuilt tonight, quietest last. Members with almost nothing to count
/// are skipped outright, and a sheet built in the last [`FRESH_SECS`] is left
/// alone - so running twice in a night is the same as running once.
pub fn candidates(active: &HashMap<u64, i64>, built: &HashMap<u64, i64>, now: i64, limit: usize) -> Vec<u64> {
    let mut out: Vec<(u64, i64, i64)> = active
        .iter()
        .filter(|(_, messages)| **messages >= MIN_MESSAGES)
        .filter(|(user, _)| built.get(*user).is_none_or(|at| now - *at >= FRESH_SECS))
        // The stalest first, and a member with no sheet at all before any of them.
        .map(|(user, messages)| (*user, built.get(user).copied().unwrap_or(i64::MIN), *messages))
        .collect();
    out.sort_by(|a, b| a.1.cmp(&b.1).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));
    out.truncate(limit);
    out.into_iter().map(|(user, _, _)| user).collect()
}

// --- reading the record -------------------------------------------------------------------------------

/// Everyone who has said anything in the window, and how much. From the
/// activity store, which already counts a message the moment it arrives.
fn active_members(now: i64) -> HashMap<u64, i64> {
    let Some(db) = super::stats::db() else { return HashMap::new() };
    let since = DateTime::from_timestamp(now - WINDOW_DAYS * 86_400, 0)
        .map(|t| t.with_timezone(&super::stats::ist()).format("%Y-%m-%d").to_string())
        .unwrap_or_default();
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare("SELECT user_id, SUM(count) FROM msg_counts WHERE day >= ?1 GROUP BY user_id") else {
        return HashMap::new();
    };
    stmt.query_map(params![since], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)?)))
        .map(|rows| rows.flatten().filter(|(user, _)| *user != 0).collect())
        .unwrap_or_default()
}

/// One member's own messages in the window, shape only - no text is read out of
/// the log and none is kept. Blocking.
fn read_rows(user: u64, now_ms: i64) -> Vec<LogRow> {
    let Some(reader) = super::msglog::reader() else { return Vec::new() };
    let lo = super::msglog::first_id_at(now_ms - WINDOW_MS);
    let conn = reader.conn.lock();
    // The message replied to is looked up through `recent`'s own primary key,
    // so "who were they answering" costs nothing extra.
    let Ok(mut stmt) = conn.prepare_cached(
        "SELECT r.channel_id, r.created_ts, LENGTH(r.content), p.author_id
           FROM recent r LEFT JOIN recent p ON p.message_id = r.reply_to
          WHERE r.author_id = ?1 AND r.message_id >= ?2
          ORDER BY r.message_id DESC LIMIT ?3",
    ) else {
        return Vec::new();
    };
    stmt.query_map(params![user as i64, lo as i64, SHEET_ROWS as i64], |r| {
        Ok(LogRow {
            channel_id: r.get::<_, i64>(0)? as u64,
            created_ms: r.get(1)?,
            len: r.get::<_, Option<i64>>(2)?.unwrap_or(0).max(0) as usize,
            reply_author: r.get::<_, Option<i64>>(3)?.map(|id| id as u64),
        })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

/// Everybody's voice minutes in the window, and everybody's minutes with
/// everybody else - read once for the whole run rather than once per member.
fn voice_for_all(now: i64) -> (HashMap<u64, i64>, HashMap<u64, Vec<(u64, i64)>>) {
    let Some(db) = super::stats::db() else { return (HashMap::new(), HashMap::new()) };
    let since = now - WINDOW_DAYS * 86_400;
    let conn = db.lock();
    let totals: HashMap<u64, i64> =
        super::activity::voice_between(&conn, None, since, now + 1, now).unwrap_or_default().into_iter().map(|(u, secs)| (u, secs / 60)).collect();
    let mut pairs: HashMap<u64, Vec<(u64, i64)>> = HashMap::new();
    for p in super::activity::voice_pairs(&conn, since, now + 1, now).unwrap_or_default() {
        let mins = p.together / 60;
        if mins > 0 {
            pairs.entry(p.a).or_default().push((p.b, mins));
            pairs.entry(p.b).or_default().push((p.a, mins));
        }
    }
    (totals, pairs)
}

/// What one run did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Run {
    pub looked_at: usize,
    pub built: usize,
    pub skipped: usize,
}

static RUNNING: AtomicBool = AtomicBool::new(false);

/// One night's rebuild. Members are done one at a time with a breath between
/// them: it is nobody's hurry, and the bot has a server to answer.
pub async fn rebuild(who: Vec<u64>, now: i64) -> Run {
    let Some(db) = db() else { return Run::default() };
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Run::default();
    }
    let (totals, pairs) = tokio::task::spawn_blocking(move || voice_for_all(now)).await.unwrap_or_default();
    let now_ms = now * 1000;
    let mut run = Run { looked_at: who.len(), ..Run::default() };
    for user in who {
        let rows = tokio::task::spawn_blocking(move || read_rows(user, now_ms)).await.unwrap_or_default();
        if (rows.len() as i64) < MIN_MESSAGES {
            run.skipped += 1;
            tokio::time::sleep(BREATH).await;
            continue;
        }
        let games: Vec<String> =
            tokio::task::spawn_blocking(move || super::notes_facts::games_of(user).into_iter().map(|g| g.name.to_string()).collect::<Vec<_>>())
                .await
                .unwrap_or_default();
        let sheet = build(user, &rows, games, totals.get(&user).copied().unwrap_or(0), pairs.get(&user).cloned().unwrap_or_default(), now);
        match save(&db.lock(), &sheet) {
            Ok(()) => run.built += 1,
            Err(err) => tracing::warn!("ship: {}'s sheet wasn't written down: {}", user, err),
        }
        tokio::time::sleep(BREATH).await;
    }
    RUNNING.store(false, Ordering::SeqCst);
    tracing::info!("ship: rebuilt {} sheet(s) of {} looked at, {} too quiet to bother with", run.built, run.looked_at, run.skipped);
    run
}

/// Members in the server who aren't bots, from the cache; `None` if it can't tell.
fn real_members(ctx: &Context) -> Option<HashSet<u64>> {
    let guild = ctx.cache.guilds().into_iter().next()?;
    let ids: HashSet<u64> = ctx.cache.guild(guild)?.members.iter().filter(|(_, m)| !m.user.bot).map(|(id, _)| id.get()).collect();
    (!ids.is_empty()).then_some(ids)
}

/// Tonight's list: everyone active enough, bots left out, stalest first.
pub async fn tonight(ctx: &Context, now: i64) -> Vec<u64> {
    let Some(db) = db() else { return Vec::new() };
    let real = real_members(ctx);
    let mut active = tokio::task::spawn_blocking(move || active_members(now)).await.unwrap_or_default();
    if let Some(real) = real {
        active.retain(|user, _| real.contains(user));
    }
    let built = built_at(&db.lock());
    candidates(&active, &built, now, batch())
}

/// Checks every half hour whether tonight's rebuild is due. Starts once per
/// process.
pub fn spawn(ctx: Context) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        tokio::time::sleep(SETTLE).await;
        loop {
            if sheets_on() && batch() > 0 {
                if let Some(db) = db() {
                    let now = chrono::Utc::now().timestamp();
                    let hour_now = ist_hour(now * 1000).unwrap_or(99) as u32;
                    // Claimed before the work, so a run that dies can't repeat.
                    let claimed = {
                        let conn = db.lock();
                        let last = meta_get(&conn, "last_run").and_then(|v| v.parse().ok());
                        let go = due(now, hour_now, build_hour(), last);
                        if go {
                            let _ = meta_set(&conn, "last_run", &now.to_string());
                        }
                        go
                    };
                    if claimed {
                        let who = tonight(&ctx, now).await;
                        rebuild(who, now).await;
                    }
                }
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

#[cfg(test)]
pub mod tests {
    use super::*;

    const ME: u64 = 771;
    const YOU: u64 = 982;
    const SOMEONE: u64 = 1_004;

    /// Messages at named India hours: `(hour IST, channel, replying to)`, one
    /// to a day, so each one stays in the hour it was written for.
    fn rows(day: i64, spec: &[(i64, u64, Option<u64>)]) -> Vec<LogRow> {
        spec.iter()
            .enumerate()
            .map(|(i, (hour, channel, to))| LogRow {
                // 00:00 IST is 18:30 UTC the evening before.
                created_ms: ((day + i as i64) * 86_400 + hour * 3_600 - 19_800) * 1000,
                channel_id: *channel,
                len: 40,
                reply_author: *to,
            })
            .collect()
    }

    pub fn a_sheet(user: u64, now: i64) -> Sheet {
        let mut spec: Vec<(i64, u64, Option<u64>)> = Vec::new();
        for _ in 0..60 {
            spec.push((20, 1, Some(YOU)));
            spec.push((21, 1, None));
            spec.push((9, 2, Some(SOMEONE)));
        }
        build(user, &rows(20_000, &spec), vec!["Chess".into()], 600, vec![(YOU, 300)], now)
    }

    #[test]
    fn a_sheet_is_counts_and_nothing_else() {
        let s = a_sheet(ME, 1_700_000_000);
        assert_eq!(s.user_id, ME);
        assert_eq!(s.messages, 180);
        assert_eq!(s.replies_sent, 120, "only the messages that answered somebody");
        assert_eq!(s.replies_to(YOU), 60);
        assert_eq!(s.replies_to(SOMEONE), 60);
        assert_eq!(s.replies_to(4_040), 0, "somebody they have never answered");
        assert_eq!(s.vc_minutes, 600);
        assert_eq!(s.vc_with(YOU), 300);
        assert_eq!(s.games, vec!["Chess".to_string()]);
        assert_eq!(s.channels.first().map(|(c, _)| *c), Some(1), "their busiest channel first");
        assert_eq!(s.channels.iter().map(|(_, n)| *n as i64).sum::<i64>(), 180);
        assert_eq!(s.hours.iter().map(|n| *n as i64).sum::<i64>(), 180, "every message falls in an hour");
        assert_eq!((s.hours[9], s.hours[20], s.hours[21]), (60, 60, 60), "India hours, not UTC: {:?}", s.hours);
        assert_eq!(s.hours.iter().filter(|n| **n > 0).count(), 3, "and nothing lands in an hour nobody wrote in");
        assert_eq!(s.avg_len, 40);
        assert!(s.burst <= 100);
        // And the score reads it against one particular other member.
        let side = s.side(YOU);
        assert_eq!((side.replies_sent, side.replies_to_them), (120, 60));
        assert_eq!((side.vc_minutes, side.vc_with_them), (600, 300));
        assert!((side.attention() - 0.5).abs() < 1e-9, "half of their replying goes to them");
        assert_eq!(s.side(4_040).replies_to_them, 0, "a stranger gets nothing");
    }

    #[test]
    fn a_sheet_keeps_nothing_anybody_said() {
        let s = a_sheet(ME, 5);
        let json = serde_json::to_string(&s).expect("a sheet is plain numbers");
        assert!(!json.contains("content") && !json.contains("text") && !json.contains("name"), "{json}");
        // Nothing but digits, the field names and the game names.
        let words: Vec<&str> = json.split(|c: char| !c.is_alphabetic()).filter(|w| w.len() > 2).collect();
        for word in words {
            let known = [
                "user", "id", "built", "ts", "hours", "channels", "games", "messages", "avg", "len", "burst", "replies", "sent", "minutes",
                "reply", "partners", "Chess",
            ];
            assert!(known.contains(&word), "a sheet said {word:?}: {json}");
        }
        // And it survives the round trip through the store untouched.
        let conn = memory();
        save(&conn, &s).unwrap();
        assert_eq!(get(&conn, ME).as_ref(), Some(&s));
        assert_eq!(built_at(&conn).get(&ME), Some(&5));
        assert_eq!(count(&conn), 1);
        assert!(get(&conn, YOU).is_none());
    }

    #[test]
    fn bursts_and_lengths_are_counted_from_the_gaps() {
        let close: Vec<LogRow> =
            (0..10).map(|i| LogRow { channel_id: 1, created_ms: 1_700_000_000_000 + i * 10_000, len: 10, reply_author: None }).collect();
        let apart: Vec<LogRow> =
            (0..10).map(|i| LogRow { channel_id: 1, created_ms: 1_700_000_000_000 + i * 3_600_000, len: 300, reply_author: None }).collect();
        assert_eq!(build(ME, &close, vec![], 0, vec![], 0).burst, 90, "ten messages ten seconds apart is one burst");
        assert_eq!(build(ME, &apart, vec![], 0, vec![], 0).burst, 0, "an hour apart is not a burst");
        assert_eq!(build(ME, &close, vec![], 0, vec![], 0).avg_len, 10);
        assert_eq!(build(ME, &apart, vec![], 0, vec![], 0).avg_len, 300);
        // Nothing at all doesn't divide by zero.
        let empty = build(ME, &[], vec![], 0, vec![], 7);
        assert_eq!((empty.messages, empty.avg_len, empty.burst, empty.replies_sent), (0, 0, 0, 0));
        assert_eq!(empty.built_ts, 7);
        // Answering yourself is not a reply to anybody.
        let self_reply = vec![LogRow { channel_id: 1, created_ms: 1_700_000_000_000, len: 5, reply_author: Some(ME) }];
        assert_eq!(build(ME, &self_reply, vec![], 0, vec![], 0).replies_sent, 0);
        // A sheet is kept small however many channels and partners there are.
        let many: Vec<LogRow> =
            (0..200u64).map(|i| LogRow { channel_id: i, created_ms: 1_700_000_000_000 + i as i64 * 1_000, len: 5, reply_author: Some(i + 5_000) }).collect();
        let big = build(ME, &many, vec![], 0, (0..200).map(|i| (i + 9_000, 5)).collect(), 0);
        assert_eq!(big.channels.len(), TOP_CHANNELS);
        assert_eq!(big.reply_partners.len(), TOP_PARTNERS);
        assert_eq!(big.vc_partners.len(), TOP_PARTNERS);
        assert!(serde_json::to_string(&big).unwrap().len() < 4_000, "a sheet has to stay small");
    }

    #[test]
    fn the_run_is_due_once_a_night_at_the_hour_it_is_set_to() {
        let now = 1_700_000_000;
        assert!(due(now, 4, 4, None), "never run before");
        assert!(!due(now, 3, 4, None), "not the hour yet");
        assert!(!due(now, 4, 4, Some(now - 3_600)), "already run tonight");
        assert!(due(now, 4, 4, Some(now - FRESH_SECS)), "a full day later");
        // The hour is a setting, and out-of-range values are pulled back in.
        assert_eq!(build_hour(), 4, "four in the morning with nothing set");
        assert!(sheets_on() && batch() > 0, "the nightly rebuild is on by default");
    }

    #[test]
    fn the_run_skips_the_quiet_and_repeating_it_does_nothing() {
        let now = 1_700_000_000;
        let active: HashMap<u64, i64> = [(ME, 4_000), (YOU, 900), (SOMEONE, MIN_MESSAGES - 1), (4_040, 0)].into_iter().collect();
        // Nobody has a sheet: the busiest of those worth doing, quietest left out.
        let first = candidates(&active, &HashMap::new(), now, 10);
        assert_eq!(first, vec![ME, YOU], "the two with enough to count");
        assert!(!first.contains(&SOMEONE) && !first.contains(&4_040), "almost nothing on record is skipped");
        // Built just now: running again tonight does nothing at all.
        let built: HashMap<u64, i64> = first.iter().map(|u| (*u, now)).collect();
        assert!(candidates(&active, &built, now, 10).is_empty(), "a run repeated is a run wasted");
        assert!(candidates(&active, &built, now + FRESH_SECS - 1, 10).is_empty());
        assert_eq!(candidates(&active, &built, now + FRESH_SECS, 10), vec![ME, YOU], "a day later they come round again");
        // The stalest first, so a big server still comes round evenly.
        let stale: HashMap<u64, i64> = [(ME, now - 2 * FRESH_SECS), (YOU, now - 9 * FRESH_SECS)].into_iter().collect();
        assert_eq!(candidates(&active, &stale, now, 10), vec![YOU, ME]);
        // And the batch is a cap: the rest wait for tomorrow.
        assert_eq!(candidates(&active, &stale, now, 1), vec![YOU]);
        assert!(candidates(&active, &stale, now, 0).is_empty());
    }

    /// The store is a cache: losing it costs a night, never any history.
    #[tokio::test]
    async fn a_missing_store_simply_builds_itself_again() {
        // Nothing open at all: a ship still works, it just has nothing to go on.
        assert!(db().is_none(), "the tests never open the real store");
        let (a, b, built_a, built_b) = sides(ME, YOU);
        assert_eq!((a, b), (Side::default(), Side::default()), "no sheets: nothing is known, and nothing is guessed");
        assert_eq!((built_a, built_b), (None, None));
        assert_eq!(rebuild(vec![ME], 1_700_000_000).await, Run::default(), "and a rebuild with no store does nothing rather than panicking");
        // An empty store fills itself from the same rows, and the sheet that
        // comes out is the one that was lost.
        let conn = memory();
        assert_eq!(count(&conn), 0);
        let was = a_sheet(ME, 1_700_000_000);
        save(&conn, &was).unwrap();
        let again = a_sheet(ME, 1_700_000_000);
        assert_eq!(again, was, "the same log makes the same sheet");
        save(&conn, &again).unwrap();
        assert_eq!(count(&conn), 1, "and rebuilding replaces rather than piling up");
        assert_eq!(meta_get(&conn, "last_run"), None);
        meta_set(&conn, "last_run", "17").unwrap();
        assert_eq!(meta_get(&conn, "last_run").as_deref(), Some("17"));
    }
}
