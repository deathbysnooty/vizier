//! The Invites page: every invite with its creator, and the people each
//! inviter brought in - how many are still here, sorted into a house, and
//! active - plus the line on a member's profile saying how they joined.
//!
//! Everything comes from invite tracking's own `invites.db` (see
//! `discord::invites`), the join log for who is still here, the house store
//! for who was sorted, and the stats store for who has been active. Tracking
//! only knows joins since it began; the page says so, and points to Discord's
//! own Server Settings → Members for anything earlier.
//!
//! Admins only, like every panel page. Looking at the page, and at one
//! inviter's list, goes in the activity log once per fifteen minutes.

use std::collections::{HashMap, HashSet};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use parking_lot::Mutex;
use serde_json::{Value, json};

use super::super::super::house::House;
use super::super::super::invites::{self as inv, Brought, Tally};
use super::super::super::invites_store::{self as store, How, Join, Stored};
use super::{ApiError, ApiResult, Caller, Panel, ok, parse_id, search};

/// How far back "active" looks, in days.
pub const ACTIVE_DAYS: i64 = 30;
/// Joins listed under "Latest joins".
pub const RECENT_JOINS: usize = 25;

fn store_db() -> Result<&'static Mutex<rusqlite::Connection>, ApiError> {
    store::db().ok_or_else(|| ApiError(StatusCode::SERVICE_UNAVAILABLE, "The invite store isn't open. Restart the bot.".into()))
}

fn db_error(err: rusqlite::Error) -> ApiError {
    ApiError::internal(format!("invite store: {}", err))
}

/// A member as the page shows them: the server's name for them when cached,
/// else the one stored at the time.
fn person(panel: &Panel, id: u64, stored: &str) -> Value {
    let cached = panel.data.cached_member(id);
    let name = cached
        .as_ref()
        .map(|m| m.name.clone())
        .filter(|n| !n.trim().is_empty())
        .or_else(|| (!stored.trim().is_empty()).then(|| stored.to_string()))
        .unwrap_or_else(|| format!("Member {}", id));
    json!({ "id": id.to_string(), "name": name, "avatar": cached.map(|m| m.avatar) })
}

fn name_of(panel: &Panel, id: u64, stored: &str) -> String {
    person(panel, id, stored)["name"].as_str().unwrap_or_default().to_string()
}

fn channel_name(panel: &Panel, id: u64, stored: &str) -> String {
    panel.data.channels().into_iter().find(|c| c.id == id.to_string()).map(|c| c.name).unwrap_or_else(|| stored.to_string())
}

// --- who each inviter brought in -----------------------------------------------------------------

/// What the page knows about one member an inviter brought in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Standing {
    pub still_here: bool,
    pub sorted: bool,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InviterRow {
    pub id: u64,
    pub name: String,
    pub brought: usize,
    pub still_here: usize,
    pub sorted: usize,
    pub active: usize,
}

/// Counts each inviter's members, most still here first, then most brought,
/// then by name.
pub fn rank(tallies: &[Tally], standing: &HashMap<u64, Standing>) -> Vec<InviterRow> {
    let mut rows: Vec<InviterRow> = tallies
        .iter()
        .map(|t| {
            let of = |f: fn(&Standing) -> bool| t.members.iter().filter(|m| standing.get(&m.member_id).is_some_and(f)).count();
            InviterRow {
                id: t.inviter_id,
                name: t.inviter_name.clone(),
                brought: t.members.len(),
                still_here: of(|s| s.still_here),
                sorted: of(|s| s.sorted),
                active: of(|s| s.active),
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.still_here.cmp(&a.still_here).then(b.brought.cmp(&a.brought)).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())).then(a.id.cmp(&b.id))
    });
    rows
}

