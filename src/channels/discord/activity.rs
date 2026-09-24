//! Chat and voice points: house points for turning up and sticking around.
//!
//! Chat pays in tiers across an India day (20, 60 and 150 messages by default),
//! voice pays a point for every full hour spent in a room with at least one
//! other person (bots aren't company), up to the daily limits. Time spent
//! deafened doesn't count, and a deafened person isn't company either
//! (`VIZIER_VOICE_IGNORE_DEAFENED`); muted is fine. Joins and leaves are the
//! counts already kept for /awards, read back on a timer; deafened stretches
//! come from the bot's own voice-state events (stats.rs `voice_deaf`), because
//! Dyno's log has no mute or deafen lines.
//!
//! Every award goes through `house::award_person_at` with a key naming the person
//! and the day, so the timer can rerun as often as it likes - after a restart,
//! on the day before, twice in a minute - and each day still pays once.
//!
//! Voice asks for a running total rather than a price per hour: hour N says
//! "the day's voice should now come to the ladder's sum through N", and only
//! what the ledger doesn't already hold for that day is paid. So a change to
//! the ladder or to the length of an hour in the middle of a day tops people
//! up to the new rule instead of colliding with keys the old one used - which
//! is what happened on 2026-09-21, when half-hour points already sat on the
//! keys the new hours wanted and five hours paid five points.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use parking_lot::Mutex;
use rusqlite::{Connection, params};
use serenity::all::{Context, UserId};

use super::points::Source;
use super::stats;

/// Messages in one India day for the first chat point, `VIZIER_CHAT_DAY_MESSAGES`.
pub const CHAT_DAY_MESSAGES: i64 = 20;
/// The second and third chat points, `VIZIER_CHAT_TIER2_MESSAGES` and `VIZIER_CHAT_TIER3_MESSAGES`.
pub const CHAT_TIER2_MESSAGES: i64 = 60;
pub const CHAT_TIER3_MESSAGES: i64 = 150;
/// Seconds of real voice time in one India day that make it a voice day,
/// `VIZIER_VOICE_DAY_MINUTES` in minutes.
pub const VOICE_DAY_SECS: i64 = 60 * 60;

fn chat_day_messages() -> i64 {
    super::control::number("VIZIER_CHAT_DAY_MESSAGES", CHAT_DAY_MESSAGES as u64).max(1) as i64
}

/// The message counts that each earn a chat point, lowest first; a tier set
/// below the one before it is ignored.
fn chat_tiers() -> Vec<i64> {
    let raw = [
        chat_day_messages(),
        super::control::number("VIZIER_CHAT_TIER2_MESSAGES", CHAT_TIER2_MESSAGES as u64) as i64,
        super::control::number("VIZIER_CHAT_TIER3_MESSAGES", CHAT_TIER3_MESSAGES as u64) as i64,
    ];
    let mut out: Vec<i64> = Vec::new();
    for t in raw {
        if t > 0 && out.last().is_none_or(|last| t > *last) {
            out.push(t);
        }
    }
    out
}

/// Whether voice time only counts with someone else in the room.
fn voice_needs_company() -> bool {
    super::control::on("VIZIER_VOICE_NEEDS_COMPANY", true)
}

/// Whether time spent deafened is left out of voice points (and a deafened
/// person is not company).
fn voice_ignores_deafened() -> bool {
    super::control::on("VIZIER_VOICE_IGNORE_DEAFENED", true)
}

/// Bots seen in the server, kept fresh by each pass, so music bots never count
/// as company - also for the panel and /today, which have no cache to hand.
static BOTS: LazyLock<Mutex<HashSet<u64>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// What each hour of voice pays, in order: the first hour, the second, and so
/// on. `VIZIER_VOICE_HOUR_POINTS`, a comma-separated list.
///
/// An hour past the end of the list pays what the last one paid, so "1,2,3,4"
/// is a ladder that levels off at four and the daily limit decides where it
/// stops for good.
pub const VOICE_HOUR_POINTS: &str = "1,2,3,4";

pub fn voice_hour_points() -> Vec<i64> {
    let raw = super::control::var("VIZIER_VOICE_HOUR_POINTS").unwrap_or_else(|| VOICE_HOUR_POINTS.to_string());
    let ladder: Vec<i64> = raw.split(',').filter_map(|v| v.trim().parse::<i64>().ok()).map(|v| v.clamp(0, 100)).collect();
    if ladder.is_empty() { vec![1] } else { ladder }
}

/// What the `hour`th full hour of the day is worth (1 is the first).
pub fn voice_hour_worth(ladder: &[i64], hour: usize) -> i64 {
    let last = ladder.last().copied().unwrap_or(1);
    ladder.get(hour.saturating_sub(1)).copied().unwrap_or(last)
}

/// The first India day paid by the hour ladder, 2026-09-21. Days before it
/// were settled under the flat rule and are left as they are.
fn ladder_since() -> i64 {
    NaiveDate::from_ymd_opt(2026, 9, 21).and_then(day_bounds).map(|(start, _)| start).unwrap_or(0)
}

fn voice_day_secs() -> i64 {
    super::control::number("VIZIER_VOICE_DAY_MINUTES", (VOICE_DAY_SECS / 60) as u64).max(1) as i64 * 60
}
/// The same bar as awards.rs: a gap longer than this means a leave was never
/// logged, and the whole stretch counts for nothing.
const MAX_SITTING: i64 = 12 * 3600;
const EVERY: i64 = 15 * 60;
/// One last pass this many seconds before midnight. The ledger dates an award
/// by when it is written, so a day finished after midnight fills the next
/// day's limit instead of its own; the fewer of those, the better.
const LAST_CALL: i64 = 90;
/// How long into a new day the day before is still checked. A sitting that
/// began before midnight only closes when its leave arrives, at most
/// `MAX_SITTING` later; an hour on top covers a slow voice log.
const RECHECK_YESTERDAY: i64 = MAX_SITTING + 3600;

/// Dedupe keys already settled by the ledger this run, so a rerun does not
/// knock on the ledger for every regular every fifteen minutes.
static SETTLED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

#[derive(Clone, Debug)]
struct VoiceEvent {
    user: u64,
    left: bool,
    room: u64,
    ts: i64,
}

#[derive(Debug, PartialEq, Eq)]
struct Award {
    user: u64,
    source: Source,
    /// What to ask the ledger for. One for chat. For voice, the day's running
    /// total through this hour: what is paid is that less what the day already
    /// holds (see `running`).
    points: i64,
    /// `points` is a running total for the day, not an amount.
    running: bool,
    reason: String,
    dedupe: String,
    /// The last second of the day it was earned. The ledger dates each point by
    /// its timestamp, so paying yesterday's award at "now" would spend today's cap.
    at: i64,
}

/// Starts the timer. `ready` fires again on every reconnect, so only the first
/// call does anything.
pub fn spawn(ctx: &Context) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let ctx = ctx.clone();
    tokio::spawn(async move { run(ctx).await });
}

async fn run(ctx: Context) {
    // The stats file is opened before the client starts: missing now, missing
    // for good.
    let Some(db) = stats::db() else {
        tracing::warn!("activity: stats are not being recorded - no chat or voice days this run");
        return;
    };
    {
        let db = db.clone();
        let indexed = tokio::task::spawn_blocking(move || add_indexes(&db.lock())).await;
        match indexed {
            Ok(Ok(())) => {}
            Ok(Err(err)) => tracing::warn!("activity: could not index the stats tables: {}", err),
            Err(err) => tracing::warn!("activity: indexing task failed: {}", err),
        }
    }
    // Give the voice log a moment to pick up what happened while we were down.
    tokio::time::sleep(Duration::from_secs(60)).await;
    loop {
        pass(&ctx, &db).await;
        let now = Utc::now().timestamp();
        let wait = (next_run(now) - now).max(1) as u64;
        tokio::time::sleep(Duration::from_secs(wait)).await;
    }
}

