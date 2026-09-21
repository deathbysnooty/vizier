//! The Kalesh page: looking back at a fight between two or more members.
//!
//! Three ways in. The fights the live detector called, newest first, so a mod
//! can open one without knowing who fought. A search for two to six named
//! members over a period (the last day by default, a week at most), which lists
//! the stretches where any of them were going at each other. And past summaries.
//!
//! The main thing a mod asks for is the summary of the WHOLE period: every
//! stretch of the chosen members together, in time order, deleted messages and
//! messages AutoMod blocked included, capped and trimmed the same honest way a
//! single stretch is. One stretch on its own can still be opened and summarised.
//!
//! A stretch or a period always shows the raw exchange, in order, bystanders
//! included and marked, deleted and blocked messages marked, with a jump link on
//! every message still in Discord. The summary is written only when a mod
//! presses Summarise, by the bot's main model unless another is set; it is
//! kept, so the same messages are never paid for twice, and every summary (and
//! every look) is in the activity log, once per fifteen minutes for the same one.
//! A model call that fails is tried again twice; if it still fails the page is
//! told why in words, never handed an empty result.
//!
//! Admins only, like every panel page. Everything is read from `msglog`'s copy
//! of every channel except #safe-corner and the skip list.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::super::kalesh::{self as k, Line, Scope, Side};
use super::super::super::kalesh_store::{self as store, SCOPE_PERIOD, SCOPE_STRETCH, Summary};
use super::super::super::msglog::{self, Place, SaidRow};
use super::super::super::weekly;
use super::msglog::{channel_json, member_json, never_shown, unavailable};
use super::{ApiError, ApiResult, Caller, Panel, ok, parse_id, search};

/// Detections and summaries the overview lists.
pub const LIST_LIMIT: usize = 50;
/// Stretches one search lists at most.
pub const MAX_STRETCHES: usize = 60;
/// Tries at the model for one summary, and the waits between them.
pub const TRIES: usize = 3;
#[cfg(not(test))]
const RETRY_WAITS: [Duration; 2] = [Duration::from_secs(3), Duration::from_secs(8)];
#[cfg(test)]
const RETRY_WAITS: [Duration; 2] = [Duration::from_millis(5), Duration::from_millis(5)];
const HOUR_MS: i64 = 3_600_000;

fn store_db() -> Result<&'static Mutex<rusqlite::Connection>, ApiError> {
    store::db().ok_or_else(|| ApiError(StatusCode::SERVICE_UNAVAILABLE, "The kalesh store isn't open. Restart the bot.".into()))
}

fn db_error(err: rusqlite::Error) -> ApiError {
    ApiError::internal(format!("kalesh store: {}", err))
}

fn blank(v: &Option<String>) -> Option<String> {
    v.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(String::from)
}

/// The members asked about, in the order they were chosen: `people` (ids split
/// by commas or spaces) and the older `a` and `b`. Two to six different people.
pub fn people_of(people: &Option<String>, a: &Option<String>, b: &Option<String>) -> Result<Vec<u64>, ApiError> {
    let mut raw: Vec<String> = blank(people).map(|p| p.split([',', ' ', '+']).filter(|x| !x.is_empty()).map(String::from).collect()).unwrap_or_default();
    raw.extend(blank(a));
    raw.extend(blank(b));
    let mut out: Vec<u64> = Vec::new();
    let mut repeated = false;
    for r in &raw {
        let id = parse_id(r).ok_or_else(|| ApiError::bad("Pick members from the list."))?;
        if out.contains(&id) {
            repeated = true;
        } else {
            out.push(id);
        }
    }
    if out.len() < 2 {
        return Err(ApiError::bad(if repeated { "Pick two different members." } else { "Pick at least two members." }));
    }
    if out.len() > k::MAX_PEOPLE {
        return Err(ApiError::bad(format!("Pick at most {} members.", k::MAX_PEOPLE)));
    }
    Ok(out)
}

/// Where a message was posted, as the page shows it, unless it is somewhere never shown.
fn place_json(panel: &Panel, row: &SaidRow, sensitive: &[u64]) -> Option<Value> {
    let place = Place { channel_id: row.channel_id, parent_id: row.parent_id, channel_name: row.channel_name.clone() };
    channel_json(&place, &panel.data.channels(), sensitive)
}

/// A channel by id alone (a detection, a past summary), unless it is never shown.
fn channel_by_id(panel: &Panel, channel: u64, sensitive: &[u64]) -> Option<Value> {
    let listed = panel.data.channels();
    let parent = if listed.iter().any(|c| c.id == channel.to_string()) { None } else { panel.data.thread_parent(channel) };
    let name = listed.iter().find(|c| c.id == channel.to_string()).map(|c| c.name.clone()).unwrap_or_default();
    let place = Place { channel_id: channel, parent_id: parent, channel_name: if name.is_empty() { format!("channel {}", channel) } else { name } };
    channel_json(&place, &listed, sensitive)
}

async fn name_of(panel: &Panel, id: u64, rows: &[SaidRow]) -> String {
    if let Some(m) = panel.data.cached_member(id) {
        return m.name;
    }
    if let Some(r) = rows.iter().rev().find(|r| r.author_id == id && !r.blocked()) {
        return r.author_name.clone();
    }
    panel.data.member(id).await.map(|m| m.name).unwrap_or_else(|| id.to_string())
}

async fn names_of(panel: &Panel, people: &[u64], rows: &[SaidRow]) -> Vec<String> {
    let mut out = Vec::with_capacity(people.len());
    for p in people {
        out.push(name_of(panel, *p, rows).await);
    }
    out
}

fn letter(i: usize) -> String {
    Side::Person(i).letter().map(|c| c.to_ascii_lowercase().to_string()).unwrap_or_default()
}

/// The chosen members as the page shows them: name, letter and picture (from the
/// member cache, so the page never asks Discord per message; none falls back to initials).
fn people_json(panel: &Panel, people: &[u64], names: &[String]) -> Vec<Value> {
    people
        .iter()
        .zip(names)
        .enumerate()
        .map(|(i, (id, name))| json!({ "id": id.to_string(), "name": name, "letter": letter(i), "avatar": panel.data.cached_member(*id).map(|m| m.avatar) }))
        .collect()
}

/// "@Nikhil vs @Aisha vs @Rahul".
fn versus(names: &[String]) -> String {
    names.iter().map(|n| format!("@{}", n)).collect::<Vec<_>>().join(" vs ")
}

fn who(panel: &Panel, id: u64, fallback: &str) -> Value {
    json!({ "id": id.to_string(), "name": panel.cached_name(id).unwrap_or_else(|| if fallback.is_empty() { id.to_string() } else { fallback.to_string() }) })
}

fn channel_name(channel: &Value) -> String {
    channel["name"].as_str().unwrap_or("").to_string()
}

fn gone_counts(lines: &[Line]) -> (usize, usize) {
    (lines.iter().filter(|l| l.row.deleted()).count(), lines.iter().filter(|l| l.row.blocked()).count())
}

// --- the overview -------------------------------------------------------------------------------

fn summary_json(panel: &Panel, s: &Summary, current_key: Option<&str>, sensitive: &[u64]) -> Option<Value> {
    let n = &s.new;
    let period = n.scope == SCOPE_PERIOD;
    let channel = if period { Value::Null } else { channel_by_id(panel, n.channel_id, sensitive)? };
    Some(json!({
        "id": s.id,
        "key": n.stretch_key,
        "scope": if period { SCOPE_PERIOD } else { SCOPE_STRETCH },
        "current": current_key.is_none_or(|k| k == n.stretch_key),
        "people": n.people.iter().enumerate().map(|(i, id)| {
            let mut w = who(panel, *id, "");
            w["letter"] = json!(letter(i));
            w
        }).collect::<Vec<_>>(),
        "a": who(panel, n.a_id, ""),
        "b": who(panel, n.b_id, ""),
        "channel": channel,
        "start_ms": n.start_ms.to_string(),
        "end_ms": n.end_ms.to_string(),
        "start_ts": n.start_ms / 1000,
        "end_ts": n.end_ms / 1000,
        "detection_id": n.detection_id,
        "run_by": who(panel, n.run_by, ""),
        "run_ts": n.run_ts,
        "model": n.model,
        "input_tokens": n.input_tokens,
        "output_tokens": n.output_tokens,
        "trimmed": n.trimmed,
        "sent_count": n.sent_count,
        "message_count": n.message_ids.len(),
        "message_ids": n.message_ids.iter().map(|i| i.to_string()).collect::<Vec<_>>(),
        "summary": n.summary,
        "raw": if n.summary.is_none() { Some(n.raw.clone()) } else { None },
    }))
}

pub async fn overview(State(panel): State<Panel>) -> ApiResult {
    let sensitive = never_shown(&panel);
    let (detections, summaries, summarised) = {
        let conn = store_db()?.lock();
        let detections = store::detections(&conn, LIST_LIMIT).map_err(db_error)?;
        let summaries = store::recent_summaries(&conn, LIST_LIMIT).map_err(db_error)?;
        let ids: Vec<i64> = detections.iter().map(|d| d.id).collect();
        let summarised: HashSet<i64> = store::summarised_detections(&conn, &ids).map_err(db_error)?.into_iter().collect();
        (detections, summaries, summarised)
    };
    let detections: Vec<Value> = detections
        .iter()
        .filter_map(|d| {
            let channel = channel_by_id(&panel, d.channel_id, &sensitive)?;
            Some(json!({
                "id": d.id,
                "channel": channel,
                "start_ts": d.start_ms / 1000,
                "end_ts": d.end_ms / 1000,
                "created_ts": d.created_ts,
                "participants": d.participants.iter().map(|p| {
                    let mut m = member_json(&panel, p.id, &p.name, "");
                    m["messages"] = json!(p.messages);
                    m
                }).collect::<Vec<_>>(),
                "message_count": d.message_count,
                "line": d.line,
                "summarised": summarised.contains(&d.id),
            }))
        })
        .collect();
    let summaries: Vec<Value> = summaries.iter().filter_map(|s| summary_json(&panel, s, None, &sensitive)).collect();
    ok(json!({
        "detections": detections,
        "summaries": summaries,
        "settings": {
            "detector_on": super::super::on("VIZIER_KALESH", true),
            "watched_channels": super::super::ids("VIZIER_KALESH_CHANNELS").len(),
            "role_set": super::super::id("VIZIER_KALESH_ROLE_ID").is_some(),
            "max_messages": k::summary_max(),
            "model": k::summary_model(),
            "max_days": k::MAX_PERIOD_MS / (24 * HOUR_MS),
            "max_people": k::MAX_PEOPLE,
            "log_on": msglog::enabled(),
        },
    }))
}

// --- finding a fight between members ---------------------------------------------------------------

