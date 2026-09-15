//! Scorers today: everyone who earned house points on an India day, and anyone
//! close to the chat or voice point, with how far each activity is from its
//! daily limit.

use std::collections::{HashMap, HashSet};

use axum::extract::{Query, State};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::super::super::activity;
use super::super::super::house::{self, HOUSES};
use super::super::super::points::{self, Cap, Source};
use super::{ApiError, ApiResult, Panel, ok};

pub const MAX_ROWS: usize = 300;
const IST: i64 = 5 * 3600 + 30 * 60;

#[derive(Clone, Debug, Serialize)]
pub struct ScorerData {
    pub user_id: u64,
    pub house: String,
    /// Source key and points on the day.
    pub sources: HashMap<String, i64>,
    /// Weekly-scan points in that day's week.
    pub weekly_week: i64,
    /// Messages on the day, as the chat point counts them.
    pub messages: i64,
    pub voice_secs: i64,
}

/// `[start, end)` of today (offset 0) or yesterday (offset 1), India time.
pub fn day_bounds(now: i64, days_back: i64) -> (i64, i64) {
    let today = (now + IST).div_euclid(86_400) * 86_400 - IST;
    let start = today - days_back * 86_400;
    (start, start + 86_400)
}

/// Reads the day from the live databases: opt-outs first (they take the house
/// lock), then the house database, then the stats database, one lock at a time.
pub fn read_live(days_back: i64, now: i64) -> Option<Vec<ScorerData>> {
    let optouts = house::optout_set();
    let (start, end) = day_bounds(now, days_back);
    let (ledger, members) = {
        let db = house::db()?;
        let conn = db.lock();
        (read_ledger(&conn, start, end).ok()?, read_members(&conn).ok()?)
    };
    let (messages, voice) = match super::super::super::stats::db() {
        Some(db) => {
            let conn = db.lock();
            let day = points::ist_day(start);
            (
                activity::messages_on(&conn, &day, None).unwrap_or_default(),
                activity::voice_points_between(&conn, None, start, end, now.min(end)).unwrap_or_default(),
            )
        }
        None => (HashMap::new(), HashMap::new()),
    };
    Some(assemble(ledger, &members, &messages, &voice, &optouts, activity::chat_bar(), activity::voice_bar_secs()))
}

/// (user, house, source, points) summed over the day, plus weekly points over
/// the week, through the (house, ts) index.
type LedgerDay = (Vec<(u64, String, String, i64)>, HashMap<u64, i64>);

pub fn read_ledger(conn: &Connection, start: i64, end: i64) -> rusqlite::Result<LedgerDay> {
    let keys: Vec<&str> = HOUSES.iter().map(|h| h.key).collect();
    let mut rows = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT user_id, house, source, SUM(points) FROM ledger
         WHERE house = ?1 AND ts >= ?2 AND ts < ?3 AND user_id IS NOT NULL GROUP BY user_id, source",
    )?;
    for key in &keys {
        rows.extend(
            stmt.query_map(params![key, start, end], |r| {
                Ok((r.get::<_, i64>(0)? as u64, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?))
            })?
            .flatten(),
        );
    }
    let week_start = {
        let weekday = ((start + IST).div_euclid(86_400) + 3).rem_euclid(7);
        start - weekday * 86_400
    };
    let mut weekly = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT user_id, SUM(points) FROM ledger WHERE house = ?1 AND ts >= ?2 AND ts < ?3 AND source = 'weekly'
         AND user_id IS NOT NULL GROUP BY user_id",
    )?;
    for key in &keys {
        for (u, n) in stmt.query_map(params![key, week_start, end], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)?)))?.flatten() {
            weekly.insert(u, n);
        }
    }
    Ok((rows, weekly))
}

pub fn read_members(conn: &Connection) -> rusqlite::Result<HashMap<u64, String>> {
    let mut stmt = conn.prepare("SELECT user_id, house FROM members")?;
    let out = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, String>(1)?)))?.flatten().collect();
    Ok(out)
}

