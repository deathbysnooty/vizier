//! Who left the server, and what they did while they were here.
//!
//! The bot keeps a join log (`joinlog__<id>` in its state store): how many
//! times someone joined and left, when they first came, and when they last
//! came and last went. Discord tells a bot nothing about people who are gone,
//! so that log is the only record there is.
//!
//! Someone counts as gone now when they have at least one leave and nothing
//! later says they came back: `last_leave` is after `last_join`, or there is no
//! `last_join` at all. Everything else on a row — house, points, messages,
//! voice — is read from the bot's own databases by id, and is simply zero for
//! someone the databases never saw.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{ApiError, ApiResult, Caller, Panel, ok, search};

pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 100;
pub const MAX_QUERY: usize = 100;
/// The periods the page offers, as days back from now.
pub const PERIODS: [(&str, Option<i64>); 4] = [("7", Some(7)), ("30", Some(30)), ("90", Some(90)), ("all", None)];
/// How far back the message and voice counts on a row look.
pub const RECENT_DAYS: i64 = 30;
/// The period the Overview tile counts.
pub const TILE_DAYS: i64 = 30;
const DAY: i64 = 86_400;

// --- what the data layer hands over ------------------------------------------------

/// One person in the join log, already folded across any alt accounts.
#[derive(Clone, Debug, Default, Serialize)]
pub struct JoinRow {
    pub id: u64,
    /// The name the bot stored when it last saw them, which may be all there is.
    pub name: String,
    pub joins: u32,
    pub leaves: u32,
    pub first_join: Option<String>,
    pub last_join: Option<String>,
    pub last_leave: Option<String>,
}

/// What the bot's databases still hold about someone. All zero for a member
/// they never recorded.
#[derive(Clone, Debug, Default, Serialize)]
pub struct LeftStats {
    /// Lifetime house points from the ledger.
    pub points: i64,
    pub messages_30d: i64,
    pub messages_all: i64,
    pub voice_30d_secs: i64,
    /// The India day of their last counted message, "YYYY-MM-DD".
    pub last_message_day: Option<String>,
    /// They had stepped out of the house game.
    pub muggle: bool,
}

// --- the rule -----------------------------------------------------------------------

/// A moment in the join log as a unix timestamp. It is written either with an
/// offset ("2026-09-10T00:55:12+00:00") or without one, in which case it is UTC.
pub fn moment(raw: &str) -> Option<i64> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Some(t.timestamp());
    }
    raw.parse::<chrono::NaiveDateTime>().ok().map(|t| t.and_utc().timestamp())
}

/// When they left, if they are gone now: they went at least once and nothing
/// later says they came back. A time that can't be read is no time at all.
pub fn gone_at(row: &JoinRow) -> Option<i64> {
    if row.leaves == 0 {
        return None;
    }
    let left = row.last_leave.as_deref().and_then(moment)?;
    match row.last_join.as_deref().and_then(moment) {
        Some(back) if back >= left => None,
        _ => Some(left),
    }
}

// --- one page of the list ------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Asked {
    /// The period as asked for ("7", "30", "90", "all").
    pub days_key: String,
    pub days: Option<i64>,
    /// Nothing older than this counts as having left in the period.
    pub since: i64,
    /// Part of the stored name, ignoring case.
    pub q: String,
    /// Newest departure (unix seconds) a page may hold, exclusive: the cursor.
    pub before: Option<i64>,
    pub limit: usize,
}

/// A member who is gone, ready to be shown.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: u64,
    pub name: String,
    pub joins: u32,
    pub leaves: u32,
    pub left_ts: i64,
    pub first_join_ts: Option<i64>,
    /// First join to last leave, when both are known: how long they were here.
    pub here_secs: Option<i64>,
}

#[derive(Clone, Debug, Default)]
pub struct Page {
    pub rows: Vec<Row>,
    /// Everyone gone in the period who matches the name, however many pages that is.
    pub total: usize,
    /// Where the next page carries on from, or none when that was the last.
    pub next_before: Option<i64>,
}