#[derive(Deserialize)]
pub struct FindQuery {
    /// Member ids, comma separated.
    #[serde(default)]
    people: Option<String>,
    #[serde(default)]
    a: Option<String>,
    #[serde(default)]
    b: Option<String>,
    /// How far back from now, in hours (default 24, at most a week).
    #[serde(default)]
    hours: Option<String>,
    /// A custom period instead: unix seconds, `from` inclusive, `to` inclusive.
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Period {
    pub since_ms: i64,
    pub until_ms: i64,
    pub label: String,
}

/// The period asked for: the last `hours`, or `from`..`to` (seconds), never
/// longer than a week and never reaching past now.
pub fn period(hours: &Option<String>, from: &Option<String>, to: &Option<String>, now_ms: i64) -> Result<Period, ApiError> {
    let max_days = k::MAX_PERIOD_MS / (24 * HOUR_MS);
    let secs = |v: &Option<String>, what: &str| -> Result<Option<i64>, ApiError> {
        match blank(v) {
            Some(raw) => raw.parse::<i64>().ok().filter(|s| *s > 0).map(Some).ok_or_else(|| ApiError::bad(format!("That isn't a {} time.", what))),
            None => Ok(None),
        }
    };
    match (secs(from, "start")?, secs(to, "end")?) {
        (Some(f), Some(t)) => {
            let (since_ms, until_ms) = (f * 1000, (t * 1000).min(now_ms));
            if since_ms >= until_ms {
                return Err(ApiError::bad("The start has to be before the end, and before now."));
            }
            if until_ms - since_ms > k::MAX_PERIOD_MS {
                return Err(ApiError::bad(format!("Pick at most {} days at a time.", max_days)));
            }
            Ok(Period { since_ms, until_ms, label: k::span(since_ms, until_ms) })
        }
        (None, None) => {
            let hours = match blank(hours) {
                Some(raw) => raw.parse::<i64>().map_err(|_| ApiError::bad("The period is a number of hours."))?,
                None => k::DEFAULT_PERIOD_MS / HOUR_MS,
            };
            if hours < 1 || hours * HOUR_MS > k::MAX_PERIOD_MS {
                return Err(ApiError::bad(format!("The period is between 1 hour and {} days.", max_days)));
            }
            let label = if hours % 24 == 0 && hours > 24 { format!("last {} days", hours / 24) } else if hours == 1 { "last hour".into() } else { format!("last {} hours", hours) };
            Ok(Period { since_ms: now_ms - hours * HOUR_MS, until_ms: now_ms, label })
        }
        _ => Err(ApiError::bad("Give both a start and an end.")),
    }
}

/// Everyone else who spoke, busiest first.
fn bystanders(panel: &Panel, lines: &[Line]) -> Vec<Value> {
    let mut counts: Vec<(u64, String, usize)> = Vec::new();
    for l in lines.iter().filter(|l| l.side == Side::Other) {
        match counts.iter_mut().find(|c| c.0 == l.row.author_id) {
            Some(c) => c.2 += 1,
            None => counts.push((l.row.author_id, l.row.author_name.clone(), 1)),
        }
    }
    counts.sort_by(|x, y| y.2.cmp(&x.2));
    counts.into_iter().map(|(id, name, n)| json!({ "id": id.to_string(), "name": panel.cached_name(id).unwrap_or(name), "messages": n })).collect()
}

fn per_person(lines: &[Line], people: &[u64]) -> Vec<usize> {
    (0..people.len()).map(|i| lines.iter().filter(|l| l.side == Side::Person(i)).count()).collect()
}

/// One stretch the search found, with everything said in it (bystanders,
/// deleted and blocked messages included).
struct Found {
    stretch: k::Stretch,
    channel: Value,
    rows: Vec<SaidRow>,
}

/// Every stretch of the chosen members over the period, newest first, each
/// with its messages; the members' own rows; and whether more were found than
/// are listed.
async fn search_period(panel: &Panel, people: &[u64], per: &Period, sensitive: &[u64]) -> Result<(Vec<Found>, Vec<SaidRow>, bool), ApiError> {
    let mut own = panel.data.kalesh_authors(people.to_vec(), per.since_ms, per.until_ms, None).await.map_err(unavailable)?;
    own.retain(|r| place_json(panel, r, sensitive).is_some());
    let found = k::find_stretches(&own, people);
    let more = found.len() > MAX_STRETCHES;
    let mut out = Vec::new();
    for s in found.into_iter().take(MAX_STRETCHES) {
        let mut rows = panel.data.kalesh_channel(s.channel_id, s.start_ms, s.end_ms).await.map_err(unavailable)?;
        rows.retain(|r| place_json(panel, r, sensitive).is_some());
        let Some(channel) = rows.first().and_then(|r| place_json(panel, r, sensitive)) else { continue };
        out.push(Found { stretch: s, channel, rows });
    }
    Ok((out, own, more))
}

/// Every stretch's messages together, oldest first, once each, at most the page's cap.
fn merged(found: &[Found]) -> (Vec<SaidRow>, bool) {
    let mut all: Vec<SaidRow> = found.iter().flat_map(|f| f.rows.iter().cloned()).collect();
    all.sort_by_key(|r| r.message_id);
    all.dedup_by_key(|r| r.message_id);
    let truncated = all.len() > k::EXCHANGE_ROWS;
    all.truncate(k::EXCHANGE_ROWS);
    (all, truncated)
}

pub async fn find(State(panel): State<Panel>, axum::Extension(Caller(user)): axum::Extension<Caller>, Query(q): Query<FindQuery>) -> ApiResult {
    let people = people_of(&q.people, &q.a, &q.b)?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let per = period(&q.hours, &q.from, &q.to, now_ms)?;
    let sensitive = never_shown(&panel);
    let (found, own, more) = search_period(&panel, &people, &per, &sensitive).await?;
    let names = names_of(&panel, &people, &own).await;
    search::log_quietly("kalesh:find", user, &format!("{} · {}", versus(&names), per.label));

    let mut out = Vec::new();
    let mut keys = Vec::new();
    for f in &found {
        let s = &f.stretch;
        let lines = k::exchange(&f.rows, &people);
        let key = k::stretch_key(&people, s.channel_id, &lines);
        keys.push(key.clone());
        let (deleted, blocked) = gone_counts(&lines);
        out.push(json!({
            "key": key,
            "channel": f.channel,
            "start_ms": s.start_ms.to_string(),
            "end_ms": s.end_ms.to_string(),
            "start_ts": s.start_ms / 1000,
            "end_ts": s.end_ms / 1000,
            "messages": lines.len(),
            "per_person": per_person(&lines, &people),
            "others": lines.iter().filter(|l| l.side == Side::Other).count(),
            "bystanders": bystanders(&panel, &lines),
            "replies": s.reply_count(),
            "replies_between": s.replies.iter().map(|(from, to, n)| json!({ "from": letter(*from), "to": letter(*to), "count": n })).collect::<Vec<_>>(),
            "mentions": s.mentions,
            "nearby": s.nearby,
            "deleted": deleted,
            "blocked": blocked,
        }));
    }
    let (all, truncated) = merged(&found);
    let all_lines = k::exchange(&all, &people);
    let period_key = k::period_key(&people, &all_lines);
    keys.push(period_key.clone());
    let summarised: HashSet<String> = match store::db() {
        Some(db) => store::summarised_keys(&db.lock(), &keys).map_err(db_error)?.into_iter().collect(),
        None => HashSet::new(),
    };
    for s in &mut out {
        let done = s["key"].as_str().is_some_and(|k| summarised.contains(k));
        s["summarised"] = json!(done);
    }
    let (deleted, blocked) = gone_counts(&all_lines);
    let max = k::summary_max();
    ok(json!({
        "people": people_json(&panel, &people, &names),
        "since_ts": per.since_ms / 1000,
        "until_ts": per.until_ms / 1000,
        "label": per.label,
        "stretches": out,
        "more": more,
        "read": own.len(),
        "log_on": msglog::enabled(),
        // The whole period: every stretch together. This is what a mod usually wants summarised.
        "period": {
            "key": period_key,
            "stretches": found.len(),
            "messages": all_lines.len(),
            "per_person": per_person(&all_lines, &people),
            "deleted": deleted,
            "blocked": blocked,
            "truncated": truncated,
            "max_messages": max,
            "will_trim": all_lines.len() > max,
            "summarised": summarised.contains(&period_key),
        },
    }))
}

// --- one stretch, or a whole period, message by message ---------------------------------------------

#[derive(Deserialize, Default)]
pub struct ExchangeQuery {
    #[serde(default)]
    people: Option<String>,
    #[serde(default)]
    a: Option<String>,
    #[serde(default)]
    b: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    /// Milliseconds, both inclusive.
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    end: Option<String>,
    #[serde(default)]
    detection: Option<String>,
    /// A whole period instead of one stretch: as the search takes it.
    #[serde(default)]
    hours: Option<String>,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
}

enum What {
    Stretch { channel: u64, channel_json: Value },
    Period { label: String, stretches: Vec<Value> },
}

struct Loaded {
    people: Vec<u64>,
    names: Vec<String>,
    what: What,
    start_ms: i64,
    end_ms: i64,
    rows: Vec<SaidRow>,
    truncated: bool,
    detection: Option<store::Detection>,
}

impl Loaded {
    fn lines(&self) -> Vec<Line<'_>> {
        k::exchange(&self.rows, &self.people)
    }
    fn key(&self, lines: &[Line]) -> String {
        match &self.what {
            What::Stretch { channel, .. } => k::stretch_key(&self.people, *channel, lines),
            What::Period { .. } => k::period_key(&self.people, lines),
        }
    }
    fn label(&self) -> String {
        match &self.what {
            What::Stretch { channel_json, .. } => format!("{} · #{} · {}", versus(&self.names), channel_name(channel_json), k::span(self.start_ms, self.end_ms)),
            What::Period { label, stretches } => format!("{} · whole period, {} · {}", versus(&self.names), plural(stretches.len(), "stretch", "stretches"), label),
        }
    }
    fn channel(&self) -> u64 {
        match &self.what {
            What::Stretch { channel, .. } => *channel,
            What::Period { .. } => 0,
        }
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", n, if n == 1 { one } else { many })
}

/// A stretch when a channel is given, the whole period otherwise.
async fn load(panel: &Panel, q: &ExchangeQuery) -> Result<Loaded, ApiError> {
    let people = people_of(&q.people, &q.a, &q.b)?;
    let detection = match blank(&q.detection) {
        Some(raw) => {
            let id: i64 = raw.parse().map_err(|_| ApiError::bad("That isn't a detection."))?;
            let d = store::detection(&store_db()?.lock(), id).map_err(db_error)?;
            Some(d.ok_or_else(|| ApiError::not_found("No such detection."))?)
        }
        None => None,
    };
    let sensitive = never_shown(panel);
    if blank(&q.channel).is_none() {
        let per = period(&q.hours, &q.from, &q.to, chrono::Utc::now().timestamp_millis())?;
        let (found, own, _) = search_period(panel, &people, &per, &sensitive).await?;
        let (rows, truncated) = merged(&found);
        let names = names_of(panel, &people, &own).await;
        let mut stretches: Vec<(i64, Value)> = found
            .iter()
            .map(|f| {
                let first = f.rows.first().map(|r| r.message_id.to_string());
                let last = f.rows.last().map(|r| r.message_id.to_string());
                (f.stretch.start_ms, json!({ "channel": f.channel, "start_ts": f.stretch.start_ms / 1000, "end_ts": f.stretch.end_ms / 1000, "first_id": first, "last_id": last }))
            })
            .collect();
        stretches.sort_by_key(|s| s.0);
        return Ok(Loaded {
            people,
            names,
            what: What::Period { label: per.label, stretches: stretches.into_iter().map(|s| s.1).collect() },
            start_ms: per.since_ms,
            end_ms: per.until_ms,
            rows,
            truncated,
            detection,
        });
    }
    let channel = blank(&q.channel).and_then(|c| parse_id(&c)).ok_or_else(|| ApiError::bad("That isn't a channel id."))?;
    let ms = |v: &Option<String>| blank(v).and_then(|s| s.parse::<i64>().ok()).filter(|m| *m > 0);
    let (Some(start_ms), Some(end_ms)) = (ms(&q.start), ms(&q.end)) else {
        return Err(ApiError::bad("That isn't a stretch of time."));
    };
    if end_ms < start_ms || end_ms - start_ms > k::MAX_PERIOD_MS {
        return Err(ApiError::bad("That isn't a stretch of time."));
    }
    let name = panel.data.channels().iter().find(|c| c.id == channel.to_string()).map(|c| c.name.clone()).unwrap_or_default();
    if sensitive.contains(&channel) || weekly::is_safe_corner(channel, &name) {
        return Err(ApiError::bad("That channel is never kept."));
    }
    let mut rows = panel.data.kalesh_channel(channel, start_ms, end_ms).await.map_err(unavailable)?;
    rows.retain(|r| place_json(panel, r, &sensitive).is_some());
    let truncated = rows.len() >= k::EXCHANGE_ROWS;
    rows.truncate(k::EXCHANGE_ROWS);
    let channel_json = match rows.first() {
        Some(r) => place_json(panel, r, &sensitive),
        None => channel_by_id(panel, channel, &sensitive),
    }
    .ok_or_else(|| ApiError::bad("That channel is never kept."))?;
    let names = names_of(panel, &people, &rows).await;
    Ok(Loaded { people, names, what: What::Stretch { channel, channel_json }, start_ms, end_ms, rows, truncated, detection })
}

fn message_json(panel: &Panel, l: &Line, guild: Option<&String>, channel: Option<&Value>, sensitive: &[u64], detected: &HashSet<u64>) -> Value {
    let r = l.row;
    let side = |s: Side| json!(s);
    let channel = match channel {
        Some(c) => c.clone(),
        None => place_json(panel, r, sensitive).unwrap_or(Value::Null),
    };
    json!({
        "n": l.n,
        "id": r.message_id.to_string(),
        "ts": r.created_ms / 1000,
        "ts_ms": r.created_ms,
        "member": member_json(panel, r.author_id, &r.author_name, &r.avatar),
        "side": side(l.side),
        "person": match l.side { Side::Person(i) => Some(i), Side::Other => None },
        "towards": l.towards.map(side),
        "text": r.content,
        "reply_to": r.reply_to.map(|id| json!({ "id": id.to_string(), "n": l.reply_n, "author": r.reply_author, "text": r.reply_text })),
        // Pictures the log saved (live or deleted), served by the panel; stickers
        // from Discord's sticker CDN, whose links never expire; other files by name.
        "images": r.images.iter().map(|f| json!({ "n": f.n, "name": f.name, "url": format!("/api/kalesh/picture/{}/{}", r.message_id, f.n) })).collect::<Vec<_>>(),
        "stickers": r.attachments.iter().filter(|a| msglog::is_sticker(a)).map(|a| json!({ "name": a.filename, "url": msglog::sticker_url(a) })).collect::<Vec<_>>(),
        "files": r.attachments.iter().filter(|a| !msglog::is_sticker(a) && !r.images.iter().any(|f| f.name == a.filename)).map(|a| json!({
            "name": a.filename,
            "image": msglog::image_ext(a.content_type.as_deref(), &a.filename).is_some(),
        })).collect::<Vec<_>>(),
        // A deleted or blocked message is not in Discord: there is nothing to jump to.
        "url": if r.gone.is_some() { None } else { guild.map(|g| search::jump_url(g, r.channel_id, r.message_id)) },
        "channel": channel,
        "gone": r.gone,
        "in_detection": detected.contains(&r.message_id),
    })
}

async fn summaries_for(panel: &Panel, loaded: &Loaded, key: &str) -> Result<Vec<Value>, ApiError> {
    let sensitive = never_shown(panel);
    let list = {
        let conn = store_db()?.lock();
        match &loaded.what {
            What::Stretch { channel, .. } => store::summaries_near(&conn, *channel, &loaded.people, loaded.start_ms, loaded.end_ms),
            What::Period { .. } => store::period_summaries(&conn, &loaded.people, loaded.start_ms, loaded.end_ms),
        }
        .map_err(db_error)?
    };
    Ok(list.iter().filter_map(|s| summary_json(panel, s, Some(key), &sensitive)).collect())
}

async fn exchange_json(panel: &Panel, loaded: &Loaded) -> Result<Value, ApiError> {
    let lines = loaded.lines();
    let key = loaded.key(&lines);
    let guild = panel.data.guild().map(|g| g.id);
    let detected: HashSet<u64> = loaded.detection.as_ref().map(|d| d.message_ids.iter().copied().collect()).unwrap_or_default();
    let sensitive = never_shown(panel);
    let summaries = summaries_for(panel, loaded, &key).await?;
    let max = k::summary_max();
    let (deleted, blocked) = gone_counts(&lines);
    let (scope, channel, stretches) = match &loaded.what {
        What::Stretch { channel_json, .. } => (SCOPE_STRETCH, channel_json.clone(), Value::Null),
        What::Period { stretches, .. } => (SCOPE_PERIOD, Value::Null, json!(stretches)),
    };
    let fixed = match &loaded.what {
        What::Stretch { channel_json, .. } => Some(channel_json),
        What::Period { .. } => None,
    };
    let label = match &loaded.what {
        What::Period { label, .. } => label.clone(),
        What::Stretch { .. } => k::span(loaded.start_ms, loaded.end_ms),
    };
    Ok(json!({
        "scope": scope,
        "people": people_json(panel, &loaded.people, &loaded.names),
        "channel": channel,
        "stretches": stretches,
        "label": label,
        "start_ms": loaded.start_ms.to_string(),
        "end_ms": loaded.end_ms.to_string(),
        "start_ts": loaded.start_ms / 1000,
        "end_ts": loaded.end_ms / 1000,
        "key": key,
        "count": lines.len(),
        "per_person": per_person(&lines, &loaded.people),
        "bystanders": bystanders(panel, &lines),
        "deleted": deleted,
        "blocked": blocked,
        "truncated": loaded.truncated,
        "max_messages": max,
        "will_trim": lines.len() > max,
        "detection_id": loaded.detection.as_ref().map(|d| d.id),
        "messages": lines.iter().map(|l| message_json(panel, l, guild.as_ref(), fixed, &sensitive, &detected)).collect::<Vec<_>>(),
        "summaries": summaries,
    }))
}

/// One stretch in one channel.
pub async fn exchange(State(panel): State<Panel>, axum::Extension(Caller(user)): axum::Extension<Caller>, Query(q): Query<ExchangeQuery>) -> ApiResult {
    if blank(&q.channel).is_none() {
        return Err(ApiError::bad("That isn't a channel id."));
    }
    let loaded = load(&panel, &q).await?;
    search::log_quietly("kalesh:look", user, &loaded.label());
    ok(exchange_json(&panel, &loaded).await?)
}

/// Every stretch of the chosen members over a period, together in time order.
pub async fn whole_period(State(panel): State<Panel>, axum::Extension(Caller(user)): axum::Extension<Caller>, Query(mut q): Query<ExchangeQuery>) -> ApiResult {
    q.channel = None;
    let loaded = load(&panel, &q).await?;
    search::log_quietly("kalesh:look", user, &loaded.label());
    ok(exchange_json(&panel, &loaded).await?)
}

/// A saved picture of a message on the Kalesh page, still in Discord or deleted.
/// Admins only, like every panel call; never from a channel that is never shown.
pub async fn picture(State(panel): State<Panel>, Path((id, n)): Path<(String, String)>) -> ApiResult {
    use axum::body::Body;
    use axum::http::{HeaderValue, header};
    use axum::response::{IntoResponse, Response};
    let id = parse_id(&id).ok_or_else(|| ApiError::bad("That isn't a message id."))?;
    let n = (n.len() == 1).then(|| n.parse::<usize>().ok()).flatten().filter(|n| *n < msglog::MAX_IMAGES).ok_or_else(|| ApiError::bad("That isn't a picture number."))?;
    let pic = panel.data.kalesh_picture(id, n).await.ok_or_else(|| ApiError::not_found("No such picture."))?;
    if channel_json(&pic.place, &panel.data.channels(), &never_shown(&panel)).is_none() {
        return Err(ApiError::not_found("No such picture."));
    }
    let mut res = Response::new(Body::from(pic.bytes));
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(pic.kind));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=3600"));
    h.insert(header::CONTENT_DISPOSITION, HeaderValue::from_static("inline"));
    Ok(res.into_response())
}