/// Both queries here look up one day, and neither table is keyed that way:
/// without these every pass would read the whole history while holding the
/// lock that live message counting waits on.
fn add_indexes(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS msg_counts_day ON msg_counts (day);
         CREATE INDEX IF NOT EXISTS voice_events_ts ON voice_events (ts);",
    )
}

async fn pass(ctx: &Context, db: &Arc<Mutex<Connection>>) {
    // Switched off: nothing is paid, and a day earned meanwhile is paid when it
    // comes back on while that day (or the day after) is still being checked.
    if !super::control::on("VIZIER_ACTIVITY_POINTS", true) {
        return;
    }
    let now = Utc::now().timestamp();
    let Some(today) = ist_date(now) else { return };
    let mut days = vec![today];
    if let (Some((midnight, _)), Some(yesterday)) = (day_bounds(today), today.pred_opt()) {
        if now - midnight < RECHECK_YESTERDAY {
            days.push(yesterday);
        }
    }
    let exclude: HashSet<u64> = super::control::ids("VIZIER_STATS_EXCLUDE_CHANNELS").into_iter().collect();
    let afk = super::control::id("VIZIER_VOICE_AFK_CHANNEL");
    // Until Dyno's log has been read to the end, today's voice is the part
    // still missing. It is picked up on a later pass; nothing is lost.
    let voice = stats::voice_caught_up();
    let bots: HashSet<u64> = ctx
        .cache
        .guilds()
        .into_iter()
        .filter_map(|g| ctx.cache.guild(g).map(|g| g.members.values().filter(|m| m.user.bot).map(|m| m.user.id.get()).collect::<Vec<_>>()))
        .flatten()
        .collect();
    let bots = {
        let mut known = BOTS.lock();
        known.extend(bots);
        known.clone()
    };

    let db = db.clone();
    let loaded = tokio::task::spawn_blocking(move || {
        let conn = db.lock();
        let mut out = Vec::new();
        for day in days {
            out.extend(load_day(&conn, day, now, &exclude, afk, voice, &bots)?);
        }
        Ok::<_, rusqlite::Error>(out)
    })
    .await;
    let awards = match loaded {
        Ok(Ok(a)) => a,
        Ok(Err(err)) => {
            tracing::warn!("activity: reading the stats failed: {}", err);
            return;
        }
        Err(err) => {
            tracing::warn!("activity: stats task failed: {}", err);
            return;
        }
    };

    let todo: Vec<Award> = {
        let settled = SETTLED.lock();
        awards
            .into_iter()
            .filter(|a| !settled.contains(&a.dedupe))
            // Counted rows are people already, but voice logs name music bots too.
            .filter(|a| !ctx.cache.user(UserId::new(a.user)).is_some_and(|u| u.bot))
            .collect()
    };
    if todo.is_empty() {
        return;
    }
    let paid = tokio::task::spawn_blocking(move || {
        let mut settled = Vec::new();
        for a in todo {
            // Voice before the ladder shipped was paid in full under the rule
            // of its day; a running total would top it up at today's rates.
            if a.source == Source::Voice && a.at < ladder_since() {
                settled.push(a.dedupe);
                continue;
            }
            let asked = if a.running { a.points - super::house::earned_on(a.user, a.source, a.at) } else { a.points };
            if asked <= 0 {
                // Already holds this much for the day: nothing to write, and
                // nothing to ask again until the next hour raises the total.
                settled.push(a.dedupe);
                continue;
            }
            match super::house::award_person_at(a.user, a.source, asked, &a.reason, None, Some(a.dedupe.clone()), None, a.at.min(chrono::Utc::now().timestamp())) {
                Some((house, outcome)) => {
                    tracing::info!("activity: {} -> {} ({}): {:?}", a.dedupe, a.user, house.name, outcome);
                    settled.push(a.dedupe);
                }
                // Unsorted or opted out: asked again next pass, in case they are
                // sorted before the day is over.
                None => {}
            }
        }
        settled
    })
    .await;
    match paid {
        Ok(keys) => {
            let mut settled = SETTLED.lock();
            settled.extend(keys);
            // Only today's and yesterday's keys can come up again.
            let keep: Vec<String> =
                [Some(today), today.pred_opt()].into_iter().flatten().map(|d| d.to_string()).collect();
            settled.retain(|k| keep.iter().any(|d| k.contains(d.as_str())));
        }
        Err(err) => tracing::warn!("activity: award task failed: {}", err),
    }
}

fn ist_date(ts: i64) -> Option<NaiveDate> {
    DateTime::from_timestamp(ts, 0).map(|t| t.with_timezone(&stats::ist()).date_naive())
}

/// Midnight to midnight, India time.
fn day_bounds(day: NaiveDate) -> Option<(i64, i64)> {
    let midnight = |d: NaiveDate| {
        d.and_hms_opt(0, 0, 0).and_then(|t| t.and_local_timezone(stats::ist()).single()).map(|t| t.timestamp())
    };
    Some((midnight(day)?, midnight(day.succ_opt()?)?))
}

/// When the next pass is due: every quarter hour, plus a last call just before
/// midnight.
fn next_run(now: i64) -> i64 {
    let quarter = (now.div_euclid(EVERY) + 1) * EVERY;
    let last_call = ist_date(now).and_then(day_bounds).map(|(_, end)| end - LAST_CALL);
    match last_call {
        Some(at) if at > now && at < quarter => at,
        _ => quarter,
    }
}

