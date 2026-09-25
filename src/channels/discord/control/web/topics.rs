//! The Topics pages: what one member has been talking about lately, and what
//! the whole server has been talking about this week.
//!
//! Both read the nightly pass's store and nothing else. No model is ever asked
//! anything here — the asking happened at five in the morning, once for the
//! whole day — so these pages are as cheap as any other list in the panel.
//!
//! The thing both pages are careful about is the difference between **nothing
//! much** and **not read yet**. A member with no row for a day said nothing
//! worth recording that day; a day the pass never ran on has no rows for
//! anybody. Those look identical in the table and must not look identical on
//! the page, so every day comes back with which of the two it is.
//!
//! Admins only, like every panel page, and a look is one line in the activity
//! log — naming who looked at whom, never what was read.

use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::super::topics::{self, DayEntry};
use super::super::super::topics_store::{self as store, KIND_TOPICS};
use super::msglog::member_json;
use super::{ApiError, ApiResult, Caller, ChannelInfo, Panel, ok, parse_id, search};

/// Days one member's history shows.
pub const MEMBER_DAYS: i64 = 28;
/// Days the server-wide view covers by default, and at most.
pub const WEEK_DAYS: i64 = 7;
pub const MAX_DAYS: i64 = 60;
/// Entries one server-wide view reads at most.
pub const MAX_ENTRIES: usize = 20_000;
/// Topics the server-wide view lists.
pub const TOP_TOPICS: usize = 40;
/// Days each half of the drift is worked out over.
pub const DRIFT_DAYS: i64 = 7;

const DAY: i64 = 86_400;

fn store_db() -> Result<&'static parking_lot::Mutex<rusqlite::Connection>, ApiError> {
    store::db().ok_or_else(|| ApiError(StatusCode::SERVICE_UNAVAILABLE, "The topics store isn't open. Restart the bot.".into()))
}

fn db_error(err: rusqlite::Error) -> ApiError {
    ApiError::internal(format!("topics store: {}", err))
}

/// The India days of a window, newest first.
fn days_of(now: i64, back: i64) -> Vec<String> {
    (0..back).map(|n| super::super::super::points::ist_day(now - (n + 1) * DAY)).collect()
}

/// Which nights the pass actually finished, out of the ones asked about.
fn nights_done(conn: &rusqlite::Connection, days: &[String]) -> HashMap<String, bool> {
    store::runs(conn, KIND_TOPICS, (days.len() + 8).min(400))
        .unwrap_or_default()
        .into_iter()
        .map(|r| (r.day.clone(), !r.unfinished()))
        .collect()
}