// --- a detection, opened ---------------------------------------------------------------------------

/// Which stretch a detection opens: the people who were really in it (everyone
/// with two or more messages in the burst, at most six, at least the two
/// busiest), and the stretch of their engagement around the detector's window,
/// or the window itself.
pub async fn detection(State(panel): State<Panel>, Path(id): Path<String>) -> ApiResult {
    let id: i64 = id.trim().parse().map_err(|_| ApiError::not_found("No such detection."))?;
    let d = store::detection(&store_db()?.lock(), id).map_err(db_error)?.ok_or_else(|| ApiError::not_found("No such detection."))?;
    let sensitive = never_shown(&panel);
    let channel = channel_by_id(&panel, d.channel_id, &sensitive).ok_or_else(|| ApiError::not_found("No such detection."))?;
    if d.participants.len() < 2 {
        return Err(ApiError::bad("Only one person was in that burst, so there is no fight between people to look at."));
    }
    let chosen: Vec<&store::Participant> =
        d.participants.iter().enumerate().filter(|(i, p)| *i < 2 || p.messages >= 2).map(|(_, p)| p).take(k::MAX_PEOPLE).collect();
    let people: Vec<u64> = chosen.iter().map(|p| p.id).collect();
    let around = 3 * HOUR_MS;
    let rows = panel.data.kalesh_authors(people.clone(), d.start_ms - around, d.end_ms + around, Some(d.channel_id)).await.map_err(unavailable)?;
    let (mut start_ms, mut end_ms) = (d.start_ms, d.end_ms);
    if let Some(s) = k::find_stretches(&rows, &people)
        .into_iter()
        .find(|s| s.start_ms <= d.end_ms + k::JOIN_GAP_MS && s.end_ms >= d.start_ms - k::JOIN_GAP_MS)
    {
        start_ms = start_ms.min(s.start_ms);
        end_ms = end_ms.max(s.end_ms);
    }
    let named: Vec<Value> = chosen
        .iter()
        .enumerate()
        .map(|(i, p)| json!({ "id": p.id.to_string(), "name": panel.cached_name(p.id).unwrap_or_else(|| p.name.clone()), "letter": letter(i) }))
        .collect();
    ok(json!({
        "id": d.id,
        "people": named,
        "a": named[0],
        "b": named[1],
        "channel": channel,
        "start_ms": start_ms.to_string(),
        "end_ms": end_ms.to_string(),
    }))
}

// --- summarising -----------------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct SummariseBody {
    #[serde(default)]
    people: Option<String>,
    #[serde(default)]
    a: Option<String>,
    #[serde(default)]
    b: Option<String>,
    /// "period" for the whole period, "stretch" (or a channel given) for one stretch.
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    end: Option<String>,
    #[serde(default)]
    detection: Option<String>,
    #[serde(default)]
    hours: Option<String>,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
}

