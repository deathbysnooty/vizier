//! The Kalesh page: looking back at a fight between two members.
//!
//! Three ways in. The fights the live detector called, newest first, so a mod
//! can open one without knowing who fought. A search for two named members
//! over a period (the last day by default, a week at most), which lists the
//! stretches where they were going at each other. And past summaries.
//!
//! A stretch always shows the raw exchange, in order, bystanders included and
//! marked, with a jump link on every message. The summary is written only when
//! a mod presses Summarise, by the bot's main model unless another is set; it
//! is kept, so the same stretch is never paid for twice, and every summary (and
//! every look) is in the activity log, once per fifteen minutes for the same one.
//!
//! Admins only, like every panel page. Everything is read from `msglog`'s copy
//! of every channel except #safe-corner and the skip list.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::super::kalesh::{self as k, Line, Side};
use super::super::super::kalesh_store::{self as store, Summary};
use super::super::super::msglog::{self, Place, SaidRow};
use super::super::super::weekly;
use super::msglog::{channel_json, member_json, never_shown, unavailable};
use super::{ApiError, ApiResult, Caller, Panel, ok, parse_id, search};

/// Detections and summaries the overview lists.
pub const LIST_LIMIT: usize = 50;
/// Stretches one search lists at most.
pub const MAX_STRETCHES: usize = 60;
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

/// The two members asked about: both given, both ids, not the same person.
fn pair(a: &Option<String>, b: &Option<String>) -> Result<(u64, u64), ApiError> {
    let one = |v: &Option<String>| blank(v).and_then(|raw| parse_id(&raw));
    let (Some(a), Some(b)) = (one(a), one(b)) else {
        return Err(ApiError::bad("Pick both members."));
    };
    if a == b {
        return Err(ApiError::bad("Pick two different members."));
    }
    Ok((a, b))
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
    if let Some(r) = rows.iter().rev().find(|r| r.author_id == id) {
        return r.author_name.clone();
    }
    panel.data.member(id).await.map(|m| m.name).unwrap_or_else(|| id.to_string())
}

fn who(panel: &Panel, id: u64, fallback: &str) -> Value {
    json!({ "id": id.to_string(), "name": panel.cached_name(id).unwrap_or_else(|| if fallback.is_empty() { id.to_string() } else { fallback.to_string() }) })
}

fn channel_name(channel: &Value) -> String {
    channel["name"].as_str().unwrap_or("").to_string()
}

// --- the overview -------------------------------------------------------------------------------

fn summary_json(panel: &Panel, s: &Summary, current_key: Option<&str>, sensitive: &[u64]) -> Option<Value> {
    let n = &s.new;
    let channel = channel_by_id(panel, n.channel_id, sensitive)?;
    Some(json!({
        "id": s.id,
        "key": n.stretch_key,
        "current": current_key.is_none_or(|k| k == n.stretch_key),
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
            "log_on": msglog::enabled(),
        },
    }))
}

// --- finding a fight between two members ----------------------------------------------------------