/// One day of one member's history, in the shape the page draws. A day with no
/// entry is not a gap: it is either a day they said nothing much on, or a day
/// nobody has read yet, and the page says which.
fn day_json(day: &str, entry: Option<&DayEntry>, done: Option<bool>, channels: &[super::ChannelInfo]) -> Value {
    let state = match (entry, done) {
        (Some(_), _) => "topics",
        (None, Some(true)) => "nothing much",
        _ => "not read yet",
    };
    json!({
        "day": day,
        "state": state,
        "topics": entry.map(|e| e.topics.clone()).unwrap_or_default(),
        "line": entry.map(|e| e.line.clone()).unwrap_or_default(),
        "messages": entry.map(|e| e.messages).unwrap_or(0),
        "channels": entry
            .map(|e| {
                e.channels
                    .iter()
                    .map(|c| json!({ "id": c.to_string(), "name": channels.iter().find(|x| x.id == c.to_string()).map(|x| x.name.clone()) }))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    })
}

// --- one member ----------------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct MemberQuery {
    #[serde(default)]
    member: Option<String>,
    #[serde(default)]
    days: Option<String>,
}

/// One member's topics over the last few weeks, and how they have moved.
pub async fn member(State(panel): State<Panel>, axum::Extension(Caller(user)): axum::Extension<Caller>, Query(q): Query<MemberQuery>) -> ApiResult {
    let raw = q.member.as_deref().map(str::trim).filter(|m| !m.is_empty()).ok_or_else(|| ApiError::bad("Pick a member."))?;
    let id = parse_id(raw).ok_or_else(|| ApiError::bad("That isn't a member id."))?;
    let back = q.days.as_deref().and_then(|d| d.parse::<i64>().ok()).unwrap_or(MEMBER_DAYS).clamp(1, topics::HISTORY_DAYS);
    let now = chrono::Utc::now().timestamp();
    let days = days_of(now, back);
    let since = days.last().cloned().unwrap_or_default();

    let (entries, done) = {
        let conn = store_db()?.lock();
        (store::for_member(&conn, id, &since, back as usize).map_err(db_error)?, nights_done(&conn, &days))
    };
    let by_day: HashMap<&str, &DayEntry> = entries.iter().map(|e| (e.day.as_str(), e)).collect();

    // What is new, what they have kept up, and what they have dropped: the two
    // halves of the window against each other.
    let split = super::super::super::points::ist_day(now - DRIFT_DAYS * DAY);
    let (recent, before): (Vec<DayEntry>, Vec<DayEntry>) = entries.iter().cloned().partition(|e| e.day > split);
    let drift = topics::drift(&recent, &before);
    let lately = topics::roll_up(&recent, 12);

    let channels = panel.data.channels();
    search::log_quietly("topics:member", user, &format!("@{}", super::members::name_or_id(&panel, id).await));
    ok(json!({
        "member": member_json(&panel, id, "", ""),
        "days": days.iter().map(|d| day_json(d, by_day.get(d.as_str()).copied(), done.get(d).copied(), &channels)).collect::<Vec<_>>(),
        // The tags of the last week, commonest first: "what they have been on about".
        "lately": lately.iter().map(|t| json!({ "topic": t.topic, "days": t.days })).collect::<Vec<_>>(),
        "drift": drift,
        "drift_days": DRIFT_DAYS,
        "recorded": entries.len(),
        "window": back,
        "on": topics::topics_on(),
    }))
}

// --- the whole server ------------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct WeekQuery {
    #[serde(default)]
    days: Option<String>,
}