/// Summaries being written right now, so two mods pressing at once pay once.
static RUNNING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

struct Running(String);

impl Running {
    fn claim(key: &str) -> Option<Self> {
        RUNNING.lock().insert(key.to_string()).then(|| Running(key.to_string()))
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        RUNNING.lock().remove(&self.0);
    }
}

/// Why a model call failed, in words a mod can act on.
pub fn why_failed(err: &str) -> &'static str {
    let e = err.to_ascii_lowercase();
    if e.contains("took over") || e.contains("timed out") || e.contains("timeout") {
        "it took too long to answer"
    } else if e.contains("error sending request") || e.contains("connect") || e.contains("dns") || e.contains("http client error") || e.contains("connection") {
        "network error"
    } else if e.contains("empty") {
        "it sent back nothing"
    } else if e.contains("429") || e.contains("rate") {
        "the provider is rate-limiting us"
    } else {
        "the provider returned an error"
    }
}

/// The model's reply, tried up to [`TRIES`] times with a wait between: the
/// provider does fail once now and then, and a second try usually works. An
/// empty reply counts as a failure. The error is the reason, in words.
async fn ask_model(panel: &Panel, prompt: &str) -> Result<k::Reply, String> {
    let mut last = String::new();
    for attempt in 1..=TRIES {
        match panel.data.kalesh_summarise(prompt.to_string()).await {
            Ok(reply) if !reply.text.trim().is_empty() => return Ok(reply),
            Ok(_) => {
                last = "the model sent back an empty reply".into();
                tracing::warn!("kalesh: summary try {} of {} came back empty", attempt, TRIES);
            }
            Err(err) => {
                last = err.to_string();
                tracing::warn!("kalesh: summary try {} of {} failed: {}", attempt, TRIES, err);
            }
        }
        if let Some(wait) = RETRY_WAITS.get(attempt - 1) {
            tokio::time::sleep(*wait).await;
        }
    }
    Err(last)
}

pub async fn summarise(State(panel): State<Panel>, axum::Extension(Caller(user)): axum::Extension<Caller>, body: axum::body::Bytes) -> ApiResult {
    let body: SummariseBody = serde_json::from_slice(&body).map_err(|_| ApiError::bad("Pick what to summarise."))?;
    let whole = body.scope.as_deref() == Some(SCOPE_PERIOD) || blank(&body.channel).is_none();
    let q = ExchangeQuery {
        people: body.people,
        a: body.a,
        b: body.b,
        channel: if whole { None } else { body.channel },
        start: body.start,
        end: body.end,
        detection: body.detection,
        hours: body.hours,
        from: body.from,
        to: body.to,
    };
    let loaded = load(&panel, &q).await?;
    let lines = loaded.lines();
    if lines.is_empty() {
        return Err(ApiError::bad(if whole {
            "Nothing they said to each other is kept for that period, so there is nothing to summarise."
        } else {
            "No messages are kept for that stretch, so there is nothing to summarise."
        }));
    }
    let key = loaded.key(&lines);
    let sensitive = never_shown(&panel);
    let label = loaded.label();

    if let Some(done) = store::summary_for(&store_db()?.lock(), &key).map_err(db_error)? {
        search::log_quietly("kalesh:summary", user, &label);
        return ok(json!({ "reused": true, "summary": summary_json(&panel, &done, Some(&key), &sensitive) }));
    }
    let Some(_running) = Running::claim(&key) else {
        return Err(ApiError(StatusCode::CONFLICT, "Someone is summarising this right now. Give it a minute and open it again.".into()));
    };

    let scope = match &loaded.what {
        What::Stretch { channel_json, .. } => Scope::Stretch { channel_name: channel_json["name"].as_str().unwrap_or("") },
        What::Period { label, stretches } => Scope::Period { label, stretches: stretches.len() },
    };
    let prompt = k::build_prompt(&loaded.names, scope, &lines, k::summary_max());
    let reply = ask_model(&panel, &prompt.text).await.map_err(|err| {
        tracing::warn!("kalesh: a summary failed after {} tries: {}", TRIES, err);
        ApiError(
            StatusCode::BAD_GATEWAY,
            format!(
                "The summary model didn't answer ({}), even after {} tries. Nothing was saved or charged twice — press Try again in a minute.",
                why_failed(&err),
                TRIES
            ),
        )
    })?;
    let parsed = k::parse_summary(&reply.text, lines.len());
    let new = store::NewSummary {
        stretch_key: key.clone(),
        channel_id: loaded.channel(),
        a_id: loaded.people[0],
        b_id: loaded.people[1],
        people: loaded.people.clone(),
        scope: if whole { SCOPE_PERIOD } else { SCOPE_STRETCH }.to_string(),
        start_ms: loaded.start_ms,
        end_ms: loaded.end_ms,
        detection_id: loaded.detection.as_ref().map(|d| d.id),
        message_ids: lines.iter().map(|l| l.row.message_id).collect(),
        sent_count: prompt.sent,
        trimmed: prompt.trimmed,
        run_by: user,
        run_ts: chrono::Utc::now().timestamp(),
        model: reply.model,
        input_tokens: reply.input_tokens,
        output_tokens: reply.output_tokens,
        summary: parsed,
        raw: reply.text,
    };
    let id = store::add_summary(&store_db()?.lock(), &new).map_err(db_error)?;
    search::log_quietly("kalesh:summary", user, &label);
    tracing::info!("kalesh: {} summarised {} ({} + {} tokens)", user, label, new.input_tokens, new.output_tokens);
    ok(json!({ "reused": false, "summary": summary_json(&panel, &Summary { id, new }, Some(&key), &sensitive) }))
}

// --- the activity log --------------------------------------------------------------------------------

/// How a look at a fight reads in the activity log: who looked into whom.
pub fn audit_entry(e: &super::super::AuditEntry) -> serde_json::Map<String, Value> {
    let mut obj = serde_json::Map::new();
    let label = match e.key.as_str() {
        "kalesh:summary" => "Summarised a kalesh",
        "kalesh:find" => "Looked for a kalesh",
        _ => "Read a kalesh",
    };
    obj.insert("label".into(), json!(label));
    obj.insert("section".into(), json!({ "id": "kalesh", "title": "Kalesh", "icon": "🍿" }));
    obj.insert("change".into(), json!(e.new.clone().unwrap_or_else(|| label.to_string())));
    obj.insert("old".into(), Value::Null);
    obj.insert("new".into(), Value::Null);
    obj
}

// --- the fake message log, for the tests and the demo server ----------------------------------------------

#[cfg(test)]
pub mod fake {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::super::super::super::kalesh::{self as k, Reply};
    use super::super::super::super::msglog::{Alert, Attachment, AuditDelete, Blocked, Deletion, NewMessage, Place, SaidRow, Store, StoredFile, day_folder};

    pub const NIKHIL: u64 = 2020;
    pub const AISHA: u64 = 2021;
    pub const RAHUL: u64 = 2022;
    pub const KAVYA: u64 = 2023;
    pub const VARUN: u64 = 2024;
    pub const MYRA: u64 = 2025;
    pub const SID: u64 = 2026;
    pub const NEHA: u64 = 2027;
    pub const OM: u64 = 2036;
    /// The moderator who deleted one of Nikhil's messages.
    pub const MEERA: u64 = 2003;
    pub const SAFE: u64 = super::super::tests::SAFE;
    const MIN: i64 = 60_000;
    const HOUR: i64 = 60 * MIN;

    pub struct FightLog {
        pub now: i64,
        pub store: parking_lot::Mutex<Store>,
        /// The message with a picture in the #desi-banter fight.
        pub scorecard: u64,
        _dir: tempfile::TempDir,
    }

    fn id_at(ms: i64, seq: u64) -> u64 {
        (((ms - 1_420_070_400_000) as u64) << 22) | (seq & 0x3f_ffff)
    }