/// Everyone who scored, plus anyone at half the chat or voice bar, Muggles left
/// out, biggest total first.
pub fn assemble(
    (ledger, weekly): LedgerDay,
    members: &HashMap<u64, String>,
    messages: &HashMap<u64, i64>,
    voice: &HashMap<u64, i64>,
    optouts: &HashSet<u64>,
    chat_bar: i64,
    voice_bar: i64,
) -> Vec<ScorerData> {
    let mut by_user: HashMap<u64, ScorerData> = HashMap::new();
    fn entry<'a>(by_user: &'a mut HashMap<u64, ScorerData>, optouts: &HashSet<u64>, user: u64, house: &str) -> Option<&'a mut ScorerData> {
        if optouts.contains(&user) || user == 0 {
            return None;
        }
        Some(by_user.entry(user).or_insert_with(|| ScorerData {
            user_id: user,
            house: house.to_string(),
            sources: HashMap::new(),
            weekly_week: 0,
            messages: 0,
            voice_secs: 0,
        }))
    }
    for (user, house, source, pts) in &ledger {
        if let Some(row) = entry(&mut by_user, optouts, *user, house) {
            *row.sources.entry(source.clone()).or_insert(0) += pts;
        }
    }
    for (user, n) in messages {
        if let (true, Some(house)) = (*n * 2 >= chat_bar, members.get(user)) {
            entry(&mut by_user, optouts, *user, house);
        }
    }
    for (user, secs) in voice {
        if let (true, Some(house)) = (*secs * 2 >= voice_bar, members.get(user)) {
            entry(&mut by_user, optouts, *user, house);
        }
    }
    let mut rows: Vec<ScorerData> = by_user
        .into_values()
        .map(|mut r| {
            r.messages = messages.get(&r.user_id).copied().unwrap_or(0);
            r.voice_secs = voice.get(&r.user_id).copied().unwrap_or(0);
            r.weekly_week = weekly.get(&r.user_id).copied().unwrap_or(0);
            r
        })
        .collect();
    let total = |r: &ScorerData| r.sources.values().sum::<i64>();
    rows.sort_by(|a, b| {
        total(b).cmp(&total(a)).then((b.messages * 1000 / chat_bar.max(1)).cmp(&(a.messages * 1000 / chat_bar.max(1)))).then(a.user_id.cmp(&b.user_id))
    });
    rows.truncate(MAX_ROWS);
    rows
}

/// The activity chips for one member's day: chat and voice progress towards
/// their point, each capped source against its limit (read now, so a changed
/// limit shows at once), and the uncapped ones.
pub fn activity_chips(sources: &HashMap<String, i64>, messages: i64, voice_secs: i64, weekly_week: i64) -> Value {
    let pts = |s: Source| sources.get(s.key()).copied().unwrap_or(0);
    let label = |s: Source| {
        let l = s.label();
        l.split_once(' ').map(|(i, n)| (i.to_string(), n.to_string())).unwrap_or_default()
    };
    let mut out = Vec::new();
    let cap_of = |s: Source| match s.cap() {
        Cap::PerDay(n) => Some(n),
        _ => None,
    };
    // Chat pays a point per tier; the target is the next tier still to reach.
    let tiers = activity::chat_tier_bars();
    let chat_pts = pts(Source::Chat);
    let chat_cap = cap_of(Source::Chat).unwrap_or(tiers.len() as i64).min(tiers.len() as i64);
    let next_tier = tiers.iter().copied().find(|t| messages < *t);
    let (icon, name) = label(Source::Chat);
    out.push(json!({
        "key": "chat", "icon": icon, "label": name, "kind": "progress",
        "count": messages, "target": next_tier.unwrap_or(*tiers.last().unwrap_or(&0)), "unit": "msgs",
        "points": chat_pts, "cap": chat_cap, "tiers": tiers,
        "reached": chat_cap > 0 && chat_pts >= chat_cap,
    }));
    // Voice pays a point per full block (an hour by default) of time that counts.
    let voice_bar = activity::voice_bar_secs().max(60);
    let voice_pts = pts(Source::Voice);
    let voice_cap = cap_of(Source::Voice).unwrap_or(0);
    let next_block = ((voice_secs / voice_bar) + 1) * voice_bar;
    let (icon, name) = label(Source::Voice);
    out.push(json!({
        "key": "voice", "icon": icon, "label": name, "kind": "progress",
        "count": voice_secs / 60, "target": next_block / 60, "unit": "min",
        "points": voice_pts, "cap": voice_cap, "per_point_min": voice_bar / 60,
        "with_company": activity::voice_company_rule(),
        "reached": voice_cap > 0 && voice_pts >= voice_cap,
    }));
    for s in [Source::Quiz, Source::Koto, Source::Anagram, Source::Cat, Source::Arena, Source::Snitch, Source::Frog, Source::Npat] {
        let (icon, name) = label(s);
        let cap = match s.cap() {
            Cap::PerDay(n) => Some(n),
            _ => None,
        };
        out.push(json!({
            "key": s.key(), "icon": icon, "label": name, "kind": "capped",
            "points": pts(s), "cap": cap, "reached": cap.is_some_and(|c| pts(s) >= c),
        }));
    }
    for s in [Source::Wordle, Source::GoldenSnitch, Source::Royale, Source::Mod] {
        if pts(s) != 0 {
            let (icon, name) = label(s);
            out.push(json!({ "key": s.key(), "icon": icon, "label": name, "kind": "extra", "points": pts(s) }));
        }
    }
    if pts(Source::Weekly) != 0 || weekly_week != 0 {
        let (icon, name) = label(Source::Weekly);
        let cap = match Source::Weekly.cap() {
            Cap::PerWeekPerChannel(n) => Some(n),
            _ => None,
        };
        out.push(json!({
            "key": "weekly", "icon": icon, "label": name, "kind": "weekly",
            "points": pts(Source::Weekly), "week": weekly_week, "cap_per_channel": cap,
        }));
    }
    Value::Array(out)
}

#[derive(Deserialize)]
pub struct ScorersQuery {
    #[serde(default)]
    day: Option<String>,
    #[serde(default)]
    house: Option<String>,
}