/// What the server has been talking about, commonest first, with who is in each.
pub async fn week(State(panel): State<Panel>, axum::Extension(Caller(user)): axum::Extension<Caller>, Query(q): Query<WeekQuery>) -> ApiResult {
    let back = q.days.as_deref().and_then(|d| d.parse::<i64>().ok()).unwrap_or(WEEK_DAYS).clamp(1, MAX_DAYS);
    let now = chrono::Utc::now().timestamp();
    let days = days_of(now, back);
    let (from, to) = (days.last().cloned().unwrap_or_default(), days.first().cloned().unwrap_or_default());

    let (entries, nights) = {
        let conn = store_db()?.lock();
        (store::between(&conn, &from, &to, MAX_ENTRIES).map_err(db_error)?, store::runs(&conn, KIND_TOPICS, back as usize + 4).map_err(db_error)?)
    };
    let top = topics::roll_up(&entries, TOP_TOPICS);
    let mut people: Vec<u64> = entries.iter().map(|e| e.user_id).collect();
    people.sort_unstable();
    people.dedup();

    search::log_quietly("topics:week", user, &format!("The last {} days", back));
    ok(json!({
        "from": from,
        "to": to,
        "days": back,
        "members": people.len(),
        "entries": entries.len(),
        "topics": top.iter().map(|t| json!({
            "topic": t.topic,
            "days": t.days,
            "members": t.people.len(),
            "people": t.people.iter().take(20).map(|(id, n)| {
                let mut v = member_json(&panel, *id, "", "");
                v["days"] = json!(n);
                v
            }).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        // Which nights were read and which were not, so an empty week reads as
        // "the pass hasn't run" rather than "nobody said anything".
        "nights": days.iter().map(|d| {
            let run = nights.iter().find(|r| r.day == *d);
            json!({
                "day": d,
                "state": match run {
                    Some(r) if !r.unfinished() => "read",
                    Some(_) => "started",
                    None => "not read yet",
                },
                "members": run.map(|r| r.members).unwrap_or(0),
                "failed": run.map(|r| r.failed).unwrap_or(0),
                "note": run.map(|r| r.note.clone()).unwrap_or_default(),
                // What the night did not read. A night that ran into its cap or
                // lost a channel looks fine by its counts alone, so it has to
                // say so here or nobody will ever know a channel went missing.
                "capped": run.map(|r| r.capped).unwrap_or(0),
                "dropped": run
                    .map(|r| r.dropped.iter().map(|(name, lost)| json!({ "channel": name, "messages": lost })).collect::<Vec<_>>())
                    .unwrap_or_default(),
                "lost": run.map(|r| r.lost.clone()).unwrap_or_default(),
                "tokens": run.map(|r| r.input_tokens + r.output_tokens).unwrap_or(0),
                "model": run.map(|r| r.model.clone()).unwrap_or_default(),
            })
        }).collect::<Vec<_>>(),
        "on": topics::topics_on(),
    }))
}

/// How the Topics pages read in the activity log.
pub fn audit_entry(e: &super::super::AuditEntry) -> serde_json::Map<String, Value> {
    let mut obj = serde_json::Map::new();
    let one = e.key == "topics:member";
    obj.insert("label".into(), json!(if one { "Looked at a member's topics" } else { "Looked at the server's topics" }));
    obj.insert("section".into(), json!({ "id": "topics", "title": "Topics", "icon": "🏷️" }));
    obj.insert("change".into(), json!(e.new.clone().unwrap_or_else(|| "Looked".into())));
    obj.insert("old".into(), Value::Null);
    obj.insert("new".into(), Value::Null);
    obj
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(user: u64, day: &str, topics: &[&str]) -> DayEntry {
        DayEntry {
            user_id: user,
            day: day.into(),
            topics: topics.iter().map(|t| t.to_string()).collect(),
            line: "On about cricket.".into(),
            messages: 40,
            channels: vec![5],
        }
    }

    /// A day with nothing on it and a day nobody has read look different.
    #[test]
    fn nothing_much_is_not_the_same_as_not_read_yet() {
        let e = entry(11, "2026-09-21", &["cricket"]);
        let with = day_json("2026-09-21", Some(&e), Some(true), &[]);
        assert_eq!(with["state"], "topics");
        assert_eq!(with["topics"], json!(["cricket"]));
        assert_eq!(with["messages"], 40);

        // The pass ran that night and they simply had nothing worth recording.
        let quiet = day_json("2026-09-20", None, Some(true), &[]);
        assert_eq!(quiet["state"], "nothing much");
        assert_eq!(quiet["topics"], json!([]));
        assert_eq!(quiet["line"], "");

        // Nobody has read that night at all: a gap, and it says so.
        assert_eq!(day_json("2026-09-19", None, None, &[])["state"], "not read yet");
        assert_eq!(day_json("2026-09-18", None, Some(false), &[])["state"], "not read yet", "a night that started and never finished is not a read one");
    }

    /// A look is logged, and the log never holds what was read.
    #[test]
    fn the_activity_log_tells_one_member_from_the_whole_server() {
        let entry = |key: &str, what: &str| super::super::super::AuditEntry {
            ts: 0,
            user_id: "1".into(),
            key: key.into(),
            new: Some(what.into()),
            old: None,
        };
        let one = audit_entry(&entry("topics:member", "@notyourbhai"));
        assert_eq!(one["label"], json!("Looked at a member's topics"));
        assert_eq!(one["change"], json!("@notyourbhai"));
        assert_eq!(one["section"]["id"], json!("topics"));
        assert!(one["new"].is_null() && one["old"].is_null(), "a topic never goes in the log");
        assert_eq!(audit_entry(&entry("topics:week", "The last 7 days"))["label"], json!("Looked at the server's topics"));
    }
}