/// Still here (join log, else the member cache), sorted into a house, and
/// still active: here, and chatted or sat in voice over the last 30 days.
async fn standings(panel: &Panel, ids: Vec<u64>, now: i64) -> HashMap<u64, Standing> {
    if ids.is_empty() {
        return HashMap::new();
    }
    let logs: HashMap<u64, super::left::JoinRow> = panel.data.join_logs().await.into_iter().map(|r| (r.id, r)).collect();
    let stats = panel.data.left_stats(ids.clone(), now).await;
    // The house store is read off the async threads, as the members page does.
    let data = panel.data.clone();
    let wanted = ids.clone();
    let houses: HashSet<u64> = tokio::task::spawn_blocking(move || wanted.into_iter().filter(|id| data.member_house(*id).is_some()).collect())
        .await
        .unwrap_or_default();
    ids.into_iter()
        .map(|id| {
            let still_here = match logs.get(&id) {
                Some(r) => inv::still_here(r.leaves, r.last_join.as_deref(), r.last_leave.as_deref()),
                None => panel.data.cached_member(id).is_some(),
            };
            let s = stats.get(&id).cloned().unwrap_or_default();
            (id, Standing { still_here, sorted: houses.contains(&id), active: still_here && (s.messages_30d > 0 || s.voice_30d_secs > 0) })
        })
        .collect()
}

// --- the overview ---------------------------------------------------------------------------------

fn candidate_json(panel: &Panel, c: &store::Candidate) -> Value {
    json!({
        "code": c.code,
        "url": inv::invite_url(&c.code),
        "vanity": c.vanity,
        "inviter": c.inviter_id.map(|id| person(panel, id, &c.inviter_name)),
    })
}

/// One join as the page shows it, with its line.
fn join_json(panel: &Panel, j: &Join, started: i64, now: i64) -> Value {
    let n = &j.new;
    json!({
        "id": j.id,
        "member": person(panel, n.member_id, &n.member_name),
        "joined_ts": n.joined_ts,
        "how": n.how.key(),
        "code": n.code,
        "url": n.code.as_deref().map(inv::invite_url),
        "inviter": n.inviter_id.map(|id| person(panel, id, &n.inviter_name)),
        "candidates": n.candidates.iter().map(|c| candidate_json(panel, c)).collect::<Vec<_>>(),
        "note": n.note,
        "line": inv::line(Some(n), started, None, now, &|id, stored| format!("@{}", name_of(panel, id, stored))),
    })
}

fn invite_json(panel: &Panel, s: &Stored, brought: &HashMap<String, usize>) -> Value {
    let i = &s.invite;
    json!({
        "code": i.code,
        "url": inv::invite_url(&i.code),
        "inviter": i.inviter_id.map(|id| person(panel, id, &i.inviter_name)),
        "channel": { "id": i.channel_id.to_string(), "name": channel_name(panel, i.channel_id, &i.channel_name) },
        "uses": i.uses,
        "max_uses": i.max_uses,
        "created_ts": (i.created_ts > 0).then_some(i.created_ts),
        "expires_ts": i.expires_ts(),
        "temporary": i.temporary,
        "gone_ts": s.gone_ts,
        "brought": brought.get(&i.code).copied().unwrap_or(0),
    })
}