#[derive(Deserialize)]
pub struct FindQuery {
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

/// Everyone else who spoke in a stretch, busiest first.
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

pub async fn find(State(panel): State<Panel>, axum::Extension(Caller(user)): axum::Extension<Caller>, Query(q): Query<FindQuery>) -> ApiResult {
    let (a, b) = pair(&q.a, &q.b)?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let per = period(&q.hours, &q.from, &q.to, now_ms)?;
    let sensitive = never_shown(&panel);
    let mut rows = panel.data.kalesh_authors(a, b, per.since_ms, per.until_ms, None).await.map_err(unavailable)?;
    rows.retain(|r| place_json(&panel, r, &sensitive).is_some());
    let (a_name, b_name) = (name_of(&panel, a, &rows).await, name_of(&panel, b, &rows).await);
    search::log_quietly("kalesh:find", user, &format!("@{} vs @{} · {}", a_name, b_name, per.label));

    let found = k::find_stretches(&rows, a, b);
    let more = found.len() > MAX_STRETCHES;
    let mut out = Vec::new();
    let mut keys = Vec::new();
    for s in found.into_iter().take(MAX_STRETCHES) {
        let mut all = panel.data.kalesh_channel(s.channel_id, s.start_ms, s.end_ms).await.map_err(unavailable)?;
        all.retain(|r| place_json(&panel, r, &sensitive).is_some());
        let lines = k::exchange(&all, a, b);
        let Some(first) = all.first() else { continue };
        let Some(channel) = place_json(&panel, first, &sensitive) else { continue };
        let key = k::stretch_key(a, b, s.channel_id, &lines);
        keys.push(key.clone());
        out.push(json!({
            "key": key,
            "channel": channel,
            "start_ms": s.start_ms.to_string(),
            "end_ms": s.end_ms.to_string(),
            "start_ts": s.start_ms / 1000,
            "end_ts": s.end_ms / 1000,
            "messages": lines.len(),
            "a_messages": lines.iter().filter(|l| l.side == Side::A).count(),
            "b_messages": lines.iter().filter(|l| l.side == Side::B).count(),
            "others": lines.iter().filter(|l| l.side == Side::Other).count(),
            "bystanders": bystanders(&panel, &lines),
            "replies_ab": s.replies_ab,
            "replies_ba": s.replies_ba,
            "mentions": s.mentions,
            "nearby": s.nearby,
        }));
    }
    let summarised: HashSet<String> = match store::db() {
        Some(db) => store::summarised_keys(&db.lock(), &keys).map_err(db_error)?.into_iter().collect(),
        None => HashSet::new(),
    };
    for s in &mut out {
        let done = s["key"].as_str().is_some_and(|k| summarised.contains(k));
        s["summarised"] = json!(done);
    }
    ok(json!({
        "a": { "id": a.to_string(), "name": a_name },
        "b": { "id": b.to_string(), "name": b_name },
        "since_ts": per.since_ms / 1000,
        "until_ts": per.until_ms / 1000,
        "label": per.label,
        "stretches": out,
        "more": more,
        "read": rows.len(),
        "log_on": msglog::enabled(),
    }))
}

// --- one stretch, message by message ---------------------------------------------------------------

#[derive(Deserialize)]
pub struct ExchangeQuery {
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
}

struct Loaded {
    a: u64,
    b: u64,
    a_name: String,
    b_name: String,
    channel: u64,
    channel_json: Value,
    start_ms: i64,
    end_ms: i64,
    rows: Vec<SaidRow>,
    truncated: bool,
    detection: Option<store::Detection>,
}

impl Loaded {
    fn lines(&self) -> Vec<Line<'_>> {
        k::exchange(&self.rows, self.a, self.b)
    }
    fn label(&self) -> String {
        format!("@{} vs @{} · #{} · {}", self.a_name, self.b_name, channel_name(&self.channel_json), k::span(self.start_ms, self.end_ms))
    }
}

async fn load(panel: &Panel, q: &ExchangeQuery) -> Result<Loaded, ApiError> {
    let (a, b) = pair(&q.a, &q.b)?;
    let channel = blank(&q.channel).and_then(|c| parse_id(&c)).ok_or_else(|| ApiError::bad("That isn't a channel id."))?;
    let ms = |v: &Option<String>| blank(v).and_then(|s| s.parse::<i64>().ok()).filter(|m| *m > 0);
    let (Some(start_ms), Some(end_ms)) = (ms(&q.start), ms(&q.end)) else {
        return Err(ApiError::bad("That isn't a stretch of time."));
    };
    if end_ms < start_ms || end_ms - start_ms > k::MAX_PERIOD_MS {
        return Err(ApiError::bad("That isn't a stretch of time."));
    }
    let detection = match blank(&q.detection) {
        Some(raw) => {
            let id: i64 = raw.parse().map_err(|_| ApiError::bad("That isn't a detection."))?;
            let d = store::detection(&store_db()?.lock(), id).map_err(db_error)?;
            Some(d.ok_or_else(|| ApiError::not_found("No such detection."))?)
        }
        None => None,
    };
    let sensitive = never_shown(panel);
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
    let (a_name, b_name) = (name_of(panel, a, &rows).await, name_of(panel, b, &rows).await);
    Ok(Loaded { a, b, a_name, b_name, channel, channel_json, start_ms, end_ms, rows, truncated, detection })
}