fn row_of(log: &JoinRow, left_ts: i64) -> Row {
    let first = log.first_join.as_deref().and_then(moment);
    Row {
        id: log.id,
        name: log.name.clone(),
        joins: log.joins,
        leaves: log.leaves,
        left_ts,
        first_join_ts: first,
        here_secs: first.filter(|f| left_ts > *f).map(|f| left_ts - f),
    }
}

/// Everyone in the join log who is gone now, newest departure first, cut to one
/// page. The whole log is small (one row per person the bot has ever seen), so
/// the period, the name and the page are all settled here.
pub fn page(logs: &[JoinRow], a: &Asked) -> Page {
    let mut all: Vec<Row> = logs
        .iter()
        .filter_map(|log| {
            let left = gone_at(log)?;
            (left >= a.since).then(|| row_of(log, left))
        })
        .filter(|r| a.q.is_empty() || search::find_ci(&r.name, &a.q).is_some())
        .collect();
    // Newest first, and a stable order for two people who left in the same second.
    all.sort_by(|x, y| y.left_ts.cmp(&x.left_ts).then(x.id.cmp(&y.id)));
    let total = all.len();
    let mut rows: Vec<Row> = match a.before {
        Some(before) => all.into_iter().filter(|r| r.left_ts < before).collect(),
        None => all,
    };
    let mut next_before = None;
    if rows.len() > a.limit {
        // Keep whole seconds: the cursor is exclusive, so a second split across
        // two pages would lose whoever fell on the far side of the cut.
        let edge = rows[a.limit - 1].left_ts;
        let keep = rows.iter().take_while(|r| r.left_ts >= edge).count();
        rows.truncate(keep);
        next_before = Some(edge);
    }
    Page { rows, total, next_before }
}

// --- what the databases still hold ----------------------------------------------------

/// Everything the bot's own databases know about these members, read off the
/// async threads. Each database lock is taken once and dropped before the next.
pub fn read_stats_live(ids: &[u64], now: i64) -> HashMap<u64, LeftStats> {
    let mut out: HashMap<u64, LeftStats> = ids.iter().map(|id| (*id, LeftStats::default())).collect();
    if out.is_empty() {
        return out;
    }
    // This takes the house lock itself: before it is taken here.
    let optouts = super::super::super::house::optout_set();
    for (id, stats) in out.iter_mut() {
        stats.muggle = optouts.contains(id);
    }
    if let Some(db) = super::super::super::house::db() {
        let conn = db.lock();
        let _ = points_for(&conn, &mut out);
    }
    if let Some(db) = super::super::super::stats::db() {
        let conn = db.lock();
        let _ = messages_for(&conn, now, &mut out);
        let _ = voice_for(&conn, now, &mut out);
    }
    out
}

/// Lifetime house points, from the ledger these members are named in.
pub fn points_for(conn: &Connection, out: &mut HashMap<u64, LeftStats>) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare_cached("SELECT COALESCE(SUM(points), 0) FROM ledger WHERE user_id = ?1")?;
    for (id, stats) in out.iter_mut() {
        stats.points = stmt.query_row(params![*id as i64], |r| r.get(0)).optional()?.unwrap_or(0);
    }
    Ok(())
}

/// Messages over the last 30 India days, all time, and the day of the last one.
pub fn messages_for(conn: &Connection, now: i64, out: &mut HashMap<u64, LeftStats>) -> rusqlite::Result<()> {
    let recent = super::super::super::points::ist_day(now - (RECENT_DAYS - 1) * DAY);
    let mut stmt = conn.prepare_cached(
        "SELECT COALESCE(SUM(count), 0), COALESCE(SUM(CASE WHEN day >= ?2 THEN count ELSE 0 END), 0), MAX(day)
         FROM msg_counts WHERE user_id = ?1",
    )?;
    for (id, stats) in out.iter_mut() {
        let row = stmt
            .query_row(params![*id as i64, recent], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<String>>(2)?))
            })
            .optional()?;
        if let Some((all, recent, last)) = row {
            stats.messages_all = all;
            stats.messages_30d = recent;
            stats.last_message_day = last;
        }
    }
    Ok(())
}