pub async fn overview(State(panel): State<Panel>, axum::Extension(Caller(user)): axum::Extension<Caller>) -> ApiResult {
    search::log_quietly("invites:list", user, "Looked at the invites");
    let now = chrono::Utc::now().timestamp();
    let (invites, joins, started, vanity) = {
        let conn = store_db()?.lock();
        (store::all_invites(&conn).map_err(db_error)?, store::all_joins(&conn).map_err(db_error)?, store::started_ts(&conn), store::vanity(&conn))
    };
    let tallies = inv::tally(&joins);
    let ids: Vec<u64> = tallies.iter().flat_map(|t| t.members.iter().map(|m| m.member_id)).collect::<HashSet<_>>().into_iter().collect();
    let standing = standings(&panel, ids, now).await;
    let live_by: HashMap<u64, usize> = invites.iter().filter(|s| s.gone_ts.is_none()).filter_map(|s| s.invite.inviter_id).fold(HashMap::new(), |mut m, id| {
        *m.entry(id).or_default() += 1;
        m
    });
    let inviters: Vec<Value> = rank(&tallies, &standing)
        .into_iter()
        .map(|r| {
            let mut who = person(&panel, r.id, &r.name);
            who["brought"] = json!(r.brought);
            who["still_here"] = json!(r.still_here);
            who["sorted"] = json!(r.sorted);
            who["active"] = json!(r.active);
            who["live_invites"] = json!(live_by.get(&r.id).copied().unwrap_or(0));
            who
        })
        .collect();
    let mut brought: HashMap<String, usize> = HashMap::new();
    for j in joins.iter().filter(|j| j.new.how.credits_inviter()) {
        if let Some(code) = &j.new.code {
            *brought.entry(code.clone()).or_default() += 1;
        }
    }
    let mut totals: HashMap<&'static str, usize> = HashMap::new();
    for j in &joins {
        *totals.entry(j.new.how.key()).or_default() += 1;
    }
    let total = |h: How| totals.get(h.key()).copied().unwrap_or(0);
    ok(json!({
        "enabled": inv::enabled(),
        "started_ts": started,
        "vanity": vanity.map(|v| json!({ "code": v.code, "url": inv::invite_url(&v.code), "uses": v.uses })),
        "totals": {
            "joins": joins.len(),
            "sure": total(How::Sure),
            "likely": total(How::Likely),
            "vanity": total(How::Vanity),
            "unsure": total(How::Unsure),
            "unknown": total(How::Unknown),
        },
        "invites": invites.iter().map(|s| invite_json(&panel, s, &brought)).collect::<Vec<_>>(),
        "inviters": inviters,
        "recent": joins.iter().take(RECENT_JOINS).map(|j| join_json(&panel, j, started, now)).collect::<Vec<_>>(),
        "active_days": ACTIVE_DAYS,
    }))
}

// --- one inviter ------------------------------------------------------------------------------------

fn house_json(h: &House) -> Value {
    json!({ "key": h.key, "name": h.name, "crest": h.crest, "colour": format!("#{:06x}", h.colour) })
}

pub async fn inviter(State(panel): State<Panel>, axum::Extension(Caller(user)): axum::Extension<Caller>, Path(id): Path<String>) -> ApiResult {
    let id = parse_id(&id).ok_or_else(|| ApiError::bad("That isn't a member id."))?;
    let now = chrono::Utc::now().timestamp();
    let (joins, started, invites) = {
        let conn = store_db()?.lock();
        let invites: Vec<Stored> = store::all_invites(&conn).map_err(db_error)?.into_iter().filter(|s| s.invite.inviter_id == Some(id)).collect();
        (store::all_joins(&conn).map_err(db_error)?, store::started_ts(&conn), invites)
    };
    let tally = inv::tally(&joins).into_iter().find(|t| t.inviter_id == id);
    let stored_name = tally.as_ref().map(|t| t.inviter_name.clone()).or_else(|| invites.first().map(|s| s.invite.inviter_name.clone())).unwrap_or_default();
    if tally.is_none() && invites.is_empty() && panel.data.cached_member(id).is_none() {
        return Err(ApiError::not_found("No invites or joins for that member."));
    }
    let who = person(&panel, id, &stored_name);
    search::log_quietly("invites:inviter", user, &format!("@{}", who["name"].as_str().unwrap_or_default()));

    let members: Vec<Brought> = tally.map(|t| t.members).unwrap_or_default();
    let ids: Vec<u64> = members.iter().map(|m| m.member_id).collect();
    let standing = standings(&panel, ids.clone(), now).await;
    let data = panel.data.clone();
    let houses: HashMap<u64, &'static House> =
        tokio::task::spawn_blocking(move || ids.into_iter().filter_map(|id| data.member_house(id).map(|h| (id, h))).collect()).await.unwrap_or_default();
    let brought: HashMap<String, usize> = members.iter().fold(HashMap::new(), |mut m, b| {
        *m.entry(b.code.clone()).or_default() += 1;
        m
    });
    let rows: Vec<Value> = members
        .iter()
        .map(|b| {
            let s = standing.get(&b.member_id).cloned().unwrap_or_default();
            let mut m = person(&panel, b.member_id, &b.member_name);
            m["joined_ts"] = json!(b.joined_ts);
            m["code"] = json!(b.code);
            m["url"] = json!(inv::invite_url(&b.code));
            m["how"] = json!(b.how.key());
            m["still_here"] = json!(s.still_here);
            m["active"] = json!(s.active);
            m["house"] = houses.get(&b.member_id).map(|h| house_json(h)).unwrap_or(Value::Null);
            m
        })
        .collect();
    let count = |f: fn(&Value) -> bool| rows.iter().filter(|r| f(r)).count();
    ok(json!({
        "inviter": who,
        "started_ts": started,
        "brought": rows.len(),
        "still_here": count(|r| r["still_here"] == true),
        "sorted": count(|r| !r["house"].is_null()),
        "active": count(|r| r["active"] == true),
        "members": rows,
        "invites": invites.iter().map(|s| invite_json(&panel, s, &brought)).collect::<Vec<_>>(),
        "active_days": ACTIVE_DAYS,
    }))
}