fn message_json(panel: &Panel, l: &Line, guild: Option<&String>, channel: &Value, detected: &HashSet<u64>) -> Value {
    let r = l.row;
    let side = |s: Side| json!(s);
    json!({
        "n": l.n,
        "id": r.message_id.to_string(),
        "ts": r.created_ms / 1000,
        "ts_ms": r.created_ms,
        "member": member_json(panel, r.author_id, &r.author_name, &r.avatar),
        "side": side(l.side),
        "towards": l.towards.map(side),
        "text": r.content,
        "reply_to": r.reply_to.map(|id| json!({ "id": id.to_string(), "n": l.reply_n, "author": r.reply_author, "text": r.reply_text })),
        "files": r.attachments.iter().map(|a| json!({ "name": a.filename, "image": msglog::image_ext(a.content_type.as_deref(), &a.filename).is_some() })).collect::<Vec<_>>(),
        "url": guild.map(|g| search::jump_url(g, r.channel_id, r.message_id)),
        "channel": channel,
        "in_detection": detected.contains(&r.message_id),
    })
}

pub async fn exchange(State(panel): State<Panel>, axum::Extension(Caller(user)): axum::Extension<Caller>, Query(q): Query<ExchangeQuery>) -> ApiResult {
    let loaded = load(&panel, &q).await?;
    search::log_quietly("kalesh:look", user, &loaded.label());
    let lines = loaded.lines();
    let key = k::stretch_key(loaded.a, loaded.b, loaded.channel, &lines);
    let guild = panel.data.guild().map(|g| g.id);
    let detected: HashSet<u64> = loaded.detection.as_ref().map(|d| d.message_ids.iter().copied().collect()).unwrap_or_default();
    let sensitive = never_shown(&panel);
    let summaries: Vec<Value> = {
        let conn = store_db()?.lock();
        store::summaries_near(&conn, loaded.channel, loaded.a, loaded.b, loaded.start_ms, loaded.end_ms).map_err(db_error)?
    }
    .iter()
    .filter_map(|s| summary_json(&panel, s, Some(&key), &sensitive))
    .collect();
    let max = k::summary_max();
    ok(json!({
        "a": { "id": loaded.a.to_string(), "name": loaded.a_name },
        "b": { "id": loaded.b.to_string(), "name": loaded.b_name },
        "channel": loaded.channel_json,
        "start_ms": loaded.start_ms.to_string(),
        "end_ms": loaded.end_ms.to_string(),
        "start_ts": loaded.start_ms / 1000,
        "end_ts": loaded.end_ms / 1000,
        "key": key,
        "count": lines.len(),
        "a_messages": lines.iter().filter(|l| l.side == Side::A).count(),
        "b_messages": lines.iter().filter(|l| l.side == Side::B).count(),
        "bystanders": bystanders(&panel, &lines),
        "truncated": loaded.truncated,
        "max_messages": max,
        "will_trim": lines.len() > max,
        "detection_id": loaded.detection.as_ref().map(|d| d.id),
        "messages": lines.iter().map(|l| message_json(&panel, l, guild.as_ref(), &loaded.channel_json, &detected)).collect::<Vec<_>>(),
        "summaries": summaries,
    }))
}

// --- a detection, opened ---------------------------------------------------------------------------