/// Time in voice over the last 30 days, each member read through the (user,
/// time) index rather than by scanning the whole month.
pub fn voice_for(conn: &Connection, now: i64, out: &mut HashMap<u64, LeftStats>) -> rusqlite::Result<()> {
    let start = now - RECENT_DAYS * DAY;
    for (id, stats) in out.iter_mut() {
        let secs = super::super::super::activity::voice_between(conn, Some(*id), start, now, now)?;
        stats.voice_30d_secs = secs.get(id).copied().unwrap_or(0);
    }
    Ok(())
}

// --- the endpoint ----------------------------------------------------------------------

#[derive(Deserialize)]
pub struct LeftQuery {
    #[serde(default)]
    days: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    before: Option<String>,
    #[serde(default)]
    limit: Option<String>,
}

fn read_query(q: &LeftQuery, now: i64) -> Result<Asked, ApiError> {
    let blank = |v: &Option<String>| v.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(String::from);
    let days_key = blank(&q.days).unwrap_or_else(|| "30".to_string());
    let days = PERIODS
        .iter()
        .find(|(k, _)| *k == days_key)
        .map(|(_, d)| *d)
        .ok_or_else(|| ApiError::bad("The period is 7, 30, 90 or all (days)."))?;
    let text = blank(&q.q).unwrap_or_default();
    if text.chars().count() > MAX_QUERY {
        return Err(ApiError::bad(format!("Keep the name under {} characters.", MAX_QUERY)));
    }
    let before = match blank(&q.before) {
        Some(raw) => Some(raw.parse::<i64>().ok().filter(|b| *b > 0).ok_or_else(|| ApiError::bad("That isn't a place to carry on from."))?),
        None => None,
    };
    let limit = match blank(&q.limit) {
        Some(raw) => raw.parse::<usize>().map_err(|_| ApiError::bad("The limit is a number."))?.clamp(1, MAX_LIMIT),
        None => DEFAULT_LIMIT,
    };
    Ok(Asked { days_key, days, since: days.map(|d| now - d * DAY).unwrap_or(i64::MIN), q: text, before, limit })
}

/// How the period reads in the activity log.
fn period_words(days: Option<i64>) -> String {
    match days {
        Some(d) => format!("last {} days", d),
        None => "all time".to_string(),
    }
}

/// The join log, kept for a minute: the page reads it, and so does the Overview
/// tile on every status poll.
pub(super) async fn logs(panel: &Panel) -> Vec<JoinRow> {
    static CACHE: std::sync::LazyLock<Mutex<Option<(Instant, Vec<JoinRow>)>>> = std::sync::LazyLock::new(|| Mutex::new(None));
    const FRESH: Duration = Duration::from_secs(60);
    if let Some((at, rows)) = CACHE.lock().as_ref() {
        if at.elapsed() < FRESH {
            return rows.clone();
        }
    }
    let rows = panel.data.join_logs().await;
    *CACHE.lock() = Some((Instant::now(), rows.clone()));
    rows
}

/// How many members left in the last 30 days, for the Overview tile. Counting
/// is not reading the list, so it isn't in the activity log.
pub async fn recent_count(panel: &Panel, now: i64) -> usize {
    let since = now - TILE_DAYS * DAY;
    logs(panel).await.iter().filter_map(gone_at).filter(|left| *left >= since).count()
}