pub async fn get(State(panel): State<Panel>, Query(q): Query<ScorersQuery>) -> ApiResult {
    let days_back = match q.day.as_deref().unwrap_or("today") {
        "today" => 0,
        "yesterday" => 1,
        _ => return Err(ApiError::bad("Day is today or yesterday.")),
    };
    let house_filter = match q.house.as_deref().map(str::trim).filter(|h| !h.is_empty() && *h != "all") {
        Some(key) => Some(house::house(key).ok_or_else(|| ApiError::bad("No such house."))?.key),
        None => None,
    };
    let now = chrono::Utc::now().timestamp();
    let Some(rows) = panel.data.scorers(days_back, now) else {
        return Err(ApiError(axum::http::StatusCode::SERVICE_UNAVAILABLE, "The house points aren't available right now.".into()));
    };
    let (start, _) = day_bounds(now, days_back);
    let mut rank = 0;
    let out: Vec<Value> = rows
        .iter()
        .filter_map(|r| {
            let who = panel.data.cached_member(r.user_id);
            if who.as_ref().is_some_and(|m| m.bot) {
                return None;
            }
            rank += 1;
            if house_filter.is_some_and(|h| h != r.house) {
                return None;
            }
            let meta = house::house(&r.house);
            Some(json!({
                "rank": rank,
                "id": r.user_id.to_string(),
                "name": who.as_ref().map(|m| m.name.clone()),
                "avatar": who.map(|m| m.avatar),
                "house": r.house,
                "crest": meta.map(|h| h.crest),
                "house_name": meta.map(|h| h.name),
                "colour": meta.map(|h| format!("#{:06x}", h.colour)),
                "total": r.sources.values().sum::<i64>(),
                "activities": activity_chips(&r.sources, r.messages, r.voice_secs, r.weekly_week),
            }))
        })
        .collect();
    ok(json!({
        "day": points::ist_day(start),
        "which": if days_back == 0 { "today" } else { "yesterday" },
        "now": now,
        "chat_bar": activity::chat_bar(),
        "voice_bar_min": activity::voice_bar_secs() / 60,
        "rows": out,
        "limit": MAX_ROWS,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scorers_include_near_misses_and_skip_muggles() {
        let ledger = vec![
            (1u64, "gryffindor".to_string(), "quiz".to_string(), 6),
            (1, "gryffindor".to_string(), "chat".to_string(), 1),
            (2, "slytherin".to_string(), "arena".to_string(), 3),
            (3, "ravenclaw".to_string(), "snitch".to_string(), 5),
        ];
        let weekly: HashMap<u64, i64> = [(2, 3)].into_iter().collect();
        let members: HashMap<u64, String> =
            [(4, "hufflepuff".to_string()), (5, "hufflepuff".to_string()), (6, "gryffindor".to_string())].into_iter().collect();
        let messages: HashMap<u64, i64> = [(1, 25), (4, 12), (5, 3), (9, 30)].into_iter().collect();
        let voice: HashMap<u64, i64> = [(6, 40 * 60)].into_iter().collect();
        let optouts: HashSet<u64> = [3].into_iter().collect();
        let rows = assemble((ledger, weekly), &members, &messages, &voice, &optouts, 20, 3600);
        let ids: Vec<u64> = rows.iter().map(|r| r.user_id).collect();
        assert_eq!(ids[0], 1, "biggest total first");
        assert!(ids.contains(&4), "12 of 20 messages is close enough to show");
        assert!(!ids.contains(&5), "3 of 20 is not");
        assert!(ids.contains(&6), "40 of 60 voice minutes shows");
        assert!(!ids.contains(&3), "Muggles are left out");
        assert!(!ids.contains(&9), "unsorted members can't score");
        assert_eq!(rows.iter().find(|r| r.user_id == 1).unwrap().messages, 25);
        assert_eq!(rows.iter().find(|r| r.user_id == 2).unwrap().weekly_week, 3);
    }

    #[test]
    fn the_ledger_day_uses_house_and_time() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(points::SCHEMA).unwrap();
        let now = 1_789_331_400; // 02:00 IST, Monday 14 September 2026
        let (start, end) = day_bounds(now, 0);
        let add = |user: i64, house: &str, source: &str, pts: i64, ts: i64| {
            conn.execute(
                "INSERT INTO ledger (user_id, house, source, points, day, ts) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![user, house, source, pts, points::ist_day(ts), ts],
            )
            .unwrap();
        };
        add(1, "gryffindor", "quiz", 2, start + 10);
        add(1, "gryffindor", "quiz", 3, start + 20);
        add(1, "gryffindor", "quiz", 9, start - 10);
        add(2, "slytherin", "weekly", 3, start + 30);
        let (rows, weekly) = read_ledger(&conn, start, end).unwrap();
        assert!(rows.contains(&(1, "gryffindor".to_string(), "quiz".to_string(), 5)));
        assert_eq!(weekly.get(&2), Some(&3));
        assert_eq!(day_bounds(now, 1).1, start);
    }
}