/// Which stretch a detection opens: its two busiest people, and the stretch of
/// their engagement around the detector's window, or the window itself.
pub async fn detection(State(panel): State<Panel>, Path(id): Path<String>) -> ApiResult {
    let id: i64 = id.trim().parse().map_err(|_| ApiError::not_found("No such detection."))?;
    let d = store::detection(&store_db()?.lock(), id).map_err(db_error)?.ok_or_else(|| ApiError::not_found("No such detection."))?;
    let sensitive = never_shown(&panel);
    let channel = channel_by_id(&panel, d.channel_id, &sensitive).ok_or_else(|| ApiError::not_found("No such detection."))?;
    let [first, second, ..] = d.participants.as_slice() else {
        return Err(ApiError::bad("Only one person was in that burst, so there is no pair to look at."));
    };
    let (a, b) = (first.id, second.id);
    let around = 3 * HOUR_MS;
    let rows = panel.data.kalesh_authors(a, b, d.start_ms - around, d.end_ms + around, Some(d.channel_id)).await.map_err(unavailable)?;
    let (mut start_ms, mut end_ms) = (d.start_ms, d.end_ms);
    if let Some(s) = k::find_stretches(&rows, a, b)
        .into_iter()
        .find(|s| s.start_ms <= d.end_ms + k::JOIN_GAP_MS && s.end_ms >= d.start_ms - k::JOIN_GAP_MS)
    {
        start_ms = start_ms.min(s.start_ms);
        end_ms = end_ms.max(s.end_ms);
    }
    ok(json!({
        "id": d.id,
        "a": { "id": a.to_string(), "name": panel.cached_name(a).unwrap_or_else(|| first.name.clone()) },
        "b": { "id": b.to_string(), "name": panel.cached_name(b).unwrap_or_else(|| second.name.clone()) },
        "channel": channel,
        "start_ms": start_ms.to_string(),
        "end_ms": end_ms.to_string(),
    }))
}

// --- summarising -----------------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct SummariseBody {
    #[serde(default)]
    a: Option<String>,
    #[serde(default)]
    b: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    end: Option<String>,
    #[serde(default)]
    detection: Option<String>,
}

/// Stretches being summarised right now, so two mods pressing at once pay once.
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