pub async fn list(State(panel): State<Panel>, axum::Extension(Caller(user)): axum::Extension<Caller>, Query(q): Query<LeftQuery>) -> ApiResult {
    let now = chrono::Utc::now().timestamp();
    let asked = read_query(&q, now)?;
    let audit = format!(
        "{} · {}",
        period_words(asked.days),
        if asked.q.is_empty() { "any name".to_string() } else { format!("“{}”", asked.q) }
    );
    search::log_quietly("members:left", user, &audit);

    let found = page(&logs(&panel).await, &asked);
    let ids: Vec<u64> = found.rows.iter().map(|r| r.id).collect();
    let stats = panel.data.left_stats(ids.clone(), now).await;
    // The house store is read off the async threads, as the members page does.
    let data = panel.data.clone();
    let houses: HashMap<u64, &'static super::super::super::house::House> =
        tokio::task::spawn_blocking(move || ids.into_iter().filter_map(|id| data.member_house(id).map(|h| (id, h))).collect())
            .await
            .unwrap_or_default();

    let results: Vec<Value> = found
        .rows
        .iter()
        .map(|r| {
            let who = panel.data.cached_member(r.id);
            let s = stats.get(&r.id).cloned().unwrap_or_default();
            json!({
                "id": r.id.to_string(),
                "name": who.as_ref().map(|m| m.name.clone()).filter(|n| !n.trim().is_empty())
                    .or_else(|| (!r.name.trim().is_empty()).then(|| r.name.clone()))
                    .unwrap_or_else(|| format!("Member {}", r.id)),
                "avatar": who.as_ref().map(|m| m.avatar.clone()),
                "in_server": who.is_some(),
                "house": houses.get(&r.id).map(|h| json!({ "key": h.key, "name": h.name, "crest": h.crest, "colour": format!("#{:06x}", h.colour) })),
                "muggle": s.muggle,
                "left_ts": r.left_ts,
                "first_join_ts": r.first_join_ts,
                "here_secs": r.here_secs,
                "joins": r.joins,
                "leaves": r.leaves,
                "points": s.points,
                "messages_30d": s.messages_30d,
                "messages_all": s.messages_all,
                "voice_30d_min": s.voice_30d_secs / 60,
                "last_message_day": s.last_message_day,
            })
        })
        .collect();
    ok(json!({
        "days": asked.days_key,
        "q": asked.q,
        "limit": asked.limit,
        "count": results.len(),
        "total": found.total,
        "results": results,
        "next_before": found.next_before,
        "recent_days": RECENT_DAYS,
    }))
}

