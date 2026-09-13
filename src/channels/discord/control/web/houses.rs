//! The House Cup page: live standings, where points came from, top scorers and
//! the latest ledger rows. Read-only.

use std::collections::{HashMap, HashSet};

use axum::extract::{Query, State};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::super::super::house::{self, HOUSES};
use super::super::super::points::{self, Source};
use super::{ApiError, ApiResult, MemberInfo, Panel, ok};

/// Which stretch of time the page shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Period {
    Today,
    Week,
    #[default]
    Month,
    LastMonth,
    All,
}

const IST: i64 = 5 * 3600 + 30 * 60;

impl Period {
    /// `[since, until)` in Unix seconds for a moment `now`, on India days.
    pub fn range(self, now: i64) -> (i64, i64) {
        let today = (now + IST).div_euclid(86_400) * 86_400 - IST;
        match self {
            Period::Today => (today, i64::MAX),
            Period::Week => {
                // 1970-01-01 was a Thursday: day 0 is weekday 3 counting from Monday.
                let weekday = ((now + IST).div_euclid(86_400) + 3).rem_euclid(7);
                (today - weekday * 86_400, i64::MAX)
            }
            Period::Month => (points::month_start(now), i64::MAX),
            Period::LastMonth => {
                let this = points::month_start(now);
                (points::month_start(this - 1), this)
            }
            Period::All => (0, i64::MAX),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Scorer {
    pub user_id: u64,
    pub points: i64,
    /// The source most of their points came from.
    pub main_source: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Standing {
    pub key: String,
    pub total: i64,
    pub captain: Option<u64>,
    pub members: i64,
    /// Source key and points in the period, biggest first.
    pub by_source: Vec<(String, i64)>,
    pub today_by_source: Vec<(String, i64)>,
    pub top: Vec<Scorer>,
}

#[derive(Clone, Debug, Serialize)]
pub struct FeedRow {
    pub user_id: Option<u64>,
    pub house: String,
    pub source: String,
    pub points: i64,
    pub reason: String,
    pub ts: i64,
}

/// Everything the page needs, without any Discord names in it.
#[derive(Clone, Debug, Serialize)]
pub struct HouseCup {
    pub since: i64,
    pub until: i64,
    pub houses: Vec<Standing>,
    pub feed: Vec<FeedRow>,
}

/// Reads the standings from the live house database. Everything that takes the
/// house lock by itself (opt-outs, captains, member counts) is read before this
/// takes it: the lock isn't re-entrant.
pub fn read_live(period: Period, now: i64) -> Option<HouseCup> {
    let optouts = house::optout_set();
    let captains: HashMap<&'static str, Option<u64>> = HOUSES.iter().map(|h| (h.key, house::captain_id(h.key))).collect();
    let counts = house::counts();
    let db = house::db()?;
    let conn = db.lock();
    read(&conn, period, now, &optouts, &captains, &counts).ok()
}

pub fn read(
    conn: &Connection,
    period: Period,
    now: i64,
    optouts: &HashSet<u64>,
    captains: &HashMap<&'static str, Option<u64>>,
    counts: &HashMap<&'static str, i64>,
) -> rusqlite::Result<HouseCup> {
    let (since, until) = period.range(now);
    let (today, _) = Period::Today.range(now);
    let period_sources = points::by_source(conn, since, until)?;
    let today_sources = points::by_source(conn, today, i64::MAX)?;
    let split = |map: &HashMap<(&'static str, Source), i64>, key: &str| {
        let mut out: Vec<(String, i64)> =
            map.iter().filter(|((h, _), _)| *h == key).map(|((_, s), n)| (s.key().to_string(), *n)).collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        out
    };
    let mut main_stmt = conn.prepare(
        "SELECT source, SUM(points) AS n FROM ledger WHERE user_id = ?1 AND house = ?2 AND ts >= ?3 AND ts < ?4
         GROUP BY source ORDER BY n DESC LIMIT 1",
    )?;
    let mut houses = Vec::new();
    for h in HOUSES {
        let total = points::house_total(conn, h.key, since, until)?;
        let mut top = Vec::new();
        for (user, pts) in points::top_members(conn, h.key, since, until)? {
            if optouts.contains(&user) {
                continue;
            }
            let main: String = main_stmt
                .query_row(params![user as i64, h.key, since, until], |r| r.get(0))
                .unwrap_or_else(|_| "mod".to_string());
            top.push(Scorer { user_id: user, points: pts, main_source: main });
            if top.len() == 10 {
                break;
            }
        }
        houses.push(Standing {
            key: h.key.to_string(),
            total,
            captain: captains.get(h.key).copied().flatten(),
            members: counts.get(h.key).copied().unwrap_or(0),
            by_source: split(&period_sources, h.key),
            today_by_source: split(&today_sources, h.key),
            top,
        });
    }
    let mut feed_stmt = conn.prepare(
        "SELECT user_id, house, source, points, reason, ts FROM ledger WHERE points != 0 ORDER BY ts DESC, id DESC LIMIT 50",
    )?;
    let feed = feed_stmt
        .query_map([], |r| {
            Ok(FeedRow {
                user_id: r.get::<_, Option<i64>>(0)?.map(|u| u as u64),
                house: r.get(1)?,
                source: r.get(2)?,
                points: r.get(3)?,
                reason: r.get(4)?,
                ts: r.get(5)?,
            })
        })?
        .flatten()
        .collect();
    Ok(HouseCup { since, until: until.min(i64::MAX / 2), houses, feed })
}

fn person(panel: &Panel, id: u64) -> Value {
    match panel.data.cached_member(id) {
        Some(MemberInfo { name, avatar, .. }) => json!({ "id": id.to_string(), "name": name, "avatar": avatar }),
        None => json!({ "id": id.to_string(), "name": Value::Null, "avatar": Value::Null }),
    }
}

fn split_label(source: &str) -> (String, String) {
    match Source::from_key(source) {
        Some(s) => {
            let label = s.label();
            match label.split_once(' ') {
                Some((icon, name)) => (icon.to_string(), name.to_string()),
                None => (String::new(), label.to_string()),
            }
        }
        None => ("•".to_string(), source.to_string()),
    }
}

#[derive(Deserialize)]
pub struct CupQuery {
    #[serde(default)]
    period: Option<String>,
}

pub async fn get(State(panel): State<Panel>, Query(q): Query<CupQuery>) -> ApiResult {
    let period = match q.period.as_deref().unwrap_or("month") {
        "today" => Period::Today,
        "week" => Period::Week,
        "month" => Period::Month,
        "last_month" => Period::LastMonth,
        "all" => Period::All,
        _ => return Err(ApiError::bad("Period is one of today, week, month, last_month or all.")),
    };
    let now = chrono::Utc::now().timestamp();
    let Some(cup) = panel.data.house_cup(period, now) else {
        return Err(ApiError(axum::http::StatusCode::SERVICE_UNAVAILABLE, "The house points aren't available right now.".into()));
    };
    let leader = cup.houses.iter().map(|h| h.total).max().unwrap_or(0);
    let mut totals: Vec<i64> = cup.houses.iter().map(|h| h.total).collect();
    totals.sort_unstable_by(|a, b| b.cmp(a));
    let houses: Vec<Value> = cup
        .houses
        .iter()
        .map(|s| {
            let meta = house::house(&s.key);
            let sources = |list: &[(String, i64)]| list.iter().map(|(k, n)| json!({ "source": k, "points": n })).collect::<Vec<_>>();
            json!({
                "key": s.key,
                "name": meta.map(|h| h.name).unwrap_or(&s.key),
                "crest": meta.map(|h| h.crest).unwrap_or(""),
                "colour": meta.map(|h| format!("#{:06x}", h.colour)),
                "secondary": meta.map(|h| format!("#{:02x}{:02x}{:02x}", h.colours.1[0], h.colours.1[1], h.colours.1[2])),
                "total": s.total,
                // Ties share a rank.
                "rank": totals.iter().position(|t| *t == s.total).map(|i| i + 1).unwrap_or(1),
                "gap": leader - s.total,
                "members": s.members,
                "captain": s.captain.map(|c| person(&panel, c)),
                "by_source": sources(&s.by_source),
                "today_by_source": sources(&s.today_by_source),
                "top": s.top.iter().map(|t| {
                    let mut p = person(&panel, t.user_id);
                    p["points"] = json!(t.points);
                    p["main_source"] = json!(t.main_source);
                    p
                }).collect::<Vec<_>>(),
            })
        })
        .collect();
    let feed: Vec<Value> = cup
        .feed
        .iter()
        .map(|row| {
            let (icon, label) = split_label(&row.source);
            // Weekly awards can come from #safe-corner: never repeat what they were for.
            let reason = if row.source == Source::Weekly.key() { "weekly posts award".to_string() } else { row.reason.clone() };
            json!({
                "ts": row.ts,
                "member": row.user_id.map(|u| person(&panel, u)),
                "house": row.house,
                "source": row.source,
                "source_icon": icon,
                "source_label": label,
                "points": row.points,
                "reason": reason,
            })
        })
        .collect();
    let sources: Vec<Value> = Source::ALL
        .iter()
        .map(|s| {
            let (icon, label) = split_label(s.key());
            json!({ "key": s.key(), "icon": icon, "label": label })
        })
        .collect();
    ok(json!({
        "period": period,
        "since": cup.since,
        "until": if cup.until >= i64::MAX / 2 { Value::Null } else { json!(cup.until) },
        "now": now,
        "houses": houses,
        "feed": feed,
        "sources": sources,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(points::SCHEMA).unwrap();
        conn
    }

    fn add(conn: &Connection, user: Option<u64>, house: &str, source: &str, pts: i64, reason: &str, ts: i64) {
        conn.execute(
            "INSERT INTO ledger (user_id, house, source, points, reason, day, ts) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![user.map(|u| u as i64), house, source, pts, reason, points::ist_day(ts), ts],
        )
        .unwrap();
    }

    #[test]
    fn periods_line_up_on_india_days() {
        // 2026-09-14 02:00 IST is a Monday; 2026-09-13 20:30 UTC.
        let now = 1_789_331_400;
        let (today, _) = Period::Today.range(now);
        assert_eq!(points::ist_day(today), "2026-09-14");
        assert_eq!(points::ist_day(today - 1), "2026-09-13");
        let (week, _) = Period::Week.range(now);
        assert_eq!(week, today, "Monday starts the week");
        let (since, until) = Period::LastMonth.range(now);
        assert_eq!(points::ist_day(since), "2026-08-01");
        assert_eq!(until, points::month_start(now));
        assert_eq!(Period::All.range(now).0, 0);
    }

    #[test]
    fn standings_skip_muggles_and_split_sources() {
        let conn = ledger();
        let now = 1_789_331_400;
        let earlier = points::month_start(now) - 3600;
        add(&conn, Some(1), "gryffindor", "quiz", 6, "quiz round", now - 100);
        add(&conn, Some(1), "gryffindor", "chat", 1, "", now - 90);
        add(&conn, Some(2), "gryffindor", "arena", 3, "won a fight", now - 80);
        add(&conn, Some(3), "slytherin", "weekly", 3, "a post in safe-corner", now - 70);
        add(&conn, None, "ravenclaw", "mod", 10, "event", now - 60);
        add(&conn, Some(4), "hufflepuff", "snitch", 5, "", earlier);
        let optouts: HashSet<u64> = [2].into_iter().collect();
        let captains = HOUSES.iter().map(|h| (h.key, (h.key == "gryffindor").then_some(1))).collect();
        let counts = HOUSES.iter().map(|h| (h.key, 3)).collect();
        let cup = read(&conn, Period::Month, now, &optouts, &captains, &counts).unwrap();
        let g = cup.houses.iter().find(|h| h.key == "gryffindor").unwrap();
        assert_eq!(g.total, 10);
        assert_eq!(g.top.len(), 1, "a Muggle isn't listed as a scorer");
        assert_eq!(g.top[0].main_source, "quiz");
        assert_eq!(g.by_source[0], ("quiz".to_string(), 6));
        assert_eq!(g.captain, Some(1));
        assert_eq!(cup.houses.iter().find(|h| h.key == "hufflepuff").unwrap().total, 0, "last month's points are out");
        assert_eq!(cup.feed.len(), 6);
        let all = read(&conn, Period::All, now, &optouts, &captains, &counts).unwrap();
        assert_eq!(all.houses.iter().find(|h| h.key == "hufflepuff").unwrap().total, 5);
    }
}