    fn name(id: u64) -> &'static str {
        match id {
            NIKHIL => "Nikhil",
            AISHA => "Aisha",
            RAHUL => "Rahul",
            KAVYA => "Kavya",
            VARUN => "Varun",
            MYRA => "Myra",
            SID => "Siddharth",
            NEHA => "Neha",
            OM => "Om",
            _ => "Someone",
        }
    }

    fn message(seq: &mut u64, at: i64, author: u64, channel: (u64, &str), text: &str, reply: Option<u64>) -> NewMessage {
        *seq += 1;
        NewMessage {
            message_id: id_at(at, *seq),
            place: Place { channel_id: channel.0, parent_id: None, channel_name: channel.1.into() },
            guild_id: 900,
            author_id: author,
            author_name: name(author).into(),
            avatar: String::new(),
            content: text.into(),
            created_ms: at,
            reply_to: reply,
            reply_author: None,
            reply_text: None,
            attachments: vec![],
        }
    }

    /// One message at `at`; `reply` is the id it answers.
    fn put(store: &mut Store, seq: &mut u64, at: i64, author: u64, channel: (u64, &str), text: &str, reply: Option<u64>) -> u64 {
        let m = message(seq, at, author, channel, text, reply);
        store.insert_new(&m).expect("fake fight message");
        m.message_id
    }

    /// A message that was posted and then deleted `after` later, by `by` (per the audit log) or by nobody on record.
    #[allow(clippy::too_many_arguments)]
    fn put_deleted(store: &mut Store, seq: &mut u64, at: i64, author: u64, channel: (u64, &str), text: &str, after: i64, by: Option<(u64, &str)>) -> u64 {
        let id = put(store, seq, at, author, channel, text, None);
        let when = at + after;
        let mut removals = Vec::new();
        store
            .delete_noting(&Deletion { ids: vec![id], place: Place { channel_id: channel.0, parent_id: None, channel_name: channel.1.into() }, ts_ms: when, bulk: false }, 365, &mut removals)
            .expect("fake delete");
        *seq += 1;
        let entries: Vec<AuditDelete> = by
            .map(|(who, name)| AuditDelete {
                id: id_at(when + 500, *seq),
                executor: who,
                executor_name: name.into(),
                executor_bot: false,
                target: Some(author),
                channel: Some(channel.0),
                count: 1,
                bulk: false,
            })
            .into_iter()
            .collect();
        store.record_deleters(&removals, Some(&entries)).expect("fake deleters");
        id
    }

    /// A message AutoMod blocked, as its alert records it.
    fn put_blocked(store: &mut Store, seq: &mut u64, at: i64, author: u64, channel: (u64, &str), text: &str, keyword: &str) -> u64 {
        *seq += 1;
        let id = id_at(at, *seq);
        store
            .insert_blocked(&Blocked {
                message_id: id,
                place: Place { channel_id: channel.0, parent_id: None, channel_name: channel.1.into() },
                author_id: author,
                author_name: name(author).into(),
                avatar: String::new(),
                created_ms: at,
                alert: Alert {
                    content: text.into(),
                    rule_name: Some("Block slurs".into()),
                    channel_id: Some(channel.0),
                    decision_id: Some(format!("{}", 1_419_000_000_000_000_000u64 + *seq)),
                    keyword: Some(format!("*{}*", keyword)),
                    matched: Some(keyword.into()),
                    outcome: Some("blocked".into()),
                },
                alert_channel: 1_516_779_799_865_987_101,
            })
            .expect("fake blocked");
        id
    }

    /// Nikhil and Aisha: a proper cricket kalesh in #desi-banter twenty hours ago,
    /// with Rahul trying to calm it and Kavya joining in, a picture, one of
    /// Nikhil's messages deleted by a mod and one blocked by AutoMod; a small
    /// back-and-forth in #general three hours ago; an older one in #memes three
    /// days ago; and a ping in #safe-corner and one nine days back that must
    /// never show. Varun, Myra, Siddharth and Neha: a four-way argument in
    /// #cricket-talk six hours ago, Om watching. Rahul and Kavya: short
    /// exchanges in #flaky and #down, where the fake model fails.
    pub fn log() -> &'static FightLog {
        static LOG: OnceLock<FightLog> = OnceLock::new();
        LOG.get_or_init(|| {
            let dir = tempfile::tempdir().expect("temp dir");
            let now = chrono::Utc::now().timestamp_millis();
            let mut store = Store::open(&dir.path().join("fight.db"), dir.path().join("files"), now - 30 * 24 * HOUR).expect("fight store");
            let mut seq = 0u64;
            let s = &mut store;
            let q = &mut seq;
            let banter = (23, "desi-banter");
            let t = now - 20 * HOUR;
            // (seconds in, who, what, which earlier line it replies to, counting from 1)
            let lines: &[(i64, u64, &str, Option<usize>)] = &[
                (0, RAHUL, "anyone watching the RCB match tonight?", None),
                (60, NIKHIL, "RCB is winning the cup this year, screenshot this", None),
                (100, AISHA, "bhai har saal yahi bolta hai tu 💀", Some(2)),
                (160, NIKHIL, "this year is different, bowling is actually good", Some(3)),
                (230, AISHA, "bowling good? 220 de diye last match mein", Some(4)),
                (290, NIKHIL, "one bad match. CSK fans ko bas stats yaad rehte hai", Some(5)),
                (330, AISHA, "stats hi toh cricket hai, feelings se trophy nahi milti", Some(6)),
                (400, NIKHIL, "tu cricket dekhti bhi hai ya sirf memes?", Some(7)),
                (450, AISHA, "okay now it's personal lol. I've watched more matches than you", Some(8)),
                (520, RAHUL, "guys chill, it's just IPL", None),
                (560, NIKHIL, "<@2021> naam bata 2019 final ka man of the match, bina google kiye", None),
                (640, AISHA, "I'm not doing a quiz for you. you always do this when you're losing", Some(11)),
                (700, KAVYA, "aisha is right tbh, RCB fans are delusional every april", Some(11)),
                (760, NIKHIL, "haan aa gayi team, 2 vs 1 😂", Some(13)),
                (820, AISHA, "nobody is teaming up, you just can't take one joke back", Some(14)),
                (900, NIKHIL, "joke? tune pehle bola main har saal bolta hu, that was the joke", Some(15)),
                (960, AISHA, "and you called me a memes-only fan. which is worse", Some(16)),
                (1030, NIKHIL, "tu Pune wale college mein hai na, sabko bata du kitna cricket aata hai tujhe 😂", Some(17)),
                (1080, AISHA, "don't bring my college into this. stop.", Some(18)),
                (1140, RAHUL, "nikhil bhai that's too much, drop it", Some(18)),
                (1200, NIKHIL, "chill I didn't say the name", Some(20)),
                (1260, AISHA, "whatever. I'm muting this channel for tonight", None),
                (1400, KAVYA, "rip the chat", None),
                (1560, NIKHIL, "fine. sorry for the college thing aisha", Some(22)),
                (1700, AISHA, "ok. still RCB won't win", Some(24)),
                (1760, NIKHIL, "we'll see in may 😤", Some(25)),
            ];
            let mut ids: Vec<u64> = Vec::new();
            for (secs, who, text, reply) in lines {
                let reply = reply.map(|n| ids[n - 1]);
                let id = put(s, q, t + secs * 1000, *who, banter, text, reply);
                ids.push(id);
            }
            // Aisha's scorecard, with its picture saved the way the log saves one.
            let mut card = message(q, t + 240_000, AISHA, banter, "see for yourself", Some(ids[3]));
            card.attachments = vec![Attachment { id: 77, filename: "scorecard.png".into(), content_type: Some("image/png".into()), size: 900, url: String::new() }];
            s.insert_new(&card).expect("scorecard");
            let rel = format!("{}/{}_0.png", day_folder(card.created_ms), card.message_id);
            let path = s.root().join(&rel);
            std::fs::create_dir_all(path.parent().expect("day folder")).expect("day folder");
            let png = super::super::tests::fake_png(320, 200, 140.0);
            std::fs::write(&path, &png).expect("scorecard png");
            s.saved(card.message_id, StoredFile { n: 0, path: rel, bytes: png.len() as u64, name: "scorecard.png".into() }).expect("scorecard saved");
            // Right after the college message: one of Nikhil's, deleted three
            // minutes later by Meera, and one AutoMod stopped before anyone saw it.
            put_deleted(s, q, t + 1_050_000, NIKHIL, banter, "sab ko pata hai tu kaun hai, zyada mat bol 🤡", 3 * MIN, Some((MEERA, "Meera")));
            put_blocked(s, q, t + 1_110_000, NIKHIL, banter, "tu ekdum chutiya hai, college waali", "chutiya");
            // The detector caught the middle of it: messages 10 to 21 of the lines above.
            if let Some(db) = super::super::super::super::kalesh_store::db() {
                let window: Vec<k::Seen> = (9..21)
                    .map(|i| k::Seen { id: ids[i], ts_ms: t + lines[i].0 * 1000, author: lines[i].1, author_name: name(lines[i].1).into() })
                    .collect();
                if let Some(d) = k::detection_of(banter.0, &window, "Kalesh alert 🍿 RCB vs CSK fans at it again, popcorn ready", now / 1000 - 20 * 3600) {
                    let _ = super::super::super::super::kalesh_store::add_detection(&db.lock(), &d);
                }
            }
            // #general, three hours ago: close together, no replies.
            let general = (21, "general");
            let g = now - 3 * HOUR;
            put(s, q, g, AISHA, general, "who is up for quiz at 10", None);
            put(s, q, g + MIN, NIKHIL, general, "me, but not on Aisha's team 😂", None);
            put(s, q, g + 3 * MIN, AISHA, general, "good, I want to win", None);
            put(s, q, g + 4 * MIN, NIKHIL, general, "we'll see", None);
            // #memes, three days ago: a short one with a mention.
            let memes = (22, "memes");
            let m = now - 72 * HOUR;
            let first = put(s, q, m, NIKHIL, memes, "<@2021> this meme is about you", None);
            put(s, q, m + 2 * MIN, AISHA, memes, "at least I'm funny", Some(first));
            // #cricket-talk, six hours ago: four of them, Om watching.
            let talk = (25, "cricket-talk");
            let c = now - 6 * HOUR;
            let v1 = put(s, q, c, VARUN, talk, "kohli > babar, no debate", None);
            let m1 = put(s, q, c + 40_000, MYRA, talk, "babar's cover drive is better and you know it", Some(v1));
            let s1 = put(s, q, c + 90_000, SID, talk, "<@2024> stats dekh bhai, babar zyada consistent hai", None);
            let v2 = put(s, q, c + 150_000, VARUN, talk, "consistency against zimbabwe lol", Some(s1));
            put(s, q, c + 200_000, MYRA, talk, "here we go again", Some(v2));
            put(s, q, c + 260_000, SID, talk, "tu har baar yahi karta hai", Some(v2));
            put(s, q, c + 300_000, OM, talk, "popcorn 🍿", None);
            let v3 = put(s, q, c + 330_000, VARUN, talk, "<@2025> <@2026> 2 vs 1 again?", Some(m1));
            put(s, q, c + 400_000, MYRA, talk, "nobody's teaming, you're just wrong", Some(v3));
            let n1 = put(s, q, c + 450_000, NEHA, talk, "<@2024> bro just accept it", None);
            let v4 = put(s, q, c + 500_000, VARUN, talk, "<@2027> you too?? unbelievable", Some(n1));
            put(s, q, c + 560_000, NEHA, talk, "yes me too", Some(v4));
            put_deleted(s, q, c + 600_000, SID, talk, "abe chup kar varun", 30_000, None);
            put_blocked(s, q, c + 620_000, VARUN, talk, "tum sab chutiye ho", "chutiye");
            // Where the fake model misbehaves.
            let flaky = (26, "flaky");
            let f = now - 5 * HOUR;
            let r1 = put(s, q, f, RAHUL, flaky, "<@2023> quiz kab hai", None);
            put(s, q, f + MIN, KAVYA, flaky, "10 baje", Some(r1));
            let down = (27, "down");
            let d = now - 4 * HOUR;
            let r2 = put(s, q, d, RAHUL, down, "<@2023> vc?", None);
            put(s, q, d + MIN, KAVYA, down, "later", Some(r2));
            // Never shown: #safe-corner, and older than the longest period.
            put(s, q, now - 2 * HOUR, NIKHIL, (SAFE, "safe-corner"), "<@2021> SECRET-SAFE we need to talk", None);
            put(s, q, now - 2 * HOUR + MIN, AISHA, (SAFE, "safe-corner"), "SECRET-SAFE ok", None);
            put_blocked(s, q, now - 2 * HOUR + 2 * MIN, NIKHIL, (SAFE, "safe-corner"), "SECRET-SAFE blocked", "x");
            put(s, q, now - 9 * 24 * HOUR, NIKHIL, (24, "music"), "<@2021> ancient history", None);
            FightLog { now, store: parking_lot::Mutex::new(store), scorecard: card.message_id, _dir: dir }
        })
    }

    pub fn authors(people: Vec<u64>, since_ms: i64, until_ms: i64, channel: Option<u64>) -> anyhow::Result<Vec<SaidRow>> {
        Ok(k::authors_between(log().store.lock().conn(), &people, since_ms, until_ms, channel, k::AUTHOR_ROWS)?)
    }

    pub fn channel(channel: u64, since_ms: i64, until_ms: i64) -> anyhow::Result<Vec<SaidRow>> {
        Ok(k::channel_between(log().store.lock().conn(), channel, since_ms, until_ms, k::EXCHANGE_ROWS)?)
    }

    pub fn picture(message: u64, n: usize) -> Option<super::super::super::super::msglog::Picture> {
        let log = log();
        let store = log.store.lock();
        super::super::super::super::msglog::said_file(store.conn(), store.root(), message, n)
    }

    /// Every prompt the fake model has been sent.
    pub static PROMPTS: parking_lot::Mutex<Vec<String>> = parking_lot::Mutex::new(Vec::new());
    /// Calls about #flaky: every third one works.
    static FLAKY: AtomicUsize = AtomicUsize::new(0);

    /// A canned summary of the #desi-banter kalesh; any other stretch gets a
    /// short one with no flags. About #flaky it fails twice in three; about
    /// #down it never answers, the way OpenRouter failed on the live server.
    pub fn summarise(prompt: String) -> anyhow::Result<Reply> {
        let fight = prompt.contains("#desi-banter");
        let flaky = prompt.contains("Channel: #flaky.");
        let down = prompt.contains("Channel: #down.");
        PROMPTS.lock().push(prompt);
        if down {
            anyhow::bail!("HttpError: Http client error: error sending request for url (https://openrouter.ai/api/v1/chat/completions)");
        }
        if flaky && FLAKY.fetch_add(1, Ordering::SeqCst) % 3 < 2 {
            anyhow::bail!("HttpError: Http client error: error sending request for url (https://openrouter.ai/api/v1/chat/completions)");
        }
        let text = if fight {
            serde_json::json!({
                "overview": "Nikhil and Aisha argued in #desi-banter about whether RCB can win the IPL this year. It started as cricket banter and turned personal for a few minutes before both stepped back.",
                "trigger": "Nikhil said RCB would win the cup [#1]; Aisha replied that he says this every year [#2].",
                "timeline": [
                    { "time": "", "what": "Nikhil predicts an RCB title; Aisha teases him for saying it every year.", "refs": [1, 2] },
                    { "time": "", "what": "They trade points about RCB's bowling; Aisha posts a scorecard picture of the 220-run match.", "refs": [3, 4, 5, 6] },
                    { "time": "", "what": "Nikhil asks whether Aisha watches cricket or only memes; Aisha says it has become personal.", "refs": [8, 9] },
                    { "time": "", "what": "Rahul asks both to calm down; Kavya agrees with Aisha.", "refs": [10, 13] },
                    { "time": "", "what": "Nikhil brings up Aisha's college in Pune and offers to tell everyone [#18]. A message of his right after was deleted by a moderator three minutes later [#19], and AutoMod blocked another before anyone saw it [#21]. Aisha asks him to stop [#20].", "refs": [18, 19, 20, 21] },
                    { "time": "", "what": "Rahul tells Nikhil it is too much. Aisha mutes the channel; Nikhil later apologises for the college remark.", "refs": [22, 24, 26] }
                ],
                "positions": [
                    { "who": "Nikhil", "points": ["RCB's bowling is better this year, so one bad match shouldn't count.", "He felt Aisha's opening remark was the first jab."] },
                    { "who": "Aisha", "points": ["Results and stats matter more than hope.", "Being called a memes-only fan was the personal part."] }
                ],
                "others": "Rahul tried to calm it twice and told Nikhil to drop the college remark [#22]. Kavya agreed with Aisha about RCB fans [#13].",
                "ending": { "state": "fizzled", "what": "Nikhil apologised for the college remark [#26]; Aisha accepted but kept her view on RCB [#27], and it ended in banter [#28]." },
                "interpretation": ["It reads mostly as heated banter; the college remark [#18] seems to be the point it stopped being fun for Aisha."],
                "flags": [
                    { "kind": "personal_info", "who": "Nikhil", "message": 18, "what": "Brought up Aisha's college and offered to share it with everyone; she asked him to stop [#20]." },
                    { "kind": "slur", "who": "Nikhil", "message": 21, "what": "Tried to call Aisha a slur; AutoMod blocked it, so nobody in the channel saw it." }
                ]
            })
            .to_string()
        } else {
            serde_json::json!({
                "overview": "A short, friendly exchange.",
                "trigger": "Small talk [#1].",
                "timeline": [{ "time": "", "what": "Friendly back and forth.", "refs": [1, 2] }],
                "positions": [],
                "others": "No one else joined in.",
                "ending": { "state": "resolved", "what": "It ended on good terms." },
                "interpretation": [],
                "flags": []
            })
            .to_string()
        };
        let input_tokens = super::super::super::super::notes_build::estimate_tokens(PROMPTS.lock().last().map(String::as_str).unwrap_or("")) as u64;
        Ok(Reply { output_tokens: super::super::super::super::notes_build::estimate_tokens(&text) as u64, text, input_tokens, model: "fake/main-model".into() })
    }
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    use super::super::tests::{ADMIN, ADMIN_TWO, MEMBER, panel, session_for};
    use super::fake::{AISHA, KAVYA, MYRA, NEHA, NIKHIL, OM, PROMPTS, RAHUL, SAFE, SID, VARUN};
    use super::*;

    async fn call(app: &Router, method: &str, path: &str, session: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(path).header("x-panel", "1");
        if let Some(s) = session {
            req = req.header("cookie", format!("mlci_panel={}", s));
        }
        let req = match body {
            Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
            None => req.body(Body::empty()).unwrap(),
        };
        let res = app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 10 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    fn ids(people: &[u64]) -> String {
        people.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(",")
    }

    fn stretch_query(s: &Value, people: &[u64]) -> String {
        format!("people={}&channel={}&start={}&end={}", ids(people), s["channel"]["id"].as_str().unwrap(), s["start_ms"].as_str().unwrap(), s["end_ms"].as_str().unwrap())
    }

    fn stretch_body(s: &Value, people: &[u64]) -> Value {
        json!({ "people": ids(people), "channel": s["channel"]["id"], "start": s["start_ms"], "end": s["end_ms"] })
    }

    async fn find(app: &Router, session: &str, query: &str) -> (StatusCode, Value) {
        call(app, "GET", &format!("/api/kalesh/find?{}", query), Some(session), None).await
    }

    fn channels_of(found: &Value) -> Vec<String> {
        found["stretches"].as_array().unwrap().iter().map(|s| s["channel"]["name"].as_str().unwrap().to_string()).collect()
    }

    fn stretch_in(found: &Value, channel: &str) -> Value {
        found["stretches"].as_array().unwrap().iter().find(|s| s["channel"]["name"] == channel).unwrap_or_else(|| panic!("no stretch in #{channel}: {found}")).clone()
    }

    #[tokio::test]
    async fn two_members_stretches_are_found_with_counts_and_bystanders() {
        let app = panel();
        let session = session_for(ADMIN);
        let (status, found) = find(&app, &session, &format!("a={NIKHIL}&b={AISHA}")).await;
        assert_eq!(status, StatusCode::OK, "{found}");
        assert_eq!(found["label"], "last 24 hours");
        // Newest first; #safe-corner never, the three-day-old one not in a day.
        assert_eq!(channels_of(&found), vec!["general", "desi-banter"], "{found}");
        let fight = &found["stretches"][1];
        assert_eq!(fight["messages"], 28, "{fight}");
        assert_eq!((fight["per_person"].clone(), fight["others"].as_u64()), (json!([13, 11]), Some(4)));
        // Replies to a bystander don't count as replies to each other.
        assert_eq!(fight["replies_between"], json!([{ "from": "a", "to": "b", "count": 7 }, { "from": "b", "to": "a", "count": 10 }]), "{fight}");
        assert_eq!(fight["replies"], 17);
        assert_eq!(fight["mentions"], 1);
        assert_eq!((fight["deleted"].as_u64(), fight["blocked"].as_u64()), (Some(1), Some(1)), "the deleted and the blocked message are in it");
        let people: Vec<(&str, u64)> = fight["bystanders"].as_array().unwrap().iter().map(|p| (p["id"].as_str().unwrap(), p["messages"].as_u64().unwrap())).collect();
        assert_eq!(people, vec![(RAHUL.to_string().as_str(), 2), (KAVYA.to_string().as_str(), 2)], "Rahul's opening question came before the fight");
        // The #general one is only nearness: no replies, no mentions.
        let near = &found["stretches"][0];
        assert_eq!((near["nearby"].as_u64(), near["replies"].as_u64(), near["mentions"].as_u64()), (Some(4), Some(0), Some(0)));
        assert!(!found.to_string().contains("SECRET-SAFE"));
        // The whole period, all stretches together, is offered as one thing to summarise.
        let period = &found["period"];
        assert_eq!((period["stretches"].as_u64(), period["messages"].as_u64()), (Some(2), Some(32)), "{period}");
        assert_eq!((period["deleted"].as_u64(), period["blocked"].as_u64()), (Some(1), Some(1)));
        assert!(period["summarised"].is_boolean(), "another test may have summarised it already");
        let named: Vec<(&str, &str, &str)> = found["people"].as_array().unwrap().iter().map(|p| (p["id"].as_str().unwrap(), p["name"].as_str().unwrap(), p["letter"].as_str().unwrap())).collect();
        assert_eq!(named, vec![(NIKHIL.to_string().as_str(), "Nikhil", "a"), (AISHA.to_string().as_str(), "Aisha", "b")]);
        assert!(found["people"][0]["avatar"].is_string(), "each person's picture, from the member cache");
    }

    #[tokio::test]
    async fn the_exchange_shows_everyone_in_order_with_deleted_and_blocked_messages_marked() {
        let app = panel();
        let session = session_for(ADMIN);
        let (_, found) = find(&app, &session, &format!("people={NIKHIL},{AISHA}")).await;
        let fight = stretch_in(&found, "desi-banter");
        let (status, ex) = call(&app, "GET", &format!("/api/kalesh/exchange?{}", stretch_query(&fight, &[NIKHIL, AISHA])), Some(&session), None).await;
        assert_eq!(status, StatusCode::OK, "{ex}");
        let msgs = ex["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 28);
        assert_eq!(msgs[0]["n"], 1);
        assert!(msgs.windows(2).all(|w| w[0]["ts_ms"].as_i64() <= w[1]["ts_ms"].as_i64()), "in order");
        let by = |text: &str| msgs.iter().find(|m| m["text"].as_str().unwrap().starts_with(text)).unwrap_or_else(|| panic!("no {text}")).clone();
        assert_eq!(by("RCB is winning")["side"], "a");
        assert_eq!(by("bhai har saal")["side"], "b");
        assert_eq!(by("bhai har saal")["towards"], "a");
        let kavya = by("aisha is right");
        assert_eq!((kavya["side"].as_str(), kavya["towards"].as_str()), (Some("other"), Some("a")), "Kavya replies to Nikhil");
        let rahul = by("nikhil bhai that's too much");
        assert_eq!((rahul["side"].as_str(), rahul["towards"].as_str()), (Some("other"), Some("a")));
        assert_eq!(by("guys chill")["towards"], Value::Null);
        // Every message still in Discord links to it; replies say which number they answer.
        assert!(msgs.iter().filter(|m| m["gone"].is_null()).all(|m| m["url"].as_str().is_some_and(|u| u.starts_with("https://discord.com/channels/900/23/"))));
        assert_eq!(by("bhai har saal")["reply_to"]["n"], 1);
        assert_eq!(ex["will_trim"], false);
        // The deleted and the blocked message sit where they were said, right after the college one.
        let college = by("tu Pune wale college");
        let deleted = by("sab ko pata hai");
        let blocked = by("tu ekdum chutiya");
        assert_eq!((college["n"].as_u64(), deleted["n"].as_u64(), blocked["n"].as_u64()), (Some(18), Some(19), Some(21)));
        assert_eq!(deleted["gone"]["kind"], "deleted");
        assert_eq!(deleted["gone"]["by"]["name"], "Meera");
        assert_eq!(deleted["gone"]["deleted_ms"].as_i64().unwrap() - deleted["ts_ms"].as_i64().unwrap(), 3 * 60_000, "deleted three minutes later");
        assert_eq!((deleted["side"].as_str(), deleted["url"].is_null()), (Some("a"), true), "nothing to jump to");
        assert_eq!((blocked["gone"]["kind"].as_str(), blocked["gone"]["rule"].as_str(), blocked["gone"]["keyword"].as_str()), (Some("blocked"), Some("Block slurs"), Some("*chutiya*")));
        assert_eq!(blocked["channel"]["name"], "desi-banter", "filed where he tried to post, not the log channel");
        assert_eq!((ex["deleted"].as_u64(), ex["blocked"].as_u64()), (Some(1), Some(1)));
        // A picture posted in it comes with the message, served by the panel.
        let card = by("see for yourself");
        assert_eq!(card["images"][0]["name"], "scorecard.png");
        let url = card["images"][0]["url"].as_str().unwrap().to_string();
        let res = app.clone().oneshot(Request::builder().uri(&url).header("cookie", format!("mlci_panel={session}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{url}");
        assert_eq!(res.headers()["content-type"], "image/png");
        let res = app.clone().oneshot(Request::builder().uri(&url).header("cookie", format!("mlci_panel={}", session_for(MEMBER))).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "admins only");
        // #safe-corner is never read, even asked for directly.
        let bad = format!("people={NIKHIL},{AISHA}&channel={SAFE}&start=1&end={}", chrono::Utc::now().timestamp_millis());
        let (status, _) = call(&app, "GET", &format!("/api/kalesh/exchange?{}", bad), Some(&session), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn three_and_four_people_are_found_together_each_with_their_own_side() {
        let app = panel();
        let session = session_for(ADMIN);
        let (status, three) = find(&app, &session, &format!("people={VARUN},{MYRA},{SID}")).await;
        assert_eq!(status, StatusCode::OK, "{three}");
        let s3 = stretch_in(&three, "cricket-talk");
        let per: Vec<u64> = s3["per_person"].as_array().unwrap().iter().map(|n| n.as_u64().unwrap()).collect();
        assert_eq!(per.len(), 3);
        assert!(per.iter().all(|n| *n >= 2), "{s3}");
        // Varun → Siddharth and Siddharth → Varun, Myra → Varun…
        let pairs: Vec<(String, String)> = s3["replies_between"].as_array().unwrap().iter().map(|r| (r["from"].as_str().unwrap().into(), r["to"].as_str().unwrap().into())).collect();
        assert!(pairs.contains(&("a".into(), "c".into())) && pairs.contains(&("c".into(), "a".into())) && pairs.contains(&("b".into(), "a".into())), "{pairs:?}");
        let names: Vec<&str> = s3["bystanders"].as_array().unwrap().iter().map(|b| b["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"Neha") && names.contains(&"Om"), "Neha wasn't picked, so she is a bystander: {names:?}");
        let (_, ex3) = call(&app, "GET", &format!("/api/kalesh/exchange?{}", stretch_query(&s3, &[VARUN, MYRA, SID])), Some(&session), None).await;
        let side_of = |ex: &Value, who: u64| ex["messages"].as_array().unwrap().iter().filter(|m| m["member"]["id"] == who.to_string()).map(|m| m["side"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        assert!(side_of(&ex3, SID).iter().all(|s| s == "c"));
        assert!(side_of(&ex3, NEHA).iter().all(|s| s == "other"));
        // The deleted one is Siddharth's own, with nobody on record: probably by himself.
        let del = ex3["messages"].as_array().unwrap().iter().find(|m| m["gone"]["kind"] == "deleted").unwrap().clone();
        assert_eq!((del["gone"]["checked"].as_bool(), del["gone"]["by"].is_null(), del["side"].as_str()), (Some(true), true, Some("c")));

        let (status, four) = find(&app, &session, &format!("people={VARUN},{MYRA},{SID},{NEHA}")).await;
        assert_eq!(status, StatusCode::OK, "{four}");
        let s4 = stretch_in(&four, "cricket-talk");
        assert_eq!(s4["per_person"].as_array().unwrap().len(), 4);
        assert!(s4["per_person"][3].as_u64().unwrap() >= 2, "Neha is one of them now");
        assert_eq!(four["people"][3]["letter"], "d");
        let (_, ex4) = call(&app, "GET", &format!("/api/kalesh/exchange?{}", stretch_query(&s4, &[VARUN, MYRA, SID, NEHA])), Some(&session), None).await;
        assert!(side_of(&ex4, NEHA).iter().all(|s| s == "d"));
        assert!(side_of(&ex4, OM).iter().all(|s| s == "other"));
        // The model is told who is who, one position each.
        let (status, got) = call(&app, "POST", "/api/kalesh/summarise", Some(&session), Some(stretch_body(&s4, &[VARUN, MYRA, SID, NEHA]))).await;
        assert_eq!(status, StatusCode::OK, "{got}");
        assert_eq!(got["summary"]["people"].as_array().unwrap().len(), 4);
        let prompt = PROMPTS.lock().iter().rev().find(|p| p.contains("Channel: #cricket-talk.")).cloned().unwrap();
        assert!(prompt.contains("The 4 people: A = Varun, B = Myra, C = Siddharth, D = Neha."), "{prompt}");
        assert!(prompt.contains("Others who spoke: Om."), "{prompt}");
        assert!(prompt.contains("one entry for each person named below"));
        assert!(prompt.contains("Siddharth (C) [DELETED under a minute later, probably by the author themselves]: abe chup kar varun"), "{prompt}");
        assert!(prompt.contains("Varun (A) [BLOCKED BY AUTOMOD, rule \"Block slurs\" - never shown in the channel]: tum sab chutiye ho"), "{prompt}");
        // Too many, too few, or the same one twice.
        for bad in [format!("people={VARUN}"), format!("people={VARUN},{VARUN}"), format!("people=1,2,3,4,5,6,7"), "people=x,y".into()] {
            let (status, body) = find(&app, &session, &bad).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
        }
    }

    #[tokio::test]
    async fn the_period_is_a_day_by_default_and_a_week_at_most() {
        let app = panel();
        let session = session_for(ADMIN);
        let (status, week) = find(&app, &session, &format!("a={NIKHIL}&b={AISHA}&hours=168")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(channels_of(&week), vec!["general", "desi-banter", "memes"], "nine days back is out of reach");
        assert_eq!(week["label"], "last 7 days");
        let now = chrono::Utc::now().timestamp();
        let custom = format!("a={NIKHIL}&b={AISHA}&from={}&to={}", now - 4 * 86_400, now - 2 * 86_400);
        let (status, got) = find(&app, &session, &custom).await;
        assert_eq!(status, StatusCode::OK, "{got}");
        assert_eq!(channels_of(&got), vec!["memes"]);
        for bad in [
            format!("a={NIKHIL}&b={AISHA}&hours=169"),
            format!("a={NIKHIL}&b={AISHA}&hours=0"),
            format!("a={NIKHIL}&b={AISHA}&from={}&to={}", now - 8 * 86_400, now),
            format!("a={NIKHIL}&b={AISHA}&from={}&to={}", now, now - 3600),
            format!("a={NIKHIL}&b={AISHA}&from={}", now - 3600),
            format!("a={NIKHIL}&b={NIKHIL}"),
            format!("a={NIKHIL}"),
            "a=x&b=y".to_string(),
        ] {
            let (status, body) = find(&app, &session, &bad).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
            assert!(body["error"].is_string());
        }
        assert!(period(&None, &None, &None, 10 * 86_400_000).unwrap().since_ms == 9 * 86_400_000);
    }

    #[tokio::test]
    async fn a_summary_is_kept_and_reused_rather_than_run_again() {
        let app = panel();
        let session = session_for(ADMIN);
        let (_, found) = find(&app, &session, &format!("a={AISHA}&b={NIKHIL}")).await;
        let near = found["stretches"][0].clone();
        assert_eq!(near["channel"]["name"], "general");
        let asked = || PROMPTS.lock().iter().filter(|p| p.contains("Channel: #general.")).count();
        let before = asked();
        let (status, first) = call(&app, "POST", "/api/kalesh/summarise", Some(&session), Some(stretch_body(&near, &[AISHA, NIKHIL]))).await;
        assert_eq!(status, StatusCode::OK, "{first}");
        assert_eq!(first["reused"], false);
        assert_eq!(asked(), before + 1);
        let s = &first["summary"];
        assert_eq!(s["run_by"]["id"], ADMIN.to_string());
        assert_eq!(s["model"], "fake/main-model");
        assert_eq!(s["scope"], "stretch");
        assert!(s["input_tokens"].as_u64().unwrap() > 500, "the prompt's tokens are counted: {s}");
        assert_eq!((s["trimmed"].as_bool(), s["message_count"].as_u64()), (Some(false), Some(4)));
        assert_eq!(s["summary"]["flags"], json!([]));
        // Again, by another admin and the other way round: the stored one, no new call.
        let (status, again) = call(&app, "POST", "/api/kalesh/summarise", Some(&session_for(ADMIN_TWO)), Some(stretch_body(&near, &[NIKHIL, AISHA]))).await;
        assert_eq!(status, StatusCode::OK, "{again}");
        assert_eq!(again["reused"], true);
        assert_eq!(again["summary"]["id"], s["id"]);
        assert_eq!(asked(), before + 1, "running it twice doesn't pay twice");
        // The stretch now says it has one, and the overview lists it.
        let (_, found) = find(&app, &session, &format!("a={AISHA}&b={NIKHIL}")).await;
        assert_eq!(found["stretches"][0]["summarised"], true);
        let (_, ex) = call(&app, "GET", &format!("/api/kalesh/exchange?{}", stretch_query(&near, &[NIKHIL, AISHA])), Some(&session), None).await;
        assert_eq!(ex["summaries"][0]["id"], s["id"]);
        assert_eq!(ex["summaries"][0]["current"], true);
        let (_, overview) = call(&app, "GET", "/api/kalesh", Some(&session), None).await;
        assert!(overview["summaries"].as_array().unwrap().iter().any(|x| x["id"] == s["id"]), "{overview}");
    }

    #[tokio::test]
    async fn the_fight_is_summarised_with_its_flags_and_the_model_reads_the_markers() {
        let app = panel();
        let session = session_for(ADMIN_TWO);
        let (_, found) = find(&app, &session, &format!("a={NIKHIL}&b={AISHA}")).await;
        let fight = stretch_in(&found, "desi-banter");
        let (status, got) = call(&app, "POST", "/api/kalesh/summarise", Some(&session), Some(stretch_body(&fight, &[NIKHIL, AISHA]))).await;
        assert_eq!(status, StatusCode::OK, "{got}");
        let s = &got["summary"];
        assert_eq!(s["summary"]["flags"][0]["kind"], "personal_info");
        assert_eq!(s["summary"]["flags"][0]["message"], 18);
        assert_eq!(s["summary"]["flags"][1]["kind"], "slur");
        // [#18] is the college message and [#21] the blocked one: the page can link both.
        let (_, ex) = call(&app, "GET", &format!("/api/kalesh/exchange?{}", stretch_query(&fight, &[NIKHIL, AISHA])), Some(&session), None).await;
        let ids = s["message_ids"].as_array().unwrap();
        let msg = |n: usize| ex["messages"].as_array().unwrap().iter().find(|m| m["id"] == ids[n - 1]).unwrap().clone();
        assert!(msg(18)["text"].as_str().unwrap().contains("college"));
        assert_eq!(msg(21)["gone"]["kind"], "blocked");
        let prompt = PROMPTS.lock().iter().rev().find(|p| p.contains("Channel: #desi-banter.")).cloned().unwrap();
        assert!(prompt.contains("The two people: A = Nikhil, B = Aisha.") && prompt.contains("Others who spoke: Rahul, Kavya."), "{prompt}");
        assert!(prompt.contains("@Aisha naam bata"), "mentions read as names");
        // The markers, what they mean, and the picture by name.
        assert!(prompt.contains("#19 [") && prompt.contains("Nikhil (A) [DELETED 3 min later by Meera (a moderator)]: sab ko pata hai"), "{prompt}");
        assert!(prompt.contains("#21 [") && prompt.contains("Nikhil (A) [BLOCKED BY AUTOMOD, rule \"Block slurs\" - never shown in the channel]: tu ekdum chutiya hai"), "{prompt}");
        assert!(prompt.contains("Of these, 1 were deleted later and 1 were blocked by AutoMod"));
        assert!(prompt.contains("nobody there saw it, so nobody replied to it"), "the model is told what blocked means");
        assert!(prompt.contains("posted and seen in the channel, and removed later"), "and what deleted means");
        assert!(prompt.contains("see for yourself [image: scorecard.png]"), "{prompt}");
    }

    #[tokio::test]
    async fn the_whole_period_is_summarised_together_and_kept() {
        let app = panel();
        let session = session_for(ADMIN);
        // The default view: the last 24 hours, nothing else chosen.
        let (status, found) = find(&app, &session, &format!("people={NIKHIL},{AISHA}")).await;
        assert_eq!(status, StatusCode::OK);
        let asked = || PROMPTS.lock().iter().filter(|p| p.contains("separate stretches, put together in time order")).count();
        let before = asked();
        let body = json!({ "people": ids(&[NIKHIL, AISHA]), "scope": "period", "from": found["since_ts"].to_string(), "to": found["until_ts"].to_string() });
        let (status, got) = call(&app, "POST", "/api/kalesh/summarise", Some(&session), Some(body.clone())).await;
        assert_eq!(status, StatusCode::OK, "{got}");
        assert_eq!(asked(), before + 1);
        let s = &got["summary"];
        assert_eq!((s["scope"].as_str(), s["channel"].is_null(), s["message_count"].as_u64()), (Some("period"), true, Some(32)));
        let prompt = PROMPTS.lock().iter().rev().find(|p| p.contains("separate stretches")).cloned().unwrap();
        assert!(prompt.contains("2 separate stretches") && prompt.contains("Channels: #desi-banter, #general."), "{prompt}");
        assert!(prompt.contains("== in #desi-banter ==") && prompt.contains("== in #general =="), "each message says where it was");
        assert!(prompt.contains("[DELETED 3 min later by Meera (a moderator)]") && prompt.contains("[BLOCKED BY AUTOMOD"));
        let (a, b) = (prompt.find("RCB is winning").unwrap(), prompt.find("who is up for quiz").unwrap());
        assert!(a < b, "in time order");
        // Asked again: the stored one.
        let (_, again) = call(&app, "POST", "/api/kalesh/summarise", Some(&session_for(ADMIN_TWO)), Some(body)).await;
        assert_eq!((again["reused"].as_bool(), again["summary"]["id"].clone()), (Some(true), s["id"].clone()));
        assert_eq!(asked(), before + 1);
        // The period view shows it as the current summary, with every message.
        let q = format!("people={}&from={}&to={}", ids(&[NIKHIL, AISHA]), found["since_ts"], found["until_ts"]);
        let (status, view) = call(&app, "GET", &format!("/api/kalesh/period?{q}"), Some(&session), None).await;
        assert_eq!(status, StatusCode::OK, "{view}");
        assert_eq!((view["scope"].as_str(), view["count"].as_u64()), (Some("period"), Some(32)));
        assert_eq!(view["stretches"].as_array().unwrap().len(), 2);
        assert_eq!(view["summaries"][0]["id"], s["id"]);
        assert_eq!(view["summaries"][0]["current"], true);
        assert!(view["messages"].as_array().unwrap().iter().all(|m| m["channel"]["name"].is_string()), "each message carries its channel");
        let (_, found) = find(&app, &session, &q).await;
        assert_eq!(found["period"]["summarised"], true);
    }

    #[tokio::test]
    async fn a_model_that_fails_once_or_twice_is_tried_again() {
        let app = panel();
        let session = session_for(ADMIN);
        let (_, found) = find(&app, &session, &format!("people={RAHUL},{KAVYA}")).await;
        let flaky = stretch_in(&found, "flaky");
        let tries = || PROMPTS.lock().iter().filter(|p| p.contains("Channel: #flaky.")).count();
        let before = tries();
        let (status, got) = call(&app, "POST", "/api/kalesh/summarise", Some(&session), Some(stretch_body(&flaky, &[RAHUL, KAVYA]))).await;
        assert_eq!(status, StatusCode::OK, "{got}");
        assert_eq!(got["reused"], false);
        assert!(got["summary"]["summary"]["overview"].is_string());
        assert_eq!(tries(), before + 3, "two failures, then an answer");
    }

    /// The owner pressed Summarise; OpenRouter failed to answer; the page showed
    /// nothing. A failure now reaches the page as a reason in words, and nothing is saved.
    #[tokio::test]
    async fn a_model_that_never_answers_reaches_the_page_as_a_message_not_an_empty_result() {
        let app = panel();
        let session = session_for(ADMIN);
        let (_, found) = find(&app, &session, &format!("people={RAHUL},{KAVYA}")).await;
        let down = stretch_in(&found, "down");
        let tries = || PROMPTS.lock().iter().filter(|p| p.contains("Channel: #down.")).count();
        let before = tries();
        let (status, got) = call(&app, "POST", "/api/kalesh/summarise", Some(&session), Some(stretch_body(&down, &[RAHUL, KAVYA]))).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{got}");
        let error = got["error"].as_str().expect("an error message the page shows");
        assert!(error.contains("didn't answer (network error)") && error.contains("Try again"), "{error}");
        assert!(got.get("summary").is_none());
        assert_eq!(tries(), before + TRIES, "tried {TRIES} times");
        let (_, found) = find(&app, &session, &format!("people={RAHUL},{KAVYA}")).await;
        assert_eq!(stretch_in(&found, "down")["summarised"], false, "nothing was saved");
        assert_eq!(why_failed("the model took over 240s"), "it took too long to answer");
    }

    #[tokio::test]
    async fn every_summary_is_in_the_activity_log_once_per_quarter_hour() {
        let app = panel();
        let session = session_for(ADMIN);
        let (_, found) = find(&app, &session, &format!("a={NIKHIL}&b={AISHA}&hours=168")).await;
        let memes = stretch_in(&found, "memes");
        for _ in 0..2 {
            let (status, _) = call(&app, "POST", "/api/kalesh/summarise", Some(&session), Some(stretch_body(&memes, &[NIKHIL, AISHA]))).await;
            assert_eq!(status, StatusCode::OK);
        }
        let (_, audit) = call(&app, "GET", "/api/audit?limit=1000", Some(&session), None).await;
        let entries: Vec<&Value> = audit
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["key"] == "kalesh:summary" && e["change"].as_str().is_some_and(|c| c.starts_with("@Nikhil vs @Aisha · #memes · ")))
            .collect();
        assert_eq!(entries.len(), 1, "{audit}");
        assert_eq!(entries[0]["label"], "Summarised a kalesh");
        assert_eq!(entries[0]["section"]["id"], "kalesh");
        assert_eq!(entries[0]["user_id"], ADMIN.to_string());
        // Looking for them is logged too, with who and when.
        assert!(audit.as_array().unwrap().iter().any(|e| e["key"] == "kalesh:find" && e["change"] == "@Nikhil vs @Aisha · last 7 days"), "{audit}");
    }

    #[tokio::test]
    async fn detections_are_listed_newest_first_and_open_their_stretch() {
        let app = panel();
        let session = session_for(ADMIN);
        let log = super::fake::log();
        let rows: Vec<_> = super::fake::channel(23, log.now - 21 * HOUR_MS, log.now).unwrap().into_iter().filter(|r| r.gone.is_none() && r.attachments.is_empty()).collect();
        let window: Vec<k::Seen> = rows[9..21].iter().map(|r| k::Seen { id: r.message_id, ts_ms: r.created_ms, author: r.author_id, author_name: r.author_name.clone() }).collect();
        let d = k::detection_of(23, &window, "kalesh alert 🍿", chrono::Utc::now().timestamp()).unwrap();
        let hidden = k::detection_of(SAFE, &window, "x", chrono::Utc::now().timestamp()).unwrap();
        let (id, _) = {
            let conn = store::db().unwrap().lock();
            (store::add_detection(&conn, &d).unwrap(), store::add_detection(&conn, &hidden).unwrap())
        };
        let (status, overview) = call(&app, "GET", "/api/kalesh", Some(&session), None).await;
        assert_eq!(status, StatusCode::OK, "{overview}");
        let listed = overview["detections"].as_array().unwrap();
        let mine = listed.iter().find(|x| x["id"] == id).expect("listed");
        assert_eq!(mine["channel"]["name"], "desi-banter");
        assert_eq!(mine["participants"][0]["id"], NIKHIL.to_string(), "busiest first");
        assert_eq!(mine["message_count"], 12);
        assert!(listed.iter().all(|x| x["channel"]["id"] != SAFE.to_string()), "#safe-corner is never listed");
        assert!(listed.windows(2).all(|w| w[0]["start_ts"].as_i64() >= w[1]["start_ts"].as_i64()), "newest first");
        // Opening it widens the burst to the whole stretch, with everyone who was really in it.
        let (status, open) = call(&app, "GET", &format!("/api/kalesh/detections/{}", id), Some(&session), None).await;
        assert_eq!(status, StatusCode::OK, "{open}");
        let people: Vec<&str> = open["people"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap()).collect();
        assert_eq!(&people[..2], &[NIKHIL.to_string().as_str(), AISHA.to_string().as_str()]);
        let q = format!("people={}&channel=23&start={}&end={}&detection={}", people.join(","), open["start_ms"].as_str().unwrap(), open["end_ms"].as_str().unwrap(), id);
        let (status, ex) = call(&app, "GET", &format!("/api/kalesh/exchange?{}", q), Some(&session), None).await;
        assert_eq!(status, StatusCode::OK, "{ex}");
        assert!(ex["messages"].as_array().unwrap().len() >= 28, "the whole stretch, not only the burst");
        assert_eq!(ex["messages"].as_array().unwrap().iter().filter(|m| m["in_detection"] == true).count(), 12);
        assert_eq!(ex["detection_id"], id);
    }

    #[tokio::test]
    async fn the_kalesh_page_is_for_admins_only() {
        let app = panel();
        let member = session_for(MEMBER);
        for (method, path) in [
            ("GET", "/api/kalesh"),
            ("GET", "/api/kalesh/find?a=2020&b=2021"),
            ("GET", "/api/kalesh/exchange?a=2020&b=2021&channel=23&start=1&end=2"),
            ("GET", "/api/kalesh/period?people=2020,2021"),
            ("GET", "/api/kalesh/detections/1"),
            ("GET", "/api/kalesh/picture/1/0"),
        ] {
            let (status, _) = call(&app, method, path, None, None).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
            let (status, _) = call(&app, method, path, Some(&member), None).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
        }
        let before = PROMPTS.lock().len();
        let body = json!({ "a": "2020", "b": "2021", "channel": "21", "start": "1", "end": "2" });
        let (status, _) = call(&app, "POST", "/api/kalesh/summarise", Some(&member), Some(body.clone())).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = call(&app, "POST", "/api/kalesh/summarise", None, Some(body)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(PROMPTS.lock().len(), before, "no model call for a non-admin");
    }
}