/// Everyone owed a chat or voice day for `day`, as of `now`.
fn load_day(
    conn: &Connection,
    day: NaiveDate,
    now: i64,
    exclude: &HashSet<u64>,
    afk: Option<u64>,
    voice: bool,
    bots: &HashSet<u64>,
) -> rusqlite::Result<Vec<Award>> {
    let label = day.to_string();
    let mut stmt = conn
        .prepare("SELECT user_id, channel_id, SUM(count) FROM msg_counts WHERE day = ?1 GROUP BY user_id, channel_id")?;
    let rows: Vec<(u64, u64, i64)> = stmt
        .query_map(params![label], |r| {
            Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64, r.get::<_, i64>(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let chat = chat_days(&rows, exclude);

    let mut seconds = HashMap::new();
    if let (true, Some(bounds)) = (voice, day_bounds(day)) {
        // Events from MAX_SITTING either side are enough to pair the day exactly:
        // any stretch reaching into the day from further out is over the limit.
        let mut stmt = conn.prepare(
            "SELECT user_id, action, channel_id, ts FROM voice_events WHERE ts >= ?1 AND ts < ?2
             ORDER BY user_id, ts, msg_id",
        )?;
        let events: Vec<VoiceEvent> = stmt
            .query_map(params![bounds.0 - MAX_SITTING, bounds.1 + MAX_SITTING], |r| {
                Ok(VoiceEvent {
                    user: r.get::<_, i64>(0)? as u64,
                    left: r.get::<_, String>(1)? == "left",
                    room: r.get::<_, i64>(2)? as u64,
                    ts: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let real = undeafened_sittings(conn, &events, bounds, exclude, afk, now, None)?;
        seconds = if voice_needs_company() { shared_seconds(&real, bots) } else { total_seconds(&real) };
    }
    Ok(plan(day, &chat, &seconds))
}

// --- read by the control panel -----------------------------------------------

/// Messages in an India day that earn the first chat point, as set now.
pub(crate) fn chat_bar() -> i64 {
    chat_day_messages()
}

/// Every chat tier, lowest first, as set now.
pub(crate) fn chat_tier_bars() -> Vec<i64> {
    chat_tiers()
}

/// Seconds in voice that earn each voice point, as set now.
pub(crate) fn voice_bar_secs() -> i64 {
    voice_day_secs()
}

/// Whether voice points only count time with someone else in the room.
pub(crate) fn voice_company_rule() -> bool {
    voice_needs_company()
}

/// Whether deafened time is left out of voice points.
pub(crate) fn voice_deafened_rule() -> bool {
    voice_ignores_deafened()
}

/// Voice seconds that count towards voice points inside `[start, end)`: time
/// with company when that rule is on, otherwise all real voice time. For one
/// person or everyone.
pub(crate) fn voice_points_between(
    conn: &Connection,
    user: Option<u64>,
    start: i64,
    end: i64,
    now: i64,
) -> rusqlite::Result<HashMap<u64, i64>> {
    if !voice_needs_company() {
        return voice_between(conn, user, start, end, now);
    }
    let exclude: HashSet<u64> = super::control::ids("VIZIER_STATS_EXCLUDE_CHANNELS").into_iter().collect();
    let afk = super::control::id("VIZIER_VOICE_AFK_CHANNEL");
    // Company needs everyone's events, not just this person's.
    let mut stmt = conn.prepare(
        "SELECT user_id, action, channel_id, ts FROM voice_events WHERE ts >= ?1 AND ts < ?2 ORDER BY user_id, ts, msg_id",
    )?;
    let events: Vec<VoiceEvent> = stmt
        .query_map(params![start - MAX_SITTING, end + MAX_SITTING], |r| {
            Ok(VoiceEvent {
                user: r.get::<_, i64>(0)? as u64,
                left: r.get::<_, String>(1)? == "left",
                room: r.get::<_, i64>(2)? as u64,
                ts: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let bots = BOTS.lock().clone();
    // Everyone's deafened stretches too: a deafened friend is no company.
    let real = undeafened_sittings(conn, &events, (start, end), &exclude, afk, now, None)?;
    let mut shared = shared_seconds(&real, &bots);
    if let Some(u) = user {
        shared.retain(|k, _| *k == u);
    }
    Ok(shared)
}

/// Messages per person on one India day ("YYYY-MM-DD"), leaving out the
/// excluded channels exactly as chat days do. Optionally for one person.
pub(crate) fn messages_on(conn: &Connection, day: &str, user: Option<u64>) -> rusqlite::Result<HashMap<u64, i64>> {
    let exclude: HashSet<u64> = super::control::ids("VIZIER_STATS_EXCLUDE_CHANNELS").into_iter().collect();
    let mut stmt = conn.prepare(
        "SELECT user_id, channel_id, SUM(count) FROM msg_counts WHERE day = ?1 AND (?2 IS NULL OR user_id = ?2)
         GROUP BY user_id, channel_id",
    )?;
    let rows = stmt.query_map(params![day, user.map(|u| u as i64)], |r| {
        Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64, r.get::<_, i64>(2)?))
    })?;
    let mut out: HashMap<u64, i64> = HashMap::new();
    for (u, channel, n) in rows.flatten() {
        if !exclude.contains(&channel) {
            *out.entry(u).or_insert(0) += n;
        }
    }
    Ok(out)
}

/// Real voice seconds per person inside `[start, end)`, paired exactly as voice
/// days are (AFK and excluded rooms left out, deafened time left out when that
/// rule is on, a room still open counts up to `now`). Optionally for one
/// person. Read-only.
pub(crate) fn voice_between(
    conn: &Connection,
    user: Option<u64>,
    start: i64,
    end: i64,
    now: i64,
) -> rusqlite::Result<HashMap<u64, i64>> {
    let exclude: HashSet<u64> = super::control::ids("VIZIER_STATS_EXCLUDE_CHANNELS").into_iter().collect();
    let afk = super::control::id("VIZIER_VOICE_AFK_CHANNEL");
    let sql = if user.is_some() {
        "SELECT user_id, action, channel_id, ts FROM voice_events WHERE user_id = ?3 AND ts >= ?1 AND ts < ?2
         ORDER BY user_id, ts, msg_id"
    } else {
        "SELECT user_id, action, channel_id, ts FROM voice_events WHERE ts >= ?1 AND ts < ?2 AND ?3 IS NULL
         ORDER BY user_id, ts, msg_id"
    };
    let mut stmt = conn.prepare(sql)?;
    let events: Vec<VoiceEvent> = stmt
        .query_map(params![start - MAX_SITTING, end + MAX_SITTING, user.map(|u| u as i64)], |r| {
            Ok(VoiceEvent {
                user: r.get::<_, i64>(0)? as u64,
                left: r.get::<_, String>(1)? == "left",
                room: r.get::<_, i64>(2)? as u64,
                ts: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(total_seconds(&undeafened_sittings(conn, &events, (start, end), &exclude, afk, now, user)?))
}

/// `sittings` for `[start, end)` with deafened time cut out when that rule is
/// on. `user` narrows which deafened stretches are read, for callers that only
/// have that person's events anyway.
fn undeafened_sittings(
    conn: &Connection,
    events: &[VoiceEvent],
    (start, end): (i64, i64),
    exclude: &HashSet<u64>,
    afk: Option<u64>,
    now: i64,
    user: Option<u64>,
) -> rusqlite::Result<Vec<Sitting>> {
    let all = sittings(events, (start, end), exclude, afk, Some(now));
    if !voice_ignores_deafened() {
        return Ok(all);
    }
    let deaf = deafened_between(conn, user, start, end, now)?;
    Ok(cut_deafened(all, &deaf))
}

/// Deafened stretches per person overlapping `[start, end)`, as recorded from
/// the bot's own voice-state events; one still open runs to `now`.
fn deafened_between(
    conn: &Connection,
    user: Option<u64>,
    start: i64,
    end: i64,
    now: i64,
) -> rusqlite::Result<HashMap<u64, Vec<(i64, i64)>>> {
    let mut stmt = conn.prepare(
        "SELECT user_id, start_ts, COALESCE(end_ts, ?3) FROM voice_deaf
         WHERE start_ts < ?2 AND (end_ts IS NULL OR end_ts > ?1) AND (?4 IS NULL OR user_id = ?4)",
    )?;
    let rows = stmt.query_map(params![start, end, now, user.map(|u| u as i64)], |r| {
        Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
    })?;
    let mut out: HashMap<u64, Vec<(i64, i64)>> = HashMap::new();
    for row in rows {
        let (u, s, e) = row?;
        if e > s {
            out.entry(u).or_default().push((s, e));
        }
    }
    Ok(out)
}

/// Each sitting with its person's deafened stretches taken out; the pieces
/// either side stay. A person with no stretches (never deafened, or only
/// muted) keeps their sittings as they are.
fn cut_deafened(sittings: Vec<Sitting>, deaf: &HashMap<u64, Vec<(i64, i64)>>) -> Vec<Sitting> {
    let mut out = Vec::with_capacity(sittings.len());
    for s in sittings {
        let Some(spans) = deaf.get(&s.user) else {
            out.push(s);
            continue;
        };
        let mut spans: Vec<(i64, i64)> = spans.iter().copied().filter(|&(a, b)| b > a && a < s.end && b > s.start).collect();
        spans.sort_unstable();
        let mut from = s.start;
        for (a, b) in spans {
            if a > from {
                out.push(Sitting { start: from, end: a, ..s });
            }
            from = from.max(b);
            if from >= s.end {
                break;
            }
        }
        if from < s.end {
            out.push(Sitting { start: from, ..s });
        }
    }
    out
}

/// Messages per person who reached the first chat tier, from (user, channel,
/// messages) rows for one day.
fn chat_days(rows: &[(u64, u64, i64)], exclude: &HashSet<u64>) -> HashMap<u64, i64> {
    let mut per_user: HashMap<u64, i64> = HashMap::new();
    for &(user, channel, count) in rows {
        if !exclude.contains(&channel) {
            *per_user.entry(user).or_insert(0) += count;
        }
    }
    let bar = chat_tiers().first().copied().unwrap_or(CHAT_DAY_MESSAGES);
    per_user.retain(|_, n| *n >= bar);
    per_user
}

/// Real voice seconds per person inside `day` (start, end).
///
/// Paired exactly as awards.rs `load()` pairs them: events in (user, ts, msg_id)
/// order, any event closes what was open, a switch opens the next room at once,
/// a gap over `MAX_SITTING` counts nothing, and time in the AFK room or an
/// excluded room is not real. Two differences, both about the day boundary:
///
/// - A stretch is split at midnight, so each day gets only the minutes that
///   fell on it. awards.rs gives a stretch to the day it began, which is fine
///   for a week but would hand 23:30-00:45 a whole 75 minutes on the first
///   day, where only 30 were spent.
/// - A room still open at `now` counts up to `now`, under the same 12-hour
///   bar. Someone sitting in voice has their hour when the hour is up, on the
///   day they spent it, rather than whenever they leave - possibly after
///   midnight, when the ledger would book it against the next day.
#[cfg(test)]
fn voice_seconds(
    events: &[VoiceEvent],
    day: (i64, i64),
    exclude: &HashSet<u64>,
    afk: Option<u64>,
    now: Option<i64>,
) -> HashMap<u64, i64> {
    total_seconds(&sittings(events, day, exclude, afk, now))
}

fn total_seconds(sittings: &[Sitting]) -> HashMap<u64, i64> {
    let mut out: HashMap<u64, i64> = HashMap::new();
    for s in sittings {
        *out.entry(s.user).or_insert(0) += s.end - s.start;
    }
    out
}

/// One stretch in one room, clipped to the day.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sitting {
    user: u64,
    room: u64,
    start: i64,
    end: i64,
}

/// Every real stretch in voice inside `day`, paired as `voice_seconds`
/// describes: AFK and excluded rooms and stretches over `MAX_SITTING` dropped,
/// split at the day's edges, an open room counted up to `now`.
fn sittings(
    events: &[VoiceEvent],
    (day_start, day_end): (i64, i64),
    exclude: &HashSet<u64>,
    afk: Option<u64>,
    now: Option<i64>,
) -> Vec<Sitting> {
    let mut out = Vec::new();
    let mut add = |user: u64, start: i64, end: i64, room: u64| {
        if end - start > MAX_SITTING || Some(room) == afk || exclude.contains(&room) {
            return;
        }
        let (s, e) = (start.max(day_start), end.min(day_end));
        if e > s {
            out.push(Sitting { user, room, start: s, end: e });
        }
    };
    let mut current: Option<u64> = None;
    let mut open: Option<(i64, u64)> = None;
    for e in events {
        if current != Some(e.user) {
            if let (Some(user), Some((start, room)), Some(now)) = (current, open, now) {
                if now > start {
                    add(user, start, now, room);
                }
            }
            current = Some(e.user);
            open = None;
        }
        if let Some((start, room)) = open {
            add(e.user, start, e.ts, room);
        }
        open = if e.left { None } else { Some((e.ts, e.room)) };
    }
    if let (Some(user), Some((start, room)), Some(now)) = (current, open, now) {
        if now > start {
            add(user, start, now, room);
        }
    }
    out
}

/// Seconds each person spent in voice with at least one other person (not a
/// bot) in the same room. Bots get nothing themselves.
fn shared_seconds(sittings: &[Sitting], bots: &HashSet<u64>) -> HashMap<u64, i64> {
    let people: Vec<&Sitting> = sittings.iter().filter(|s| !bots.contains(&s.user)).collect();
    let mut out: HashMap<u64, i64> = HashMap::new();
    for me in &people {
        // Everyone else's overlap with this stretch, merged so two companions
        // at once aren't counted twice.
        let mut overlaps: Vec<(i64, i64)> = people
            .iter()
            .filter(|o| o.user != me.user && o.room == me.room)
            .map(|o| (o.start.max(me.start), o.end.min(me.end)))
            .filter(|(s, e)| e > s)
            .collect();
        overlaps.sort_unstable();
        let mut total = 0;
        let mut reach = i64::MIN;
        for (s, e) in overlaps {
            let s = s.max(reach);
            if e > s {
                total += e - s;
                reach = e;
            }
        }
        if total > 0 {
            *out.entry(me.user).or_insert(0) += total;
        }
    }
    out
}

// --- who sits with whom -------------------------------------------------------------------

/// How long two people were in voice together, and how much of that was with
/// nobody else in the room.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct VoicePair {
    /// The lower id of the two, so a pair is the same pair either way round.
    pub a: u64,
    pub b: u64,
    /// Seconds in the same room at the same time.
    pub together: i64,
    /// Of those, the seconds when the room held exactly the two of them.
    pub alone: i64,
    /// The last moment they were in a room together.
    pub last_ts: i64,
    /// The room they shared the longest, and for how long.
    pub top_room: Option<(u64, i64)>,
}

/// Every pair's shared time, from stretches already cleaned up by `sittings`.
/// Bots are not company and get no pairs of their own.
///
/// A room's arrivals and departures are walked in order, so the company is
/// known at every moment without comparing every stretch to every other one:
/// between two marks the room holds a fixed set of people, and each of its
/// pairs gets that stretch - the whole of it as time together, and, when the
/// two of them are all that is there, as time alone as well.
fn pair_seconds(sittings: &[Sitting], bots: &HashSet<u64>) -> Vec<VoicePair> {
    let mut by_room: HashMap<u64, Vec<&Sitting>> = HashMap::new();
    for s in sittings.iter().filter(|s| s.end > s.start && !bots.contains(&s.user)) {
        by_room.entry(s.room).or_default().push(s);
    }
    // (together, alone, last moment, seconds per room)
    let mut totals: HashMap<(u64, u64), (i64, i64, i64, HashMap<u64, i64>)> = HashMap::new();
    for (room, list) in by_room {
        let mut marks: Vec<(i64, i8, u64)> = Vec::with_capacity(list.len() * 2);
        for s in &list {
            marks.push((s.start, 1, s.user));
            marks.push((s.end, -1, s.user));
        }
        // Leaving sorts before arriving at the same moment: someone who leaves
        // as another arrives was never there with them.
        marks.sort_unstable();
        let mut here: HashMap<u64, i32> = HashMap::new();
        let mut prev = 0i64;
        for (ts, delta, user) in marks {
            if here.len() >= 2 && ts > prev {
                let span = ts - prev;
                let only_two = here.len() == 2;
                let mut who: Vec<u64> = here.keys().copied().collect();
                who.sort_unstable();
                for (i, &a) in who.iter().enumerate() {
                    for &b in &who[i + 1..] {
                        let e = totals.entry((a, b)).or_default();
                        e.0 += span;
                        if only_two {
                            e.1 += span;
                        }
                        e.2 = e.2.max(ts);
                        *e.3.entry(room).or_insert(0) += span;
                    }
                }
            }
            prev = ts;
            if delta > 0 {
                *here.entry(user).or_insert(0) += 1;
            } else if let Some(n) = here.get_mut(&user) {
                *n -= 1;
                if *n <= 0 {
                    here.remove(&user);
                }
            }
        }
    }
    let mut out: Vec<VoicePair> = totals
        .into_iter()
        .map(|((a, b), (together, alone, last_ts, rooms))| VoicePair {
            a,
            b,
            together,
            alone,
            last_ts,
            top_room: rooms.into_iter().max_by_key(|&(room, secs)| (secs, std::cmp::Reverse(room))),
        })
        .collect();
    out.sort_by(|x, y| y.together.cmp(&x.together).then(y.alone.cmp(&x.alone)).then((x.a, x.b).cmp(&(y.a, y.b))));
    out
}

/// Seconds every pair spent in voice together inside `[start, end)`, counted
/// exactly as voice points are: the AFK room, excluded rooms and stretches over
/// `MAX_SITTING` are nothing, deafened time is cut out when that rule is on,
/// and a room still open counts up to `now`. Most time first. Read-only.
pub(crate) fn voice_pairs(conn: &Connection, start: i64, end: i64, now: i64) -> rusqlite::Result<Vec<VoicePair>> {
    let exclude: HashSet<u64> = super::control::ids("VIZIER_STATS_EXCLUDE_CHANNELS").into_iter().collect();
    let afk = super::control::id("VIZIER_VOICE_AFK_CHANNEL");
    // Pairs need everybody's events, not one person's.
    let mut stmt = conn.prepare(
        "SELECT user_id, action, channel_id, ts FROM voice_events WHERE ts >= ?1 AND ts < ?2 ORDER BY user_id, ts, msg_id",
    )?;
    let events: Vec<VoiceEvent> = stmt
        .query_map(params![start - MAX_SITTING, end + MAX_SITTING], |r| {
            Ok(VoiceEvent {
                user: r.get::<_, i64>(0)? as u64,
                left: r.get::<_, String>(1)? == "left",
                room: r.get::<_, i64>(2)? as u64,
                ts: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let real = undeafened_sittings(conn, &events, (start, end), &exclude, afk, now, None)?;
    Ok(pair_seconds(&real, &BOTS.lock().clone()))
}

/// Every stretch in every room inside `[start, end)`, everybody's, as
/// `(user, room, start, end)` — the rows behind [`voice_pairs`] before they are
/// folded into pairs. The Deep dive needs them whole, so that one member's
/// sessions can be listed with the company each one had. Counted exactly as
/// voice points are: the AFK room, excluded rooms and stretches over
/// `MAX_SITTING` are nothing, deafened time is cut out when that rule is on,
/// and a room still open counts up to `now`. Read-only.
pub(crate) fn voice_stays(conn: &Connection, start: i64, end: i64, now: i64) -> rusqlite::Result<Vec<(u64, u64, i64, i64)>> {
    let exclude: HashSet<u64> = super::control::ids("VIZIER_STATS_EXCLUDE_CHANNELS").into_iter().collect();
    let afk = super::control::id("VIZIER_VOICE_AFK_CHANNEL");
    // Company can't be known from one person's rows, so this reads everyone's.
    let mut stmt = conn.prepare(
        "SELECT user_id, action, channel_id, ts FROM voice_events WHERE ts >= ?1 AND ts < ?2 ORDER BY user_id, ts, msg_id",
    )?;
    let events: Vec<VoiceEvent> = stmt
        .query_map(params![start - MAX_SITTING, end + MAX_SITTING], |r| {
            Ok(VoiceEvent {
                user: r.get::<_, i64>(0)? as u64,
                left: r.get::<_, String>(1)? == "left",
                room: r.get::<_, i64>(2)? as u64,
                ts: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let real = undeafened_sittings(conn, &events, (start, end), &exclude, afk, now, None)?;
    let bots = BOTS.lock().clone();
    Ok(real.iter().filter(|s| !bots.contains(&s.user)).map(|s| (s.user, s.room, s.start, s.end)).collect())
}

/// The awards owed for one day: a chat point per tier reached and a voice point
/// per full hour (with company, when that rule is on). The same rows always give
/// the same list, and each key names the person, the day and the tier, so a
/// rerun asks for nothing new. The first tier keeps the key it had when there
/// was only one, so a day already paid isn't paid again.
fn plan(day: NaiveDate, chat: &HashMap<u64, i64>, voice: &HashMap<u64, i64>) -> Vec<Award> {
    let at = day_bounds(day).map(|(_, end)| end - 1).unwrap_or(0);
    let key = |kind: &str, user: u64, tier: usize| {
        if tier == 1 { format!("{}:{}:{}", kind, day, user) } else { format!("{}:{}:{}:{}", kind, day, user, tier) }
    };
    let tiers = chat_tiers();
    let mut chatters: Vec<(&u64, &i64)> = chat.iter().filter(|(u, _)| **u != 0).collect();
    chatters.sort_unstable();
    let mut out: Vec<Award> = Vec::new();
    for (&user, &messages) in chatters {
        for (i, bar) in tiers.iter().enumerate().filter(|(_, bar)| messages >= **bar) {
            out.push(Award {
                user,
                source: Source::Chat,
                points: 1,
                reason: format!("{}+ messages on {}", bar, day),
                dedupe: key("chat", user, i + 1),
                at,
                running: false,
            });
        }
    }
    let bar = voice_day_secs();
    let company = voice_needs_company();
    let mut voiced: Vec<(&u64, &i64)> = voice.iter().filter(|(u, s)| **u != 0 && **s >= bar).collect();
    voiced.sort_unstable();
    for (&user, &secs) in voiced {
        // The ledger's daily limit decides how many are kept; a few spare
        // hours on a marathon day are asked for and refused as capped.
        let hours = (secs / bar).min(24) as usize;
        let ladder = voice_hour_points();
        let mut total = 0;
        for hour in 1..=hours {
            let worth = voice_hour_worth(&ladder, hour);
            if worth <= 0 {
                continue;
            }
            total += worth;
            out.push(Award {
                user,
                source: Source::Voice,
                points: total,
                running: true,
                reason: format!(
                    "{} {} in voice{} on {} (hour {})",
                    hour * (bar / 60) as usize,
                    "minutes",
                    if company { " with others" } else { "" },
                    day,
                    hour
                ),
                // "h" keeps these clear of the keys the half-hour rule used
                // (`voice:<day>:<user>:<n>`), which the same day may still hold.
                dedupe: format!("voice:{}:{}:h{}", day, user, hour),
                at,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::discord::house::HOUSES;
    use crate::channels::discord::points::{self, Entry, Outcome};

    const H: i64 = 3600;
    const ME: u64 = 11;
    const YOU: u64 = 22;
    const ROOM: u64 = 500;
    const OTHER_ROOM: u64 = 501;
    const AFK: u64 = 900;
    const MOD_ROOM: u64 = 901;

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, 14).unwrap()
    }

    fn bounds() -> (i64, i64) {
        day_bounds(day()).unwrap()
    }

    fn ev(user: u64, action: &str, room: u64, ts: i64) -> VoiceEvent {
        VoiceEvent { user, left: action == "left", room, ts }
    }

    fn excluded() -> HashSet<u64> {
        [MOD_ROOM].into_iter().collect()
    }

    fn secs(events: &[VoiceEvent], now: Option<i64>) -> HashMap<u64, i64> {
        voice_seconds(events, bounds(), &excluded(), Some(AFK), now)
    }

    /// Pairs from events given in any order - the reader wants them by person,
    /// the way the query hands them over.
    fn pairs(events: &[VoiceEvent]) -> Vec<VoicePair> {
        pair_seconds(&by_person(events), &HashSet::new())
    }

    fn by_person(events: &[VoiceEvent]) -> Vec<Sitting> {
        let mut events = events.to_vec();
        events.sort_by_key(|e| (e.user, e.ts));
        sittings(&events, bounds(), &excluded(), Some(AFK), None)
    }

    #[test]
    fn two_people_in_a_room_are_together_and_alone() {
        let s = bounds().0;
        let list = pairs(&[
            ev(ME, "joined", ROOM, s + H),
            ev(YOU, "joined", ROOM, s + H),
            ev(ME, "left", ROOM, s + 3 * H),
            ev(YOU, "left", ROOM, s + 3 * H),
        ]);
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].a, list[0].b), (ME, YOU));
        assert_eq!(list[0].together, 2 * H);
        assert_eq!(list[0].alone, 2 * H);
        assert_eq!(list[0].top_room, Some((ROOM, 2 * H)));
    }

    #[test]
    fn a_third_person_ends_the_time_alone_but_not_the_time_together() {
        let s = bounds().0;
        let third = 33;
        let list = pairs(&[
            ev(ME, "joined", ROOM, s),
            ev(YOU, "joined", ROOM, s),
            ev(third, "joined", ROOM, s + H),
            ev(third, "left", ROOM, s + 2 * H),
            ev(ME, "left", ROOM, s + 3 * H),
            ev(YOU, "left", ROOM, s + 3 * H),
        ]);
        let mine = list.iter().find(|p| (p.a, p.b) == (ME, YOU)).expect("the pair");
        assert_eq!(mine.together, 3 * H);
        assert_eq!(mine.alone, 2 * H);
        // The hour of three is company for all three pairs, alone for none.
        for p in list.iter().filter(|p| p.b == third || p.a == third) {
            assert_eq!((p.together, p.alone), (H, 0));
        }
    }

    #[test]
    fn different_rooms_at_the_same_time_are_not_together() {
        let s = bounds().0;
        assert!(
            pairs(&[
                ev(ME, "joined", ROOM, s),
                ev(YOU, "joined", OTHER_ROOM, s),
                ev(ME, "left", ROOM, s + 2 * H),
                ev(YOU, "left", OTHER_ROOM, s + 2 * H),
            ])
            .is_empty()
        );
    }

    #[test]
    fn the_afk_room_and_excluded_rooms_are_no_company() {
        let s = bounds().0;
        assert!(
            pairs(&[
                ev(ME, "joined", AFK, s),
                ev(YOU, "joined", AFK, s),
                ev(ME, "left", AFK, s + 2 * H),
                ev(YOU, "left", AFK, s + 2 * H),
            ])
            .is_empty()
        );
        assert!(
            pairs(&[
                ev(ME, "joined", MOD_ROOM, s),
                ev(YOU, "joined", MOD_ROOM, s),
                ev(ME, "left", MOD_ROOM, s + 2 * H),
                ev(YOU, "left", MOD_ROOM, s + 2 * H),
            ])
            .is_empty()
        );
    }

    #[test]
    fn leaving_as_another_arrives_is_no_time_together() {
        let s = bounds().0;
        assert!(
            pairs(&[
                ev(ME, "joined", ROOM, s),
                ev(ME, "left", ROOM, s + H),
                ev(YOU, "joined", ROOM, s + H),
                ev(YOU, "left", ROOM, s + 2 * H),
            ])
            .is_empty()
        );
    }

    #[test]
    fn a_bot_in_the_room_is_not_company_and_keeps_no_pair() {
        let s = bounds().0;
        let bot = 77;
        let list = pair_seconds(
            &by_person(&[
                    ev(ME, "joined", ROOM, s),
                    ev(YOU, "joined", ROOM, s),
                    ev(bot, "joined", ROOM, s),
                    ev(bot, "left", ROOM, s + 2 * H),
                    ev(ME, "left", ROOM, s + 2 * H),
                    ev(YOU, "left", ROOM, s + 2 * H),
            ]),
            &[bot].into_iter().collect(),
        );
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].together, list[0].alone), (2 * H, 2 * H));
    }

    #[test]
    fn the_hours_in_voice_climb_and_then_level_off() {
        let ladder = vec![1, 2, 3, 4];
        assert_eq!(voice_hour_worth(&ladder, 1), 1);
        assert_eq!(voice_hour_worth(&ladder, 2), 2);
        assert_eq!(voice_hour_worth(&ladder, 3), 3);
        assert_eq!(voice_hour_worth(&ladder, 4), 4);
        // Past the end it keeps paying what the last hour paid; the daily
        // limit is what stops a marathon, not the ladder.
        assert_eq!(voice_hour_worth(&ladder, 5), 4);
        assert_eq!(voice_hour_worth(&ladder, 12), 4);
        // Four hours is ten points, six is eighteen - so a cap of twenty is
        // reached somewhere in the seventh.
        let day: i64 = (1..=4).map(|h| voice_hour_worth(&ladder, h)).sum();
        assert_eq!(day, 10);
        assert_eq!((1..=6).map(|h| voice_hour_worth(&ladder, h)).sum::<i64>(), 18);
        // A flat ladder is the old rule, unchanged.
        assert_eq!(voice_hour_worth(&[1], 9), 1);
        // Hour nought is not a thing, and asking for it must not panic.
        assert_eq!(voice_hour_worth(&ladder, 0), 1);
    }

    #[test]
    fn a_voice_day_asks_for_each_hour_at_its_own_worth() {
        let voice: HashMap<u64, i64> = [(ME, 3 * H + 600)].into_iter().collect();
        let awards = plan(day(), &HashMap::new(), &voice);
        let hours: Vec<(String, i64)> = awards
            .iter()
            .filter(|a| a.source == Source::Voice)
            .map(|a| (a.dedupe.clone(), a.points))
            .collect();
        assert_eq!(hours.len(), 3, "three full hours: {:?}", hours);
        // Each asks for the day's running total: 1, then 1+2, then 1+2+3.
        assert_eq!(hours.iter().map(|h| h.1).collect::<Vec<_>>(), vec![1, 3, 6]);
        assert!(awards.iter().filter(|a| a.source == Source::Voice).all(|a| a.running));
        // Each hour keeps its own key, so a rerun pays none of them twice.
        assert_eq!(hours[0].0, format!("voice:{}:{}:h1", day(), ME));
        assert!(hours[2].0.ends_with(":h3"), "{}", hours[2].0);
        // And the reason says which hour it was, because "60 minutes" on a
        // three-point row reads like a mistake otherwise.
        let third = awards.iter().filter(|a| a.source == Source::Voice).nth(2).expect("the third hour");
        assert!(third.reason.contains("hour 3"), "{}", third.reason);
    }

    /// 2026-09-21: five half-hours were paid a point each under the old rule,
    /// on keys `voice:<day>:<user>:1..5`. The ladder's hours used the same keys,
    /// so five hours came to five points. Asking for the running total less what
    /// the day holds tops it up to the ladder's fourteen, and no further.
    #[test]
    fn a_rule_change_mid_day_tops_up_to_the_ladder_and_never_pays_twice() {
        let ledger = Connection::open_in_memory().unwrap();
        ledger.execute_batch(points::SCHEMA).unwrap();
        let (start, _) = bounds();
        let at = start + 20 * H;
        let write = |points: i64, dedupe: String| {
            let entry = Entry {
                user: Some(ME),
                house: &HOUSES[0],
                source: Source::Voice,
                scope: None,
                points,
                reason: "voice",
                by: None,
                dedupe: Some(dedupe),
            };
            points::write(&ledger, &entry, at).unwrap()
        };
        for half in 1..=5 {
            let key = if half == 1 { format!("voice:{}:{}", day(), ME) } else { format!("voice:{}:{}:{}", day(), ME, half) };
            write(1, key);
        }
        let held = || -> i64 {
            ledger
                .query_row(
                    "SELECT COALESCE(SUM(points), 0) FROM ledger WHERE user_id = ?1 AND source = 'voice' AND day = ?2 AND points > 0",
                    params![ME as i64, points::ist_day(at)],
                    |r| r.get(0),
                )
                .unwrap()
        };
        assert_eq!(held(), 5);
        let voice: HashMap<u64, i64> = [(ME, 305 * 60)].into_iter().collect();
        let pay = || {
            for a in plan(day(), &HashMap::new(), &voice).into_iter().filter(|a| a.source == Source::Voice) {
                let asked = a.points - held();
                if asked > 0 {
                    write(asked, a.dedupe);
                }
            }
        };
        pay();
        assert_eq!(held(), 14, "1+2+3+4+4 for five hours");
        pay();
        assert_eq!(held(), 14, "a rerun pays nothing more");
    }

    #[test]
    fn an_india_day_runs_midnight_to_midnight() {
        let (start, end) = bounds();
        assert_eq!(end - start, 24 * H);
        assert_eq!(points::ist_day(start), "2026-09-14");
        assert_eq!(points::ist_day(start - 1), "2026-09-13");
        assert_eq!(points::ist_day(end), "2026-09-15");
    }

    #[test]
    fn twenty_messages_make_a_chat_day_but_not_in_excluded_rooms() {
        let exclude: HashSet<u64> = [7].into_iter().collect();
        let rows = vec![
            (ME, 1, 12),
            (ME, 2, 8), // 20 across two rooms
            (YOU, 1, 15),
            (YOU, 7, 30), // mostly in an excluded room
            (33, 1, 19),
        ];
        assert_eq!(chat_days(&rows, &exclude), [(ME, 20)].into_iter().collect());
    }

    #[test]
    fn chat_pays_a_point_per_tier_reached() {
        let chat: HashMap<u64, i64> = [(ME, 150), (YOU, 59), (33, 19)].into_iter().collect();
        let awards = plan(day(), &chat, &HashMap::new());
        let keys: Vec<&str> = awards.iter().map(|a| a.dedupe.as_str()).collect();
        assert_eq!(keys, vec!["chat:2026-09-14:11", "chat:2026-09-14:11:2", "chat:2026-09-14:11:3", "chat:2026-09-14:22"]);
    }

    #[test]
    fn voice_counts_only_time_with_someone_else_and_bots_are_not_company() {
        let (s, _) = bounds();
        const BOT: u64 = 99;
        const THIRD: u64 = 33;
        let e = [
            // ME in ROOM 10:00-13:00; YOU there 11:00-12:30; THIRD 12:00-13:30.
            ev(ME, "joined", ROOM, s + 10 * H),
            ev(ME, "left", ROOM, s + 13 * H),
            ev(YOU, "joined", ROOM, s + 11 * H),
            ev(YOU, "left", ROOM, s + 12 * H + 1800),
            ev(THIRD, "joined", ROOM, s + 12 * H),
            ev(THIRD, "left", ROOM, s + 13 * H + 1800),
            // A music bot keeps someone company in another room: that's alone.
            ev(BOT, "joined", OTHER_ROOM, s + 14 * H),
            ev(BOT, "left", OTHER_ROOM, s + 16 * H),
            ev(44, "joined", OTHER_ROOM, s + 14 * H),
            ev(44, "left", OTHER_ROOM, s + 16 * H),
        ];
        let mut sorted = e.to_vec();
        sorted.sort_by_key(|v| (v.user, v.ts));
        let bots: HashSet<u64> = [BOT].into_iter().collect();
        let shared = shared_seconds(&sittings(&sorted, bounds(), &excluded(), Some(AFK), None), &bots);
        // ME had company 11:00-13:00 (YOU then THIRD overlapping, not counted twice).
        assert_eq!(shared.get(&ME), Some(&(2 * H)));
        assert_eq!(shared.get(&YOU), Some(&(90 * 60)));
        assert_eq!(shared.get(&THIRD), Some(&H));
        assert_eq!(shared.get(&44), None, "a bot is not company");
        assert_eq!(shared.get(&BOT), None);
        let hours: Vec<String> = plan(day(), &HashMap::new(), &shared).into_iter().map(|a| a.dedupe).collect();
        assert_eq!(hours, vec!["voice:2026-09-14:11:h1", "voice:2026-09-14:11:h2", "voice:2026-09-14:22:h1", "voice:2026-09-14:33:h1"]);
    }

    fn sit(user: u64, room: u64, start: i64, end: i64) -> Sitting {
        Sitting { user, room, start, end }
    }

    fn deaf(spans: &[(u64, i64, i64)]) -> HashMap<u64, Vec<(i64, i64)>> {
        let mut out: HashMap<u64, Vec<(i64, i64)>> = HashMap::new();
        for &(u, s, e) in spans {
            out.entry(u).or_default().push((s, e));
        }
        out
    }

    #[test]
    fn deafened_stretches_are_cut_out_of_sittings() {
        let s = sit(ME, ROOM, 10 * H, 14 * H);
        let cut = |spans: &[(u64, i64, i64)]| cut_deafened(vec![s], &deaf(spans));
        // Inside: the pieces either side stay.
        assert_eq!(cut(&[(ME, 11 * H, 12 * H)]), vec![sit(ME, ROOM, 10 * H, 11 * H), sit(ME, ROOM, 12 * H, 14 * H)]);
        // Over either edge.
        assert_eq!(cut(&[(ME, 9 * H, 10 * H + 900)]), vec![sit(ME, ROOM, 10 * H + 900, 14 * H)]);
        assert_eq!(cut(&[(ME, 13 * H, 15 * H)]), vec![sit(ME, ROOM, 10 * H, 13 * H)]);
        // The whole sitting, or exactly it.
        assert!(cut(&[(ME, 9 * H, 15 * H)]).is_empty());
        assert!(cut(&[(ME, 10 * H, 14 * H)]).is_empty());
        // Touching the edges from outside cuts nothing.
        assert_eq!(cut(&[(ME, 8 * H, 10 * H), (ME, 14 * H, 16 * H)]), vec![s]);
        // Several, out of order and overlapping each other.
        assert_eq!(
            cut(&[(ME, 13 * H, 13 * H + 600), (ME, 10 * H + 600, 11 * H), (ME, 10 * H + 1200, 11 * H + 1800)]),
            vec![sit(ME, ROOM, 10 * H, 10 * H + 600), sit(ME, ROOM, 11 * H + 1800, 13 * H), sit(ME, ROOM, 13 * H + 600, 14 * H)]
        );
        // Someone else's deafen, or none at all (muted leaves no record), changes nothing.
        assert_eq!(cut(&[(YOU, 11 * H, 12 * H)]), vec![s]);
        assert_eq!(cut(&[]), vec![s]);
        let total: i64 = cut(&[(ME, 11 * H, 12 * H), (ME, 13 * H, 13 * H + 1800)]).iter().map(|p| p.end - p.start).sum();
        assert_eq!(total, 4 * H - H - 1800);
    }

    #[test]
    fn a_deafened_friend_is_not_company_and_earns_nothing_meanwhile() {
        let (s, _) = bounds();
        // ME and YOU in ROOM 10:00-12:00; YOU deafened 10:30-11:30.
        let sittings = vec![sit(ME, ROOM, s + 10 * H, s + 12 * H), sit(YOU, ROOM, s + 10 * H, s + 12 * H)];
        let cut = cut_deafened(sittings.clone(), &deaf(&[(YOU, s + 10 * H + 1800, s + 11 * H + 1800)]));
        let shared = shared_seconds(&cut, &HashSet::new());
        assert_eq!(shared.get(&ME), Some(&H), "alone with a deafened friend is alone");
        assert_eq!(shared.get(&YOU), Some(&H));
        // Not deafened (muted, or nothing recorded): the full two hours each.
        let shared = shared_seconds(&cut_deafened(sittings, &HashMap::new()), &HashSet::new());
        assert_eq!((shared[&ME], shared[&YOU]), (2 * H, 2 * H));
    }

    #[test]
    fn join_then_leave_is_the_time_between() {
        let (s, _) = bounds();
        let e = [ev(ME, "joined", ROOM, s + 10 * H), ev(ME, "left", ROOM, s + 11 * H)];
        assert_eq!(secs(&e, None)[&ME], H);
    }

    #[test]
    fn a_switch_carries_the_sitting_on_and_afk_or_excluded_rooms_count_nothing() {
        let (s, _) = bounds();
        // Each comment is the stretch that event closes.
        let e = [
            ev(ME, "joined", ROOM, s + 10 * H),
            ev(ME, "switched", OTHER_ROOM, s + 10 * H + 1200), // 20 min in ROOM
            ev(ME, "switched", AFK, s + 10 * H + 2400),        // 20 min in OTHER_ROOM
            ev(ME, "switched", MOD_ROOM, s + 12 * H),          // AFK: nothing
            ev(ME, "switched", ROOM, s + 13 * H),              // excluded room: nothing
            ev(ME, "left", ROOM, s + 13 * H + 1200),           // 20 min in ROOM
        ];
        let got = secs(&e, None);
        assert_eq!(got[&ME], 3600);
        assert_eq!(plan(day(), &HashMap::new(), &got).len(), 1, "exactly an hour is enough");
    }

    #[test]
    fn a_stray_leave_and_a_gap_over_twelve_hours_count_nothing() {
        let (s, _) = bounds();
        let e = [
            ev(ME, "left", ROOM, s + H), // nothing was open
            ev(ME, "joined", ROOM, s + 2 * H),
            // The leave was never logged: the next event is 12h and a second on.
            ev(ME, "joined", ROOM, s + 14 * H + 1),
            ev(ME, "left", ROOM, s + 14 * H + 1801),
        ];
        assert_eq!(secs(&e, None)[&ME], 1800, "only the last half hour is believed");
        // Exactly twelve hours is still believed, as in awards.rs.
        let e = [ev(YOU, "joined", ROOM, s), ev(YOU, "left", ROOM, s + 12 * H)];
        assert_eq!(secs(&e, None)[&YOU], 12 * H);
    }

    #[test]
    fn a_sitting_across_midnight_is_split_between_the_days() {
        let (s, e_) = bounds();
        let e = [ev(ME, "joined", ROOM, s - 30 * 60), ev(ME, "left", ROOM, s + 45 * 60)];
        assert_eq!(secs(&e, None)[&ME], 45 * 60, "only the minutes after midnight belong to this day");
        let e = [ev(YOU, "joined", ROOM, e_ - 50 * 60), ev(YOU, "left", ROOM, e_ + 3 * H)];
        assert_eq!(secs(&e, None)[&YOU], 50 * 60);
        assert!(plan(day(), &HashMap::new(), &secs(&e, None)).is_empty(), "50 minutes before midnight is not a voice day");
    }

    #[test]
    fn a_room_still_open_counts_up_to_now_under_the_same_bar() {
        let (s, _) = bounds();
        let e = [ev(ME, "joined", ROOM, s + 20 * H), ev(YOU, "joined", AFK, s + 20 * H)];
        let got = secs(&e, Some(s + 21 * H + 60));
        assert_eq!(got.get(&ME), Some(&(H + 60)));
        assert_eq!(got.get(&YOU), None, "sitting in AFK is never voice time");
        // Open longer than twelve hours: the leave was lost, nothing counts.
        assert!(secs(&e, Some(s + 32 * H + 1)).get(&ME).is_none());
        // Every user's tail is closed, not only the last one's.
        let e = [ev(ME, "joined", ROOM, s + H), ev(YOU, "joined", ROOM, s + H), ev(YOU, "left", ROOM, s + 90 * 60)];
        let got = secs(&e, Some(s + 3 * H));
        assert_eq!((got[&ME], got[&YOU]), (2 * H, 30 * 60));
    }

    #[test]
    fn the_quarter_hour_timer_adds_a_last_call_before_midnight() {
        let (s, e) = bounds();
        assert_eq!(next_run(s + 10 * H + 1), s + 10 * H + EVERY);
        assert_eq!(next_run(s + 10 * H), s + 10 * H + EVERY, "never the same moment twice");
        assert_eq!(next_run(e - 15 * 60), e - LAST_CALL);
        assert_eq!(next_run(e - LAST_CALL), e);
    }

    fn stats_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE msg_counts (
                 user_id INTEGER NOT NULL, channel_id INTEGER NOT NULL,
                 day TEXT NOT NULL, hour INTEGER NOT NULL, count INTEGER NOT NULL,
                 PRIMARY KEY (user_id, channel_id, day, hour)) WITHOUT ROWID;
             CREATE TABLE voice_events (
                 msg_id INTEGER NOT NULL, user_id INTEGER NOT NULL, action TEXT NOT NULL,
                 channel_id INTEGER NOT NULL, ts INTEGER NOT NULL,
                 PRIMARY KEY (msg_id, user_id, action));",
        )
        .unwrap();
        conn.execute_batch(stats::VOICE_DEAF_SCHEMA).unwrap();
        add_indexes(&conn).unwrap();
        conn
    }

    #[test]
    fn a_day_is_read_from_the_stats_tables_and_reruns_ask_for_nothing_new() {
        let conn = stats_db();
        let (s, _) = bounds();
        let counts = [
            (ME, 1, "2026-09-14", 9, 15),
            (ME, 1, "2026-09-14", 22, 6),
            (YOU, 1, "2026-09-14", 9, 19),
            (YOU, 1, "2026-09-13", 9, 40), // the day before: not this day's
        ];
        for (user, channel, day, hour, count) in counts {
            let row = params![user as i64, channel, day, hour, count];
            conn.execute("INSERT INTO msg_counts VALUES (?1, ?2, ?3, ?4, ?5)", row).unwrap();
        }
        let voice = [
            (1, YOU, "joined", s + 8 * H),
            (2, YOU, "left", s + 9 * H + 5),
            // Company for the hour, so it counts under the with-others rule.
            (5, 44, "joined", s + 8 * H),
            (6, 44, "left", s + 9 * H + 5),
            // Joined 13h before midnight, left an hour after: over the 12h bar, so
            // nothing - and the join falling outside the query window changes nothing.
            (3, 33, "joined", s - 13 * H),
            (4, 33, "left", s + H),
        ];
        for (msg, user, action, ts) in voice {
            let row = params![msg, user as i64, action, ROOM as i64, ts];
            conn.execute("INSERT INTO voice_events VALUES (?1, ?2, ?3, ?4, ?5)", row).unwrap();
        }
        let now = s + 23 * H;
        let first = load_day(&conn, day(), now, &HashSet::new(), None, true, &HashSet::new()).unwrap();
        let keys: Vec<&str> = first.iter().map(|a| a.dedupe.as_str()).collect();
        assert_eq!(keys, vec!["chat:2026-09-14:11", "voice:2026-09-14:22:h1", "voice:2026-09-14:44:h1"]);
        // Voice log still importing: no voice days yet.
        let early = load_day(&conn, day(), now, &HashSet::new(), None, false, &HashSet::new()).unwrap();
        assert!(early.iter().all(|a| a.source == Source::Chat));

        // Through the ledger: the first pass pays, the rerun is refused outright.
        let ledger = Connection::open_in_memory().unwrap();
        ledger.execute_batch(points::SCHEMA).unwrap();
        let write = |a: &Award| {
            let entry = Entry {
                user: Some(a.user),
                house: &HOUSES[0],
                source: a.source,
                scope: None,
                points: 1,
                reason: &a.reason,
                by: None,
                dedupe: Some(a.dedupe.clone()),
            };
            points::write(&ledger, &entry, now).unwrap()
        };
        assert!(first.iter().map(write).all(|o| o == Outcome::Granted(1)));
        let again = load_day(&conn, day(), now + 900, &HashSet::new(), None, true, &HashSet::new()).unwrap();
        assert_eq!(again, first);
        assert!(again.iter().map(write).all(|o| o == Outcome::Duplicate));

        // YOU was deafened for ten minutes of that hour, recorded by the bot's
        // own voice states: now neither YOU nor 44 (whose only company YOU
        // was) has a full hour. A stretch still open runs to `now`.
        stats::apply_voice_state(&conn, YOU, Some(ROOM), true, s + 8 * H + 1200).unwrap();
        stats::apply_voice_state(&conn, YOU, Some(ROOM), false, s + 8 * H + 1800).unwrap();
        let deafened = load_day(&conn, day(), now, &HashSet::new(), None, true, &HashSet::new()).unwrap();
        let keys: Vec<&str> = deafened.iter().map(|a| a.dedupe.as_str()).collect();
        assert_eq!(keys, vec!["chat:2026-09-14:11"]);
        let (from, to) = bounds();
        assert_eq!(voice_between(&conn, Some(YOU), from, to, now).unwrap()[&YOU], H + 5 - 600);
        assert_eq!(voice_between(&conn, None, from, to, now).unwrap()[&44], H + 5, "44 was never deafened");
        let points = voice_points_between(&conn, None, from, to, now).unwrap();
        assert_eq!((points[&YOU], points[&44]), (H + 5 - 600, H + 5 - 600));
        stats::apply_voice_state(&conn, 44, Some(ROOM), true, s + 9 * H).unwrap();
        // Deafened from 9:00 and never undeafened: open to `now`, so the last five seconds go.
        assert_eq!(voice_between(&conn, Some(44), from, to, now).unwrap()[&44], H);
    }
}