// --- the line on a member's profile -----------------------------------------------------------------

pub async fn member(State(panel): State<Panel>, Path(id): Path<String>) -> ApiResult {
    let id = parse_id(&id).ok_or_else(|| ApiError::bad("That isn't a member id."))?;
    let now = chrono::Utc::now().timestamp();
    let (joins, started) = {
        let conn = store_db()?.lock();
        (store::joins_of(&conn, id).map_err(db_error)?, store::started_ts(&conn))
    };
    let joined_at = match joins.first() {
        Some(_) => None,
        None => panel.data.member_detail(id).await.and_then(|d| d.joined_at),
    };
    let latest = joins.first();
    let line = inv::line(latest.map(|j| &j.new), started, joined_at, now, &|who, stored| format!("@{}", name_of(&panel, who, stored)));
    ok(json!({
        "line": line,
        "started_ts": started,
        "tracked": latest.is_some(),
        "before_tracking": latest.is_none() && joined_at.is_none_or(|at| at < started),
        "joined_at": joined_at,
        "join": latest.map(|j| join_json(&panel, j, started, now)),
        "joins": joins.len(),
    }))
}

// --- the activity log --------------------------------------------------------------------------------

pub fn audit_entry(e: &super::super::AuditEntry) -> serde_json::Map<String, Value> {
    let mut obj = serde_json::Map::new();
    let label = match e.key.as_str() {
        "invites:inviter" => "Looked at who someone invited",
        _ => "Looked at the invites",
    };
    obj.insert("label".into(), json!(label));
    obj.insert("section".into(), json!({ "id": "invites", "title": "Invites", "icon": "✉️" }));
    obj.insert("change".into(), json!(e.new.clone().unwrap_or_else(|| label.to_string())));
    obj.insert("old".into(), Value::Null);
    obj.insert("new".into(), Value::Null);
    obj
}

// --- demo and test data -----------------------------------------------------------------------------