/// How a look at the list reads in the activity log.
pub fn audit_entry(e: &super::super::AuditEntry) -> serde_json::Map<String, Value> {
    let mut obj = serde_json::Map::new();
    obj.insert("label".into(), json!("Looked at who left"));
    obj.insert("section".into(), json!({ "id": "left", "title": "Left the server", "icon": "🚪" }));
    obj.insert("change".into(), json!(e.new.clone().unwrap_or_else(|| "Looked".into())));
    obj.insert("old".into(), Value::Null);
    obj.insert("new".into(), Value::Null);
    obj
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(id: u64, name: &str, joins: u32, leaves: u32, first: Option<&str>, last_join: Option<&str>, last_leave: Option<&str>) -> JoinRow {
        JoinRow {
            id,
            name: name.into(),
            joins,
            leaves,
            first_join: first.map(String::from),
            last_join: last_join.map(String::from),
            last_leave: last_leave.map(String::from),
        }
    }

    #[test]
    fn moments_are_read_with_or_without_an_offset() {
        assert_eq!(moment("2026-09-10T00:55:12+00:00"), Some(1_789_001_712));
        assert_eq!(moment("2026-09-10T00:55:12Z"), Some(1_789_001_712));
        assert_eq!(moment("2026-09-10T00:55:12"), Some(1_789_001_712), "no offset means UTC");
        assert_eq!(moment("2026-09-10T06:25:12+05:30"), Some(1_789_001_712));
        assert_eq!(moment("2026-09-10T00:55:12.480"), Some(1_789_001_712));
        assert_eq!(moment(""), None);
        assert_eq!(moment("sometime last year"), None);
    }

    #[test]
    fn only_people_who_havent_come_back_are_gone() {
        // Left and stayed away.
        let left = log(1, "ofcadeath", 4, 4, Some("2026-06-18T14:43:44"), Some("2026-09-09T00:55:12"), Some("2026-09-10T17:55:07"));
        assert_eq!(gone_at(&left), moment("2026-09-10T17:55:07"));
        // Came back after the last leave.
        let back = log(2, "returned", 4, 3, Some("2026-06-18T14:43:44"), Some("2026-09-10T00:55:12"), Some("2026-09-09T17:55:07"));
        assert_eq!(gone_at(&back), None);
        // Never left at all.
        let stayed = log(3, "stayed", 1, 0, Some("2026-06-18T14:43:44"), Some("2026-06-18T14:43:44"), None);
        assert_eq!(gone_at(&stayed), None);
        // A leave with no join recorded: gone.
        let seeded = log(4, "dyno only", 0, 1, None, None, Some("2026-05-02T10:00:00"));
        assert_eq!(gone_at(&seeded), moment("2026-05-02T10:00:00"));
        // The same second is not a comeback.
        let same = log(5, "same second", 2, 1, None, Some("2026-05-02T10:00:00"), Some("2026-05-02T10:00:00"));
        assert_eq!(gone_at(&same), None);
        // Leaves counted but no time for them: nothing to show.
        assert_eq!(gone_at(&log(6, "no time", 1, 1, None, None, None)), None);
        assert_eq!(gone_at(&log(7, "junk", 1, 1, None, None, Some("last tuesday"))), None);
    }

    /// Everyone left this many days ago, so a period is easy to reason about.
    fn logs_days_ago(now: i64, people: &[(u64, &str, i64)]) -> Vec<JoinRow> {
        people
            .iter()
            .map(|(id, name, days)| {
                let left = chrono::DateTime::from_timestamp(now - days * DAY, 0).unwrap();
                let first = chrono::DateTime::from_timestamp(now - (days + 400) * DAY, 0).unwrap();
                log(*id, name, 1, 1, Some(&first.to_rfc3339()), None, Some(&left.to_rfc3339()))
            })
            .collect()
    }

    fn asked(days: Option<i64>, q: &str, before: Option<i64>, limit: usize, now: i64) -> Asked {
        Asked {
            days_key: days.map(|d| d.to_string()).unwrap_or_else(|| "all".into()),
            days,
            since: days.map(|d| now - d * DAY).unwrap_or(i64::MIN),
            q: q.into(),
            before,
            limit,
        }
    }

    #[test]
    fn the_period_and_the_name_narrow_the_list() {
        let now = 1_789_000_000;
        let mut logs = logs_days_ago(now, &[(1, "Aarav", 2), (2, "Diya", 20), (3, "aaravi", 45), (4, "Meera", 200)]);
        // Someone who came back is never in the list, however recently they went.
        logs.push(log(5, "Back Again", 3, 2, None, Some("2099-01-01T00:00:00"), Some("2098-01-01T00:00:00")));
        let names = |a: &Asked| page(&logs, a).rows.iter().map(|r| r.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&asked(Some(7), "", None, 50, now)), vec!["Aarav"]);
        assert_eq!(names(&asked(Some(30), "", None, 50, now)), vec!["Aarav", "Diya"]);
        assert_eq!(names(&asked(Some(90), "", None, 50, now)), vec!["Aarav", "Diya", "aaravi"]);
        assert_eq!(names(&asked(None, "", None, 50, now)), vec!["Aarav", "Diya", "aaravi", "Meera"]);
        // The name is matched anywhere in it, ignoring case.
        assert_eq!(names(&asked(None, "AARAV", None, 50, now)), vec!["Aarav", "aaravi"]);
        assert_eq!(names(&asked(Some(7), "aarav", None, 50, now)), vec!["Aarav"]);
        assert!(names(&asked(None, "nobody", None, 50, now)).is_empty());
        // The count is for the whole period, not the page.
        assert_eq!(page(&logs, &asked(None, "", None, 2, now)).total, 4);
        assert_eq!(page(&logs, &asked(Some(30), "", None, 50, now)).total, 2);
    }

    #[test]
    fn rows_carry_how_long_they_were_here() {
        let now = 1_789_000_000;
        let logs = logs_days_ago(now, &[(1, "Aarav", 2)]);
        let row = &page(&logs, &asked(None, "", None, 50, now)).rows[0];
        assert_eq!(row.left_ts, now - 2 * DAY);
        assert_eq!(row.here_secs, Some(400 * DAY));
        assert_eq!((row.joins, row.leaves), (1, 1));
        // No first join: nothing to measure.
        let bare = vec![log(9, "bare", 1, 1, None, None, Some("2026-05-02T10:00:00"))];
        let row = &page(&bare, &asked(None, "", None, 50, now)).rows[0];
        assert_eq!((row.here_secs, row.first_join_ts), (None, None));
    }

    #[test]
    fn pages_carry_on_where_the_last_one_stopped() {
        let now = 1_789_000_000;
        let people: Vec<(u64, String, i64)> = (0..7).map(|i| (i as u64 + 1, format!("Gone{}", i), i + 1)).collect();
        let logs = logs_days_ago(now, &people.iter().map(|(id, n, d)| (*id, n.as_str(), *d)).collect::<Vec<_>>());
        let first = page(&logs, &asked(None, "", None, 3, now));
        assert_eq!(first.rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), vec!["Gone0", "Gone1", "Gone2"]);
        assert_eq!((first.total, first.next_before), (7, Some(now - 3 * DAY)));
        let second = page(&logs, &asked(None, "", first.next_before, 3, now));
        assert_eq!(second.rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), vec!["Gone3", "Gone4", "Gone5"]);
        let third = page(&logs, &asked(None, "", second.next_before, 3, now));
        assert_eq!(third.rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), vec!["Gone6"]);
        assert_eq!(third.next_before, None, "nothing older is left");
        // The cursor never splits a second: three people who left at once come together.
        let together: Vec<JoinRow> = (1..=3).map(|i| logs_days_ago(now, &[(i, "Same", 5)])[0].clone()).collect();
        let one = page(&together, &asked(None, "", None, 1, now));
        assert_eq!(one.rows.len(), 3, "a whole second is kept");
        assert_eq!(one.next_before, Some(now - 5 * DAY));
        assert!(page(&together, &asked(None, "", one.next_before, 1, now)).rows.is_empty());
    }

    #[test]
    fn the_databases_fill_in_what_they_have_and_zero_for_the_rest() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(super::super::super::super::points::SCHEMA).unwrap();
        conn.execute_batch(
            "CREATE TABLE msg_counts (user_id INTEGER NOT NULL, channel_id INTEGER NOT NULL, day TEXT NOT NULL,
                 hour INTEGER NOT NULL, count INTEGER NOT NULL, PRIMARY KEY (user_id, channel_id, day, hour));",
        )
        .unwrap();
        // 2026-09-14 12:00 India time.
        let now = 1_789_367_400;
        let day = |back: i64| super::super::super::super::points::ist_day(now - back * DAY);
        for (user, points) in [(1u64, 30i64), (1, 12), (2, 7)] {
            conn.execute(
                "INSERT INTO ledger (user_id, house, source, points, day, ts) VALUES (?1, 'gryffindor', 'chat', ?2, ?3, ?4)",
                params![user as i64, points, day(0), now],
            )
            .unwrap();
        }
        for (user, back, count) in [(1u64, 0i64, 5i64), (1, 12, 9), (1, 400, 50), (2, 200, 3)] {
            conn.execute(
                "INSERT INTO msg_counts (user_id, channel_id, day, hour, count) VALUES (?1, 21, ?2, 10, ?3)",
                params![user as i64, day(back), count],
            )
            .unwrap();
        }
        let mut out: HashMap<u64, LeftStats> = [1u64, 2, 3].into_iter().map(|id| (id, LeftStats::default())).collect();
        points_for(&conn, &mut out).unwrap();
        messages_for(&conn, now, &mut out).unwrap();
        assert_eq!(out[&1].points, 42);
        assert_eq!((out[&1].messages_all, out[&1].messages_30d), (64, 14));
        assert_eq!(out[&1].last_message_day.as_deref(), Some(day(0).as_str()));
        assert_eq!((out[&2].points, out[&2].messages_all, out[&2].messages_30d), (7, 3, 0));
        // Someone the databases never saw is all zeros, not a missing row.
        assert_eq!((out[&3].points, out[&3].messages_all, out[&3].messages_30d), (0, 0, 0));
        assert_eq!(out[&3].last_message_day, None);
    }
}