pub async fn summarise(State(panel): State<Panel>, axum::Extension(Caller(user)): axum::Extension<Caller>, body: axum::body::Bytes) -> ApiResult {
    let body: SummariseBody = serde_json::from_slice(&body).map_err(|_| ApiError::bad("Pick a stretch to summarise."))?;
    let q = ExchangeQuery { a: body.a, b: body.b, channel: body.channel, start: body.start, end: body.end, detection: body.detection };
    let loaded = load(&panel, &q).await?;
    let lines = loaded.lines();
    if lines.is_empty() {
        return Err(ApiError::bad("No messages are kept for that stretch, so there is nothing to summarise."));
    }
    let key = k::stretch_key(loaded.a, loaded.b, loaded.channel, &lines);
    let sensitive = never_shown(&panel);
    let label = loaded.label();

    if let Some(done) = store::summary_for(&store_db()?.lock(), &key).map_err(db_error)? {
        search::log_quietly("kalesh:summary", user, &label);
        return ok(json!({ "reused": true, "summary": summary_json(&panel, &done, Some(&key), &sensitive) }));
    }
    let Some(_running) = Running::claim(&key) else {
        return Err(ApiError(StatusCode::CONFLICT, "Someone is summarising this right now. Give it a minute and open it again.".into()));
    };

    let prompt = k::build_prompt(&loaded.a_name, &loaded.b_name, &channel_name(&loaded.channel_json), &lines, k::summary_max());
    let reply = panel.data.kalesh_summarise(prompt.text).await.map_err(|err| {
        tracing::warn!("kalesh: a summary failed: {}", err);
        ApiError(StatusCode::BAD_GATEWAY, "The model didn't give a summary. Nothing was saved or spent twice; try again in a minute.".into())
    })?;
    let parsed = k::parse_summary(&reply.text, lines.len());
    if parsed.is_none() && reply.text.trim().is_empty() {
        return Err(ApiError(StatusCode::BAD_GATEWAY, "The model sent back nothing. Try again in a minute.".into()));
    }
    let new = store::NewSummary {
        stretch_key: key.clone(),
        channel_id: loaded.channel,
        a_id: loaded.a,
        b_id: loaded.b,
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

    use super::super::super::super::kalesh::{self as k, Reply};
    use super::super::super::super::msglog::{NewMessage, Place, SaidRow, Store};

    pub const NIKHIL: u64 = 2020;
    pub const AISHA: u64 = 2021;
    pub const RAHUL: u64 = 2022;
    pub const KAVYA: u64 = 2023;
    pub const SAFE: u64 = super::super::tests::SAFE;
    const MIN: i64 = 60_000;
    const HOUR: i64 = 60 * MIN;

    pub struct FightLog {
        pub now: i64,
        pub store: parking_lot::Mutex<Store>,
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
            _ => "Someone",
        }
    }

    /// One message, `ago` before now; `reply` is the id it answers.
    fn put(store: &mut Store, seq: &mut u64, at: i64, author: u64, channel: (u64, &str), text: &str, reply: Option<u64>) -> u64 {
        *seq += 1;
        let id = id_at(at, *seq);
        let m = NewMessage {
            message_id: id,
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
        };
        store.insert_new(&m).expect("fake fight message");
        id
    }

    /// Nikhil and Aisha: a proper cricket kalesh in #desi-banter twenty hours ago,
    /// with Rahul trying to calm it and Kavya joining in; a small back-and-forth in
    /// #general three hours ago; an older one in #memes three days ago; and a
    /// ping in #safe-corner and one nine days back that must never show.
    pub fn log() -> &'static FightLog {
        static LOG: OnceLock<FightLog> = OnceLock::new();
        LOG.get_or_init(|| {
            let dir = tempfile::tempdir().expect("temp dir");
            let now = chrono::Utc::now().timestamp_millis();
            let mut store = Store::open(&dir.path().join("fight.db"), dir.path().join("files"), now).expect("fight store");
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
            // The detector caught the middle of it: messages 10 to 21.
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
            // Never shown: #safe-corner, and older than the longest period.
            put(s, q, now - 2 * HOUR, NIKHIL, (SAFE, "safe-corner"), "<@2021> SECRET-SAFE we need to talk", None);
            put(s, q, now - 2 * HOUR + MIN, AISHA, (SAFE, "safe-corner"), "SECRET-SAFE ok", None);
            put(s, q, now - 9 * 24 * HOUR, NIKHIL, (24, "music"), "<@2021> ancient history", None);
            FightLog { now, store: parking_lot::Mutex::new(store), _dir: dir }
        })
    }

    pub fn authors(a: u64, b: u64, since_ms: i64, until_ms: i64, channel: Option<u64>) -> anyhow::Result<Vec<SaidRow>> {
        Ok(k::authors_between(log().store.lock().conn(), a, b, since_ms, until_ms, channel, k::AUTHOR_ROWS)?)
    }

    pub fn channel(channel: u64, since_ms: i64, until_ms: i64) -> anyhow::Result<Vec<SaidRow>> {
        Ok(k::channel_between(log().store.lock().conn(), channel, since_ms, until_ms, k::EXCHANGE_ROWS)?)
    }

    /// Every prompt the fake model has been sent.
    pub static PROMPTS: parking_lot::Mutex<Vec<String>> = parking_lot::Mutex::new(Vec::new());

    /// A canned summary of the #desi-banter kalesh; any other stretch gets a
    /// short one with no flags.
    pub fn summarise(prompt: String) -> anyhow::Result<Reply> {
        let fight = prompt.contains("Channel: #desi-banter.");
        PROMPTS.lock().push(prompt);
        let text = if fight {
            serde_json::json!({
                "overview": "Nikhil and Aisha argued in #desi-banter about whether RCB can win the IPL this year. It started as cricket banter and turned personal for a few minutes before both stepped back.",
                "trigger": "Nikhil said RCB would win the cup [#1]; Aisha replied that he says this every year [#2].",
                "timeline": [
                    { "time": "", "what": "Nikhil predicts an RCB title; Aisha teases him for saying it every year.", "refs": [1, 2] },
                    { "time": "", "what": "They trade points about RCB's bowling and a recent 220-run match.", "refs": [3, 4, 5] },
                    { "time": "", "what": "Nikhil asks whether Aisha watches cricket or only memes; Aisha says it has become personal.", "refs": [7, 8] },
                    { "time": "", "what": "Rahul asks both to calm down; Kavya agrees with Aisha.", "refs": [9, 12] },
                    { "time": "", "what": "Nikhil mentions Aisha's college in Pune and offers to tell everyone; Aisha asks him to stop.", "refs": [17, 18] },
                    { "time": "", "what": "Rahul tells Nikhil it is too much. Aisha mutes the channel; Nikhil later apologises for the college remark.", "refs": [19, 21, 23] }
                ],
                "positions": [
                    { "who": "Nikhil", "points": ["RCB's bowling is better this year, so one bad match shouldn't count.", "He felt Aisha's opening remark was the first jab."] },
                    { "who": "Aisha", "points": ["Results and stats matter more than hope.", "Being called a memes-only fan was the personal part."] }
                ],
                "others": "Rahul tried to calm it twice and told Nikhil to drop the college remark [#19]. Kavya agreed with Aisha about RCB fans [#12].",
                "ending": { "state": "fizzled", "what": "Nikhil apologised for the college remark [#23]; Aisha accepted but kept her view on RCB [#24], and it ended in banter [#25]." },
                "interpretation": ["It reads mostly as heated banter; the college remark [#17] seems to be the point it stopped being fun for Aisha."],
                "flags": [{ "kind": "personal_info", "who": "Nikhil", "message": 17, "what": "Brought up Aisha's college and offered to share it with everyone; she asked him to stop [#18]." }]
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
    use super::fake::{AISHA, KAVYA, NIKHIL, PROMPTS, RAHUL, SAFE};
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

    fn stretch_query(s: &Value, a: u64, b: u64) -> String {
        format!("a={}&b={}&channel={}&start={}&end={}", a, b, s["channel"]["id"].as_str().unwrap(), s["start_ms"].as_str().unwrap(), s["end_ms"].as_str().unwrap())
    }

    fn stretch_body(s: &Value, a: u64, b: u64) -> Value {
        json!({ "a": a.to_string(), "b": b.to_string(), "channel": s["channel"]["id"], "start": s["start_ms"], "end": s["end_ms"] })
    }

    async fn find(app: &Router, session: &str, query: &str) -> (StatusCode, Value) {
        call(app, "GET", &format!("/api/kalesh/find?{}", query), Some(session), None).await
    }

    fn channels_of(found: &Value) -> Vec<String> {
        found["stretches"].as_array().unwrap().iter().map(|s| s["channel"]["name"].as_str().unwrap().to_string()).collect()
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
        assert_eq!(fight["messages"], 25, "{fight}");
        assert_eq!((fight["a_messages"].as_u64(), fight["b_messages"].as_u64(), fight["others"].as_u64()), (Some(11), Some(10), Some(4)));
        // Replies to a bystander don't count as replies to each other.
        assert_eq!((fight["replies_ab"].as_u64(), fight["replies_ba"].as_u64()), (Some(7), Some(9)), "{fight}");
        assert_eq!(fight["mentions"], 1);
        let people: Vec<(&str, u64)> = fight["bystanders"].as_array().unwrap().iter().map(|p| (p["id"].as_str().unwrap(), p["messages"].as_u64().unwrap())).collect();
        assert_eq!(people, vec![(RAHUL.to_string().as_str(), 2), (KAVYA.to_string().as_str(), 2)], "Rahul's opening question came before the fight");
        // The #general one is only nearness: no replies, no mentions.
        let near = &found["stretches"][0];
        assert_eq!((near["nearby"].as_u64(), near["replies_ab"].as_u64(), near["mentions"].as_u64()), (Some(4), Some(0), Some(0)));
        assert!(!found.to_string().contains("SECRET-SAFE"));
    }

    #[tokio::test]
    async fn the_exchange_shows_everyone_in_order_and_marks_sides() {
        let app = panel();
        let session = session_for(ADMIN);
        let (_, found) = find(&app, &session, &format!("a={NIKHIL}&b={AISHA}")).await;
        let fight = found["stretches"][1].clone();
        let (status, ex) = call(&app, "GET", &format!("/api/kalesh/exchange?{}", stretch_query(&fight, NIKHIL, AISHA)), Some(&session), None).await;
        assert_eq!(status, StatusCode::OK, "{ex}");
        let msgs = ex["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 25);
        assert_eq!(msgs[0]["n"], 1);
        assert!(msgs.windows(2).all(|w| w[0]["ts_ms"].as_i64() <= w[1]["ts_ms"].as_i64()), "in order");
        let by = |text: &str| msgs.iter().find(|m| m["text"].as_str().unwrap().starts_with(text)).unwrap().clone();
        assert_eq!(by("RCB is winning")["side"], "a");
        assert_eq!(by("bhai har saal")["side"], "b");
        assert_eq!(by("bhai har saal")["towards"], "a");
        let kavya = by("aisha is right");
        assert_eq!((kavya["side"].as_str(), kavya["towards"].as_str()), (Some("other"), Some("a")), "Kavya replies to Nikhil");
        let rahul = by("nikhil bhai that's too much");
        assert_eq!((rahul["side"].as_str(), rahul["towards"].as_str()), (Some("other"), Some("a")));
        assert_eq!(by("guys chill")["towards"], Value::Null);
        // Every message links to Discord, and replies say which number they answer.
        assert!(msgs.iter().all(|m| m["url"].as_str().is_some_and(|u| u.starts_with("https://discord.com/channels/900/23/"))));
        assert_eq!(by("bhai har saal")["reply_to"]["n"], 1);
        assert_eq!(ex["will_trim"], false);
        // #safe-corner is never read, even asked for directly.
        let bad = format!("a={NIKHIL}&b={AISHA}&channel={SAFE}&start=1&end={}", chrono::Utc::now().timestamp_millis());
        let (status, _) = call(&app, "GET", &format!("/api/kalesh/exchange?{}", bad), Some(&session), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
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
        let (status, first) = call(&app, "POST", "/api/kalesh/summarise", Some(&session), Some(stretch_body(&near, AISHA, NIKHIL))).await;
        assert_eq!(status, StatusCode::OK, "{first}");
        assert_eq!(first["reused"], false);
        assert_eq!(asked(), before + 1);
        let s = &first["summary"];
        assert_eq!(s["run_by"]["id"], ADMIN.to_string());
        assert_eq!(s["model"], "fake/main-model");
        assert!(s["input_tokens"].as_u64().unwrap() > 500, "the prompt's tokens are counted: {s}");
        assert_eq!((s["trimmed"].as_bool(), s["message_count"].as_u64()), (Some(false), Some(4)));
        assert_eq!(s["summary"]["flags"], json!([]));
        // Again, by another admin and the other way round: the stored one, no new call.
        let (status, again) = call(&app, "POST", "/api/kalesh/summarise", Some(&session_for(ADMIN_TWO)), Some(stretch_body(&near, NIKHIL, AISHA))).await;
        assert_eq!(status, StatusCode::OK, "{again}");
        assert_eq!(again["reused"], true);
        assert_eq!(again["summary"]["id"], s["id"]);
        assert_eq!(asked(), before + 1, "running it twice doesn't pay twice");
        // The stretch now says it has one, and the overview lists it.
        let (_, found) = find(&app, &session, &format!("a={AISHA}&b={NIKHIL}")).await;
        assert_eq!(found["stretches"][0]["summarised"], true);
        let (_, ex) = call(&app, "GET", &format!("/api/kalesh/exchange?{}", stretch_query(&near, NIKHIL, AISHA)), Some(&session), None).await;
        assert_eq!(ex["summaries"][0]["id"], s["id"]);
        assert_eq!(ex["summaries"][0]["current"], true);
        let (_, overview) = call(&app, "GET", "/api/kalesh", Some(&session), None).await;
        assert!(overview["summaries"].as_array().unwrap().iter().any(|x| x["id"] == s["id"]), "{overview}");
    }

    #[tokio::test]
    async fn the_fight_is_summarised_with_its_flags_linked_to_messages() {
        let app = panel();
        let session = session_for(ADMIN_TWO);
        let (_, found) = find(&app, &session, &format!("a={NIKHIL}&b={AISHA}")).await;
        let fight = found["stretches"][1].clone();
        let (status, got) = call(&app, "POST", "/api/kalesh/summarise", Some(&session), Some(stretch_body(&fight, NIKHIL, AISHA))).await;
        assert_eq!(status, StatusCode::OK, "{got}");
        let s = &got["summary"];
        assert_eq!(s["summary"]["flags"][0]["kind"], "personal_info");
        assert_eq!(s["summary"]["flags"][0]["message"], 17);
        // [#17] is the college message: the page can link it.
        let (_, ex) = call(&app, "GET", &format!("/api/kalesh/exchange?{}", stretch_query(&fight, NIKHIL, AISHA)), Some(&session), None).await;
        let ids = s["message_ids"].as_array().unwrap();
        let flagged = ex["messages"].as_array().unwrap().iter().find(|m| m["id"] == ids[16]).unwrap();
        assert!(flagged["text"].as_str().unwrap().contains("college"));
        let prompt = PROMPTS.lock().iter().rev().find(|p| p.contains("Channel: #desi-banter.")).cloned().unwrap();
        assert!(prompt.contains("A = Nikhil, B = Aisha") && prompt.contains("Others who spoke: Rahul, Kavya."), "{prompt}");
        assert!(prompt.contains("@Aisha naam bata"), "mentions read as names");
    }

    #[tokio::test]
    async fn every_summary_is_in_the_activity_log_once_per_quarter_hour() {
        let app = panel();
        let session = session_for(ADMIN);
        let (_, found) = find(&app, &session, &format!("a={NIKHIL}&b={AISHA}&hours=168")).await;
        let memes = found["stretches"].as_array().unwrap().iter().find(|s| s["channel"]["name"] == "memes").unwrap().clone();
        for _ in 0..2 {
            let (status, _) = call(&app, "POST", "/api/kalesh/summarise", Some(&session), Some(stretch_body(&memes, NIKHIL, AISHA))).await;
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
        let rows = super::fake::channel(23, log.now - 21 * HOUR_MS, log.now).unwrap();
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
        // Opening it widens the burst to the whole stretch.
        let (status, open) = call(&app, "GET", &format!("/api/kalesh/detections/{}", id), Some(&session), None).await;
        assert_eq!(status, StatusCode::OK, "{open}");
        assert_eq!((open["a"]["id"].as_str(), open["b"]["id"].as_str()), (Some(NIKHIL.to_string().as_str()), Some(AISHA.to_string().as_str())));
        let q = format!("a={}&b={}&channel=23&start={}&end={}&detection={}", NIKHIL, AISHA, open["start_ms"].as_str().unwrap(), open["end_ms"].as_str().unwrap(), id);
        let (status, ex) = call(&app, "GET", &format!("/api/kalesh/exchange?{}", q), Some(&session), None).await;
        assert_eq!(status, StatusCode::OK, "{ex}");
        assert_eq!(ex["messages"].as_array().unwrap().len(), 25, "the whole stretch, not only the burst");
        assert_eq!(ex["messages"].as_array().unwrap().iter().filter(|m| m["in_detection"] == true).count(), 12);
        assert_eq!(ex["detection_id"], id);
    }

    #[tokio::test]
    async fn the_kalesh_page_is_for_admins_only() {
        let app = panel();
        let member = session_for(MEMBER);
        for (method, path) in [("GET", "/api/kalesh"), ("GET", "/api/kalesh/find?a=2020&b=2021"), ("GET", "/api/kalesh/exchange?a=2020&b=2021&channel=23&start=1&end=2"), ("GET", "/api/kalesh/detections/1")] {
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