/// Three weeks of tracked joins for the fake server: every kind of answer, a
/// few inviters, and two members who have gone again.
#[cfg(test)]
pub fn seed(conn: &rusqlite::Connection, now: i64) {
    use store::{Candidate, Invite, NewJoin, Vanity};
    const DAY: i64 = 86_400;
    if store::meta_get(conn, "demo_seeded").ok().flatten().is_some() {
        return;
    }
    store::meta_set(conn, "started_ts", &(now - 21 * DAY).to_string()).unwrap();
    let invite = |code: &str, by: u64, by_name: &str, channel: u64, uses: u64, max_uses: u64, max_age: u64, created: i64| Invite {
        code: code.into(),
        inviter_id: Some(by),
        inviter_name: by_name.into(),
        channel_id: channel,
        channel_name: String::new(),
        uses,
        max_uses,
        max_age,
        temporary: false,
        created_ts: created,
    };
    let live = [
        invite("mlciFam", 1001, "Kabir", 21, 14, 0, 0, now - 60 * DAY),
        invite("chai4u", 2001, "Diya", 21, 6, 0, 7 * DAY as u64, now - 2 * DAY),
        invite("k7Rz2pQ", 2000, "Aarav", 23, 1, 10, 0, now - 9 * DAY),
        invite("vihaanX", 2004, "Vihaan", 22, 3, 0, DAY as u64, now - 3 * 3600),
    ];
    for i in &live {
        store::upsert(conn, i, now - 3600).unwrap();
    }
    for (i, gone) in [(invite("oneShot", 1002, "Meera", 21, 0, 1, 0, now - 5 * DAY), now - 3 * DAY), (invite("oldLnk", 2004, "Vihaan", 22, 2, 0, DAY as u64, now - 11 * DAY), now - 10 * DAY)] {
        store::upsert(conn, &i, gone - 60).unwrap();
        store::mark_gone(conn, &i.code, gone).unwrap();
    }
    store::claim(conn, "oneShot").unwrap();
    store::set_vanity(conn, &Vanity { code: "mlci".into(), uses: 52 }).unwrap();

    let roster = |id: u64| -> &'static str {
        match id {
            2005 => "Anaya",
            2006 => "Rohan",
            2007 => "Zoya",
            2008 => "Arjun",
            2009 => "Ishita",
            2010 => "Dev",
            2011 => "Tanvi",
            2012 => "Sameer",
            2013 => "Riya",
            2014 => "Yash",
            2015 => "Nisha",
            2016 => "Aditya",
            2017 => "Pooja",
            2018 => "Karan",
            2020 => "Nikhil",
            2021 => "Aisha",
            3001 => "ofcadeath",
            3003 => "notyourbhai",
            _ => "someone",
        }
    };
    let cand = |code: &str, by: u64, name: &str| Candidate { code: code.into(), inviter_id: Some(by), inviter_name: name.into(), vanity: false };
    // (member, days ago, how, code, inviter, inviter name)
    let joins: &[(u64, f64, How, Option<&str>, Option<u64>, &str)] = &[
        (3003, 11.0, How::Sure, Some("mlciFam"), Some(1001), "Kabir"),
        (2005, 19.5, How::Sure, Some("mlciFam"), Some(1001), "Kabir"),
        (2006, 17.0, How::Sure, Some("mlciFam"), Some(1001), "Kabir"),
        (2007, 12.2, How::Sure, Some("mlciFam"), Some(1001), "Kabir"),
        (2008, 4.1, How::Sure, Some("mlciFam"), Some(1001), "Kabir"),
        (2009, 1.6, How::Sure, Some("chai4u"), Some(2001), "Diya"),
        (2010, 1.2, How::Sure, Some("chai4u"), Some(2001), "Diya"),
        (2011, 0.9, How::Sure, Some("chai4u"), Some(2001), "Diya"),
        (3001, 0.2, How::Sure, Some("chai4u"), Some(2001), "Diya"),
        (2012, 8.0, How::Sure, Some("k7Rz2pQ"), Some(2000), "Aarav"),
        (2013, 10.5, How::Sure, Some("oldLnk"), Some(2004), "Vihaan"),
        (2014, 0.1, How::Sure, Some("vihaanX"), Some(2004), "Vihaan"),
        (2015, 3.0, How::Likely, Some("oneShot"), Some(1002), "Meera"),
        (2016, 14.0, How::Vanity, Some("mlci"), None, ""),
        (2017, 6.3, How::Vanity, Some("mlci"), None, ""),
        (2018, 2.0, How::Unsure, None, None, ""),
        (2020, 2.0, How::Unsure, None, None, ""),
        (2021, 5.5, How::Unknown, None, None, ""),
    ];
    for (member, days, how, code, by, by_name) in joins {
        let unsure = *how == How::Unsure;
        store::add_join(
            conn,
            &NewJoin {
                member_id: *member,
                member_name: roster(*member).into(),
                joined_ts: now - (days * DAY as f64) as i64,
                how: *how,
                code: code.map(String::from),
                inviter_id: *by,
                inviter_name: by_name.to_string(),
                candidates: if unsure { vec![cand("mlciFam", 1001, "Kabir"), cand("chai4u", 2001, "Diya")] } else { vec![] },
                note: String::new(),
            },
        )
        .unwrap();
    }
    store::meta_set(conn, "demo_seeded", "1").unwrap();
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    use super::super::tests::{ADMIN, ADMIN_TWO, MEMBER, panel, session_for};
    use super::*;

    async fn call(app: &Router, path: &str, session: Option<&str>) -> (StatusCode, Value) {
        let mut req = Request::builder().method("GET").uri(path).header("x-panel", "1");
        if let Some(s) = session {
            req = req.header("cookie", format!("mlci_panel={}", s));
        }
        let res = app.clone().oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 10 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn inviters_are_counted_and_sorted_by_who_is_still_here() {
        let app = panel();
        let (status, page) = call(&app, "/api/invites", Some(&session_for(ADMIN))).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        let inviters = page["inviters"].as_array().unwrap();
        let names: Vec<&str> = inviters.iter().map(|i| i["name"].as_str().unwrap()).collect();
        // Kabir brought five, one gone; Diya four, one gone; Vihaan two; then one each by name.
        assert_eq!(names, vec!["Kabir", "Diya", "Vihaan", "Aarav", "Meera"], "{page}");
        assert_eq!((inviters[0]["brought"].as_u64(), inviters[0]["still_here"].as_u64()), (Some(5), Some(4)));
        assert_eq!((inviters[1]["brought"].as_u64(), inviters[1]["still_here"].as_u64()), (Some(4), Some(3)));
        // Every roster member is sorted in the fake server; everyone counted active is still counted once.
        for i in inviters {
            assert!(i["sorted"].as_u64() <= i["brought"].as_u64() && i["active"].as_u64() <= i["brought"].as_u64(), "{i}");
        }
        assert_eq!(inviters[0]["live_invites"], 1);
        // Unsure, vanity and unknown joins are nobody's.
        assert!(!names.contains(&""));
        assert_eq!(page["totals"]["joins"], 18);
        assert_eq!((page["totals"]["unsure"].as_u64(), page["totals"]["vanity"].as_u64(), page["totals"]["unknown"].as_u64()), (Some(2), Some(2), Some(1)));
        // Live invites first, the gone ones after; each with its creator and channel.
        let invites = page["invites"].as_array().unwrap();
        let first_gone = invites.iter().position(|i| !i["gone_ts"].is_null()).unwrap();
        assert!(invites[first_gone..].iter().all(|i| !i["gone_ts"].is_null()));
        let fam = invites.iter().find(|i| i["code"] == "mlciFam").unwrap();
        assert_eq!((fam["inviter"]["name"].as_str(), fam["channel"]["name"].as_str(), fam["uses"].as_u64(), fam["brought"].as_u64()), (Some("Kabir"), Some("general"), Some(14), Some(5)));
        assert!(invites.iter().find(|i| i["code"] == "chai4u").unwrap()["expires_ts"].is_i64());
        assert_eq!(page["vanity"]["url"], "discord.gg/mlci");
    }

    #[test]
    fn rank_counts_still_here_sorted_and_active() {
        let b = |id: u64| Brought { member_id: id, member_name: String::new(), joined_ts: 0, code: "x".into(), how: How::Sure };
        let tallies = vec![
            Tally { inviter_id: 1, inviter_name: "Busy".into(), members: vec![b(10), b(11), b(12)] },
            Tally { inviter_id: 2, inviter_name: "Kept".into(), members: vec![b(20), b(21)] },
        ];
        let s = |still_here, sorted, active| Standing { still_here, sorted, active };
        let standing: HashMap<u64, Standing> =
            [(10, s(false, true, false)), (11, s(false, true, false)), (12, s(true, true, true)), (20, s(true, false, true)), (21, s(true, true, false))].into_iter().collect();
        let rows = rank(&tallies, &standing);
        // Two still here beats three brought with one left.
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![2, 1]);
        assert_eq!((rows[0].brought, rows[0].still_here, rows[0].sorted, rows[0].active), (2, 2, 1, 1));
        assert_eq!((rows[1].brought, rows[1].still_here, rows[1].sorted, rows[1].active), (3, 1, 3, 1));
    }

    #[tokio::test]
    async fn one_inviter_lists_the_members_they_brought() {
        let app = panel();
        let (status, page) = call(&app, "/api/invites/inviters/1001", Some(&session_for(ADMIN))).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        let members = page["members"].as_array().unwrap();
        assert_eq!(members.len(), 5);
        assert!(members.windows(2).all(|w| w[0]["joined_ts"].as_i64() >= w[1]["joined_ts"].as_i64()), "newest first");
        let gone = members.iter().find(|m| m["id"] == "3003").unwrap();
        assert_eq!(gone["still_here"], false);
        assert_eq!(gone["active"], false, "someone who left is not counted active");
        assert_eq!(page["still_here"], 4);
        assert!(members.iter().all(|m| m["url"] == "discord.gg/mlciFam"));
    }

    #[tokio::test]
    async fn the_profile_line_says_how_they_joined() {
        let app = panel();
        let session = session_for(ADMIN);
        let (_, sure) = call(&app, "/api/invites/members/2008", Some(&session)).await;
        let line = sure["line"].as_str().unwrap();
        assert!(line.starts_with("Joined ") && line.ends_with(" via discord.gg/mlciFam — invite created by @Kabir"), "{line}");
        let (_, likely) = call(&app, "/api/invites/members/2015", Some(&session)).await;
        assert!(likely["line"].as_str().unwrap().ends_with("invite created by @Meera (likely)"), "{likely}");
        let (_, vanity) = call(&app, "/api/invites/members/2016", Some(&session)).await;
        assert!(vanity["line"].as_str().unwrap().ends_with("via the server's vanity link"), "{vanity}");
        let (_, unsure) = call(&app, "/api/invites/members/2018", Some(&session)).await;
        assert!(unsure["line"].as_str().unwrap().ends_with("(unsure)"), "{unsure}");
        assert_eq!(unsure["join"]["candidates"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_member_from_before_tracking_is_not_known() {
        let app = panel();
        // Zoya (1004) joined in 2023 in the fake server, long before tracking began.
        let (status, page) = call(&app, "/api/invites/members/1004", Some(&session_for(ADMIN))).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert_eq!((page["tracked"].as_bool(), page["before_tracking"].as_bool()), (Some(false), Some(true)));
        let line = page["line"].as_str().unwrap();
        assert!(line.contains("before invite tracking began") && line.contains("not known"), "{line}");
    }

    #[tokio::test]
    async fn the_invites_page_is_for_admins_only() {
        let app = panel();
        let member = session_for(MEMBER);
        for path in ["/api/invites", "/api/invites/inviters/1001", "/api/invites/members/2008"] {
            let (status, _) = call(&app, path, None).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
            let (status, _) = call(&app, path, Some(&member)).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
        }
    }

    #[tokio::test]
    async fn looking_is_in_the_activity_log_once_per_quarter_hour() {
        let app = panel();
        let session = session_for(ADMIN_TWO);
        for _ in 0..3 {
            let (status, _) = call(&app, "/api/invites/inviters/2001", Some(&session)).await;
            assert_eq!(status, StatusCode::OK);
        }
        let (_, audit) = call(&app, "/api/audit?limit=1000", Some(&session)).await;
        let entries: Vec<&Value> = audit.as_array().unwrap().iter().filter(|e| e["key"] == "invites:inviter" && e["user_id"] == ADMIN_TWO.to_string()).collect();
        assert_eq!(entries.len(), 1, "{audit}");
        assert_eq!(entries[0]["change"], "@Diya");
        assert_eq!(entries[0]["label"], "Looked at who someone invited");
        assert_eq!(entries[0]["section"]["id"], "invites");
    }
}
