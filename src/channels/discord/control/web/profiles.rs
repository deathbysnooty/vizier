//! The panel's side of member analyses: the most-active ranking, gathering a
//! member's messages and numbers, asking the model, the one-at-a-time job, and
//! copying chosen fields into a member's note.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use parking_lot::Mutex;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::super::members::{self as notes, MemberNote};
use super::super::profiles::{self, Activity, Profile, RankBy, RawMessage, Status};
use super::members::MemberStats;
use super::{ApiError, ApiResult, Caller, Panel, ok, parse_id};

/// Pause between two members in a job, so a batch doesn't hammer the model.
#[cfg(not(test))]
const PAUSE: Duration = Duration::from_secs(4);
#[cfg(test)]
const PAUSE: Duration = Duration::from_millis(5);
/// A member analysed more recently than this is skipped unless forced.
pub const FRESH_SECS: i64 = 24 * 3600;
pub const MAX_BATCH: usize = 50;
/// Newest stored requests read per member before the transcript limits apply.
pub const SCAN_ROWS: usize = 600;

// --- live readers ---------------------------------------------------------------------

/// Everyone's messages, voice and game points over the last 30 days, with house
/// and Muggle flags. Locks are taken one at a time; opt-outs before the house lock.
pub fn read_activity_live(now: i64) -> Vec<Activity> {
    let start = now - profiles::WINDOW_DAYS * 86_400;
    let optouts = super::super::super::house::optout_set();
    let mut by_user: HashMap<u64, Activity> = HashMap::new();
    fn row(by_user: &mut HashMap<u64, Activity>, u: u64) -> &mut Activity {
        by_user.entry(u).or_insert(Activity { user_id: u, messages: 0, voice_secs: 0, points: 0, house: None, muggle: false })
    }
    if let Some(db) = super::super::super::stats::db() {
        let conn = db.lock();
        let exclude: std::collections::HashSet<u64> = super::super::ids("VIZIER_STATS_EXCLUDE_CHANNELS").into_iter().collect();
        if let Ok(mut stmt) = conn.prepare("SELECT user_id, channel_id, SUM(count) FROM msg_counts WHERE day >= ?1 GROUP BY user_id, channel_id") {
            let first_day = super::super::super::points::ist_day(start);
            if let Ok(rows) = stmt.query_map(params![first_day], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64, r.get::<_, i64>(2)?))) {
                for (u, c, n) in rows.flatten() {
                    if !exclude.contains(&c) {
                        row(&mut by_user, u).messages += n;
                    }
                }
            }
        }
        if let Ok(voice) = super::super::super::activity::voice_between(&conn, None, start, now, now) {
            for (u, secs) in voice {
                row(&mut by_user, u).voice_secs += secs;
            }
        }
    }
    if let Some(db) = super::super::super::house::db() {
        let conn = db.lock();
        let _ = ledger_points(&conn, start, &mut |u, h, n| {
            let r = row(&mut by_user, u);
            r.points += n;
            r.house.get_or_insert(h);
        });
        if let Ok(mut stmt) = conn.prepare("SELECT user_id, house FROM members") {
            if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, String>(1)?))) {
                for (u, h) in rows.flatten() {
                    if let Some(a) = by_user.get_mut(&u) {
                        a.house = Some(h);
                    }
                }
            }
        }
    }
    by_user
        .into_values()
        .map(|mut a| {
            a.muggle = optouts.contains(&a.user_id);
            a
        })
        .collect()
}

/// Game and activity points per person since `start`, per house through the
/// (house, ts) index; mods' and weekly awards are not games.
pub fn ledger_points(conn: &Connection, start: i64, add: &mut dyn FnMut(u64, String, i64)) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(
        "SELECT user_id, SUM(points) FROM ledger WHERE house = ?1 AND ts >= ?2 AND user_id IS NOT NULL
         AND source NOT IN ('mod', 'weekly') GROUP BY user_id",
    )?;
    for h in super::super::super::house::HOUSES {
        for (u, n) in stmt.query_map(params![h.key, start], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)?)))?.flatten() {
            add(u, h.key.to_string(), n);
        }
    }
    Ok(())
}

/// A member's stored requests over the last 30 days, newest first, with the
/// channel and DM flag from the request's metadata. Uses the read-only history
/// connection and the (agent_id, timestamp) index.
pub fn query_messages(conn: &Connection, agent_id: &str, user: u64, now: i64) -> rusqlite::Result<Vec<RawMessage>> {
    let mut stmt = conn.prepare(
        "SELECT channel, timestamp, data FROM session_history INDEXED BY idx_sh_agent_time
         WHERE agent_id = ?1 AND timestamp >= ?2 AND content_type = 'Request' AND data LIKE ?3
         ORDER BY timestamp DESC LIMIT ?4",
    )?;
    let since_ms = (now - profiles::WINDOW_DAYS * 86_400) * 1000;
    let pattern = format!("%(DiscordId: {})%", user);
    let rows = stmt.query_map(params![agent_id, since_ms, pattern, SCAN_ROWS as i64], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?))
    })?;
    Ok(rows.flatten().filter_map(|(channel, ts, data)| parse_message(&channel, ts, &data, user)).collect())
}

pub fn parse_message(channel: &str, ts_ms: i64, data: &str, user: u64) -> Option<RawMessage> {
    let v: Value = serde_json::from_str(data).ok()?;
    let req = v.get("content")?.get("Request")?;
    if !req.get("user")?.as_str()?.ends_with(&format!("(DiscordId: {})", user)) {
        return None;
    }
    let text = req.get("content")?.as_object()?.values().next()?.as_str()?.to_string();
    let meta = req.get("metadata");
    let is_dm = meta.and_then(|m| m.get("is_dm")).and_then(Value::as_bool).unwrap_or(false);
    let channel_id = meta
        .and_then(|m| m.get("discord_channel_id"))
        .and_then(Value::as_str)
        .and_then(|c| c.parse().ok())
        .or_else(|| channel.strip_prefix("discord__").and_then(|r| r.split("__").next()).and_then(|c| c.parse().ok()));
    Some(RawMessage { ts: ts_ms / 1000, channel_id, parent_id: None, is_dm, text })
}

// --- analysing one member ---------------------------------------------------------------

fn stats_text(stats: &MemberStats, act: Option<&Activity>, channel_name: &dyn Fn(u64) -> Option<String>, sensitive: &[u64]) -> (String, Value) {
    let mut lines = Vec::new();
    let house = stats.house.as_deref().and_then(super::super::super::house::house);
    lines.push(match (house, stats.muggle) {
        (_, true) => "- House: none (stepped out, a Muggle)".to_string(),
        (Some(h), _) => format!("- House: {}{}", h.name, if stats.captain { " (captain)" } else { "" }),
        (None, _) => "- House: not sorted".to_string(),
    });
    let messages = act.map(|a| a.messages).unwrap_or(stats.messages_30d);
    let channels: Vec<String> = stats
        .top_channels
        .iter()
        .filter(|(c, _)| !sensitive.contains(c))
        .filter_map(|(c, n)| channel_name(*c).map(|name| format!("#{} ({})", name, n)))
        .collect();
    lines.push(format!("- Messages: {}{}", messages, if channels.is_empty() { String::new() } else { format!(", most in {}", channels.join(", ")) }));
    let mut hours: Vec<(usize, i64)> = stats.hours.iter().copied().enumerate().filter(|(_, n)| *n > 0).collect();
    hours.sort_by(|a, b| b.1.cmp(&a.1));
    if !hours.is_empty() {
        lines.push(format!(
            "- Busiest hours (India time): {}",
            hours.iter().take(3).map(|(h, _)| format!("{:02}:00", h)).collect::<Vec<_>>().join(", ")
        ));
    }
    let voice_min = act.map(|a| a.voice_secs / 60).unwrap_or(0);
    lines.push(format!("- Voice: {} h {} min", voice_min / 60, voice_min % 60));
    let points = act.map(|a| a.points).unwrap_or(0);
    let month: Vec<String> = stats.month.iter().filter(|(s, _)| s != "weekly" && s != "mod").map(|(s, n)| format!("{} {}", s.replace('_', " "), n)).collect();
    lines.push(format!("- House points from games and activity: {}{}", points, if month.is_empty() { String::new() } else { format!(" (this calendar month: {})", month.join(", ")) }));
    lines.push(format!(
        "- Quiz answers won: {} all time; fights: {} ({} won); battle royales won: {}; Snitches caught this month: {}",
        stats.quiz_all, stats.fights, stats.wins, stats.crowns, stats.snitch_month
    ));
    let snapshot = json!({
        "house": stats.house, "captain": stats.captain, "muggle": stats.muggle,
        "messages": messages, "voice_min": voice_min, "points": points,
        "top_channels": channels, "quiz_all": stats.quiz_all, "fights": stats.fights, "wins": stats.wins,
        "crowns": stats.crowns, "snitch_month": stats.snitch_month,
    });
    (lines.join("\n"), snapshot)
}

/// The prompt for one member, and how many messages and characters went in.
pub async fn prepare(panel: &Panel, user: u64, name: &str, now: i64) -> (String, usize, usize, Value) {
    let channels = panel.data.channels();
    let sensitive = panel.data.sensitive_channels();
    let known: std::collections::HashSet<u64> = channels.iter().filter_map(|c| c.id.parse().ok()).collect();
    let mut messages = panel.data.member_messages(user, now).await;
    // A channel the server doesn't list and that isn't a known thread (an
    // archived thread, somewhere private) is left out: it might be sensitive.
    for m in &mut messages {
        if m.parent_id.is_none() {
            m.parent_id = m.channel_id.and_then(|c| panel.data.thread_parent(c));
        }
    }
    messages.retain(|m| m.channel_id.is_some_and(|c| known.contains(&c)) || m.parent_id.is_some_and(|p| known.contains(&p)));
    let channel_name = |c: u64| channels.iter().find(|x| x.id == c.to_string()).map(|x| x.name.clone());
    let name_of = |id: u64| panel.data.cached_member(id).map(|m| m.name);
    let (transcript, count) = profiles::transcript(&messages, &sensitive, &channel_name, &name_of);
    let stats = panel.data.member_stats(user, now).await;
    let activity = activity_cached(panel, now).await;
    let (stats_text, snapshot) = stats_text(&stats, activity.iter().find(|a| a.user_id == user), &channel_name, &sensitive);
    let chars = transcript.chars().count();
    (profiles::build_prompt(name, &stats_text, &transcript, count), count, chars, snapshot)
}

pub async fn analyse_one(panel: &Panel, user: u64, by: u64, now: i64) -> Result<Profile, String> {
    let who = match panel.data.cached_member(user) {
        Some(m) => Some(m),
        None => panel.data.member(user).await,
    };
    let Some(who) = who else {
        return Err("not in the server".into());
    };
    if who.bot {
        return Err("bots aren't analysed".into());
    }
    let (prompt, count, chars, snapshot) = prepare(panel, user, &who.name, now).await;
    let mut asked = prompt.clone();
    let mut last_err = String::new();
    for attempt in 0..2 {
        let (raw, model) = panel.data.ask_model(asked.clone()).await.map_err(|e| format!("the model failed: {}", e))?;
        match profiles::parse_analysis(&raw) {
            Ok(ai) => {
                let profile = Profile {
                    user_id: user.to_string(),
                    name: who.name.clone(),
                    ai,
                    edits: Default::default(),
                    status: Status::Draft,
                    stats: snapshot,
                    messages_analysed: count,
                    chars_analysed: chars,
                    window_days: profiles::WINDOW_DAYS,
                    generated_ts: now,
                    generated_by: by.to_string(),
                    model,
                    edited_ts: 0,
                    edited_by: String::new(),
                };
                profiles::save(&profile, by, "generated", &[]).map_err(|e| e.to_string())?;
                return Ok(profile);
            }
            Err(err) => {
                tracing::warn!("profiles: unreadable analysis for {} (attempt {}): {}", user, attempt + 1, err);
                last_err = err.clone();
                asked = format!(
                    "{}\n\nYour previous answer could not be used ({}). Reply with only the JSON object in the shape above.",
                    prompt, err
                );
            }
        }
    }
    Err(format!("the model's answer couldn't be read twice: {}", last_err))
}

async fn activity_cached(panel: &Panel, now: i64) -> Vec<Activity> {
    static CACHE: LazyLock<Mutex<Option<(i64, Vec<Activity>)>>> = LazyLock::new(|| Mutex::new(None));
    if let Some((at, rows)) = CACHE.lock().as_ref() {
        if (now - at).abs() < 60 && !cfg!(test) {
            return rows.clone();
        }
    }
    let rows = panel.data.activity(now).await;
    *CACHE.lock() = Some((now, rows.clone()));
    rows
}

// --- the job ---------------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct JobItem {
    pub user_id: String,
    pub name: String,
    /// queued, running, done, skipped, failed, cancelled
    pub status: String,
    pub detail: Option<String>,
}

#[derive(Debug)]
pub struct Job {
    pub id: u64,
    pub started_ts: i64,
    pub by: u64,
    pub force: bool,
    pub items: Vec<JobItem>,
    pub finished_ts: Option<i64>,
    pub cancel: Arc<AtomicBool>,
}

static JOB: LazyLock<Mutex<Option<Job>>> = LazyLock::new(|| Mutex::new(None));

fn job_json(job: &Option<Job>) -> Value {
    match job {
        None => json!({ "job": null }),
        Some(j) => {
            let count = |s: &str| j.items.iter().filter(|i| i.status == s).count();
            json!({ "job": {
                "id": j.id,
                "started_ts": j.started_ts,
                "by": j.by.to_string(),
                "finished_ts": j.finished_ts,
                "running": j.finished_ts.is_none(),
                "cancelled": j.cancel.load(Ordering::SeqCst),
                "total": j.items.len(),
                "done": count("done"),
                "skipped": count("skipped"),
                "failed": count("failed"),
                "items": j.items,
            }})
        }
    }
}

#[derive(Deserialize)]
pub struct AnalyseBody {
    #[serde(default)]
    user_ids: Vec<String>,
    #[serde(default)]
    top: Option<usize>,
    #[serde(default)]
    force: bool,
}

pub async fn start_job(
    State(panel): State<Panel>,
    axum::Extension(Caller(user)): axum::Extension<Caller>,
    body: axum::body::Bytes,
) -> ApiResult {
    let body: AnalyseBody = serde_json::from_slice(&body).map_err(|e| ApiError::bad(format!("Send user_ids or top: {}", e)))?;
    let now = chrono::Utc::now().timestamp();
    let ids: Vec<u64> = if !body.user_ids.is_empty() {
        if body.user_ids.len() > MAX_BATCH {
            return Err(ApiError::bad(format!("At most {} members at a time.", MAX_BATCH)));
        }
        let mut ids = Vec::new();
        for raw in &body.user_ids {
            let id = parse_id(raw).ok_or_else(|| ApiError::bad(format!("\"{}\" isn't a member id.", raw)))?;
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        ids
    } else {
        let top = body.top.ok_or_else(|| ApiError::bad("Send user_ids or top."))?;
        if top == 0 || top > MAX_BATCH {
            return Err(ApiError::bad(format!("Top is 1 to {}.", MAX_BATCH)));
        }
        profiles::rank(activity_cached(&panel, now).await, RankBy::Overall)
            .into_iter()
            .map(|r| r.activity.user_id)
            .filter(|id| !panel.data.cached_member(*id).is_some_and(|m| m.bot))
            .take(top)
            .collect()
    };
    if ids.is_empty() {
        return Err(ApiError::bad("Nobody to analyse."));
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let job_id = {
        let mut slot = JOB.lock();
        if slot.as_ref().is_some_and(|j| j.finished_ts.is_none()) {
            return Err(ApiError(StatusCode::CONFLICT, "An analysis is already running. Wait for it or cancel it.".into()));
        }
        let id = slot.as_ref().map(|j| j.id + 1).unwrap_or(1);
        let items = ids
            .iter()
            .map(|u| JobItem {
                user_id: u.to_string(),
                name: panel.data.cached_member(*u).map(|m| m.name).unwrap_or_else(|| format!("Member {}", u)),
                status: "queued".into(),
                detail: None,
            })
            .collect();
        *slot = Some(Job { id, started_ts: now, by: user, force: body.force, items, finished_ts: None, cancel: cancel.clone() });
        id
    };
    tracing::info!("profiles: {} started analysing {} members", user, ids.len());
    let runner = panel.clone();
    tokio::spawn(async move { run_job(runner, job_id, ids, user, body.force, cancel).await });
    let snapshot = job_json(&JOB.lock());
    Ok((StatusCode::ACCEPTED, axum::Json(snapshot)).into_response())
}


fn set_item(job_id: u64, index: usize, status: &str, detail: Option<String>) {
    if let Some(job) = JOB.lock().as_mut().filter(|j| j.id == job_id) {
        if let Some(item) = job.items.get_mut(index) {
            item.status = status.to_string();
            item.detail = detail;
        }
    }
}

async fn run_job(panel: Panel, job_id: u64, ids: Vec<u64>, by: u64, force: bool, cancel: Arc<AtomicBool>) {
    for (i, user) in ids.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            for j in i..ids.len() {
                set_item(job_id, j, "cancelled", None);
            }
            break;
        }
        let now = chrono::Utc::now().timestamp();
        if !force {
            if let Some(p) = profiles::get(*user) {
                if now - p.generated_ts < FRESH_SECS {
                    set_item(job_id, i, "skipped", Some("analysed in the last 24 hours".into()));
                    continue;
                }
            }
        }
        set_item(job_id, i, "running", None);
        match analyse_one(&panel, *user, by, now).await {
            Ok(p) => set_item(job_id, i, "done", Some(format!("{} messages", p.messages_analysed))),
            Err(e) => set_item(job_id, i, "failed", Some(e)),
        }
        if i + 1 < ids.len() {
            tokio::time::sleep(PAUSE).await;
        }
    }
    if let Some(job) = JOB.lock().as_mut().filter(|j| j.id == job_id) {
        job.finished_ts = Some(chrono::Utc::now().timestamp());
    }
}

pub async fn get_job() -> ApiResult {
    ok(job_json(&JOB.lock()))
}

pub async fn cancel_job() -> ApiResult {
    let guard = JOB.lock();
    match guard.as_ref() {
        Some(j) if j.finished_ts.is_none() => {
            j.cancel.store(true, Ordering::SeqCst);
            Ok(axum::Json(job_json(&guard)).into_response())
        }
        _ => Err(ApiError::not_found("No analysis is running.")),
    }
}

// --- ranking and profiles -----------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ActiveQuery {
    #[serde(default)]
    by: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

pub async fn active(State(panel): State<Panel>, Query(q): Query<ActiveQuery>) -> ApiResult {
    let by = match q.by.as_deref().unwrap_or("overall") {
        "overall" => RankBy::Overall,
        "chat" => RankBy::Chat,
        "voice" => RankBy::Voice,
        "games" => RankBy::Games,
        _ => return Err(ApiError::bad("Sort by overall, chat, voice or games.")),
    };
    let limit = q.limit.unwrap_or(50).clamp(1, 100);
    let now = chrono::Utc::now().timestamp();
    let rows: Vec<Activity> =
        activity_cached(&panel, now).await.into_iter().filter(|a| !panel.data.cached_member(a.user_id).is_some_and(|m| m.bot)).collect();
    let index = profiles::index();
    let ranked = profiles::rank(rows, by);
    let total = ranked.len();
    let out: Vec<Value> = ranked
        .iter()
        .take(limit)
        .enumerate()
        .map(|(i, r)| {
            let a = &r.activity;
            let who = panel.data.cached_member(a.user_id);
            let house = a.house.as_deref().and_then(super::super::super::house::house);
            json!({
                "rank": i + 1,
                "id": a.user_id.to_string(),
                "name": who.as_ref().map(|m| m.name.clone()),
                "avatar": who.map(|m| m.avatar),
                "house": house.map(|h| json!({ "key": h.key, "name": h.name, "crest": h.crest, "colour": format!("#{:06x}", h.colour) })),
                "muggle": a.muggle,
                "messages": a.messages,
                "voice_min": a.voice_secs / 60,
                "points": a.points,
                "shares": { "chat": r.chat_share, "voice": r.voice_share, "games": r.games_share },
                "score": r.score,
                "profile": index.get(&a.user_id).map(|(s, ts)| json!({ "status": s, "generated_ts": ts })),
            })
        })
        .collect();
    ok(json!({ "window_days": profiles::WINDOW_DAYS, "by": q.by.unwrap_or_else(|| "overall".into()), "total": total, "rows": out }))
}

fn profile_json(p: &Profile) -> Value {
    let fields: serde_json::Map<String, Value> = profiles::FIELDS
        .iter()
        .map(|(f, list)| {
            (
                f.to_string(),
                json!({
                    "label": profiles::field_label(f),
                    "list": list,
                    "value": p.effective(f),
                    "original": serde_json::to_value(&p.ai).ok().and_then(|v| v.get(*f).cloned()),
                    "edited": p.edits.contains_key(*f),
                }),
            )
        })
        .collect();
    json!({
        "user_id": p.user_id,
        "name": p.name,
        "status": p.status,
        "fields": fields,
        "stats": p.stats,
        "messages_analysed": p.messages_analysed,
        "chars_analysed": p.chars_analysed,
        "window_days": p.window_days,
        "generated_ts": p.generated_ts,
        "generated_by": p.generated_by,
        "model": p.model,
        "edited_ts": p.edited_ts,
        "edited_by": p.edited_by,
    })
}

fn user_id(raw: &str) -> Result<u64, ApiError> {
    parse_id(raw).ok_or_else(|| ApiError::bad("That isn't a member id."))
}

pub async fn get(Path(id): Path<String>) -> ApiResult {
    let id = user_id(&id)?;
    match profiles::get(id) {
        Some(p) => ok(json!({ "profile": profile_json(&p), "note": notes::get(id) })),
        None => ok(json!({ "profile": null, "note": notes::get(id) })),
    }
}

#[derive(Deserialize)]
pub struct EditBody {
    #[serde(default)]
    edits: serde_json::Map<String, Value>,
    #[serde(default)]
    status: Option<Status>,
}

pub async fn put(Path(id): Path<String>, axum::Extension(Caller(user)): axum::Extension<Caller>, body: axum::body::Bytes) -> ApiResult {
    let id = user_id(&id)?;
    let body: EditBody = serde_json::from_slice(&body).map_err(|e| ApiError::bad(format!("The change isn't right: {}", e)))?;
    let mut p = profiles::get(id).ok_or_else(|| ApiError::not_found("There's no analysis for that member."))?;
    let mut changed: Vec<&'static str> = Vec::new();
    for (field, value) in &body.edits {
        let (name, _) = profiles::FIELDS
            .iter()
            .find(|(f, _)| f == field)
            .ok_or_else(|| ApiError::bad(format!("\"{}\" isn't a field of the analysis.", field)))?;
        if value.is_null() {
            if p.edits.remove(field).is_some() {
                changed.push(name);
            }
            continue;
        }
        let stored = profiles::check_edit(field, value).map_err(ApiError::bad)?;
        let original = serde_json::to_value(&p.ai).ok().and_then(|v| v.get(field).cloned());
        let before = p.effective(field);
        if Some(&stored) == original.as_ref() {
            p.edits.remove(field);
        } else {
            p.edits.insert(field.clone(), stored.clone());
        }
        if before != stored {
            changed.push(name);
        }
    }
    changed.sort_by_key(|f| profiles::FIELDS.iter().position(|(x, _)| x == f));
    let now = chrono::Utc::now().timestamp();
    if !changed.is_empty() {
        p.edited_ts = now;
        p.edited_by = user.to_string();
        profiles::save(&p, user, "edited", &changed).map_err(ApiError::internal)?;
    }
    if let Some(status) = body.status {
        if status != p.status {
            p.status = status;
            p.edited_ts = now;
            p.edited_by = user.to_string();
            profiles::save(&p, user, if status == Status::Reviewed { "reviewed" } else { "draft" }, &[]).map_err(ApiError::internal)?;
        }
    }
    ok(json!({ "profile": profile_json(&p), "changed": changed }))
}

pub async fn delete(Path(id): Path<String>, axum::Extension(Caller(user)): axum::Extension<Caller>) -> ApiResult {
    let id = user_id(&id)?;
    if !profiles::delete(id, user).map_err(ApiError::internal)? {
        return Err(ApiError::not_found("There's no analysis for that member."));
    }
    ok(json!({ "ok": true }))
}

#[derive(Deserialize)]
pub struct ApplyBody {
    #[serde(default)]
    fields: Vec<String>,
    #[serde(default)]
    tone: bool,
}

struct Planned {
    note: MemberNote,
    over_by: usize,
    added: String,
}

async fn plan_apply(panel: &Panel, id: u64, body: &ApplyBody) -> Result<(Profile, Planned), ApiError> {
    let p = profiles::get(id).ok_or_else(|| ApiError::not_found("There's no analysis for that member."))?;
    for f in &body.fields {
        if !profiles::FIELDS.iter().any(|(x, _)| x == f) || f == "suggested_tone" {
            return Err(ApiError::bad(format!("\"{}\" can't be added to notes.", f)));
        }
    }
    if body.fields.is_empty() && !body.tone {
        return Err(ApiError::bad("Pick at least one field, or the tone."));
    }
    let name = match panel.data.cached_member(id) {
        Some(m) => m.name,
        None => panel.data.member(id).await.map(|m| m.name).unwrap_or_else(|| p.name.clone()),
    };
    let existing = notes::get(id);
    let mut note = existing.clone().unwrap_or(MemberNote {
        user_id: id.to_string(),
        name: name.clone(),
        tone: Default::default(),
        notes: String::new(),
        use_in_replies: true,
        updated_ts: 0,
        updated_by: String::new(),
    });
    note.name = name;
    let added = if body.fields.is_empty() { String::new() } else { profiles::note_block(&p, &body.fields) };
    if added.lines().count() > 1 {
        note.notes = if note.notes.trim().is_empty() { added.clone() } else { format!("{}\n\n{}", note.notes.trim_end(), added) };
    }
    if body.tone {
        note.tone = p.effective_tone();
    }
    let len = note.notes.chars().count();
    Ok((p, Planned { note, over_by: len.saturating_sub(notes::MAX_NOTE_CHARS), added }))
}

pub async fn apply_preview(State(panel): State<Panel>, Path(id): Path<String>, body: axum::body::Bytes) -> ApiResult {
    let id = user_id(&id)?;
    let body: ApplyBody = serde_json::from_slice(&body).map_err(|e| ApiError::bad(format!("Send fields and tone: {}", e)))?;
    let (_, plan) = plan_apply(&panel, id, &body).await?;
    let name = plan.note.name.clone();
    ok(json!({
        "note": plan.note.notes,
        "tone": plan.note.tone,
        "chars": plan.note.notes.chars().count(),
        "max": notes::MAX_NOTE_CHARS,
        "over_by": plan.over_by,
        "fits": plan.over_by == 0,
        "added": plan.added,
        "context_block": notes::preview(&plan.note, &name),
    }))
}

pub async fn apply(
    State(panel): State<Panel>,
    Path(id): Path<String>,
    axum::Extension(Caller(user)): axum::Extension<Caller>,
    body: axum::body::Bytes,
) -> ApiResult {
    let id = user_id(&id)?;
    let body: ApplyBody = serde_json::from_slice(&body).map_err(|e| ApiError::bad(format!("Send fields and tone: {}", e)))?;
    let (p, plan) = plan_apply(&panel, id, &body).await?;
    if plan.over_by > 0 {
        return Err(ApiError::bad(format!(
            "The note would be {} characters over the {} limit. Trim the note or pick fewer fields.",
            plan.over_by,
            notes::MAX_NOTE_CHARS
        )));
    }
    let saved = notes::save(&plan.note, user).map_err(ApiError::internal)?;
    let mut fields: Vec<&str> = body.fields.iter().filter_map(|f| profiles::FIELDS.iter().find(|(x, _)| x == f).map(|(x, _)| *x)).collect();
    if body.tone {
        fields.push("suggested_tone");
    }
    profiles::save(&p, user, "applied", &fields).map_err(ApiError::internal)?;
    ok(json!({ "note": saved, "context_block": notes::context_block(&[(id, saved.name.clone())]) }))
}

/// The exact prompt that would be sent for a member now: for checking the basis.
pub async fn prompt_preview(State(panel): State<Panel>, Path(id): Path<String>) -> ApiResult {
    let id = user_id(&id)?;
    let name = match panel.data.cached_member(id) {
        Some(m) => m.name,
        None => panel.data.member(id).await.map(|m| m.name).ok_or_else(|| ApiError::not_found("No such member in the server."))?,
    };
    let (prompt, count, chars, _) = prepare(&panel, id, &name, chrono::Utc::now().timestamp()).await;
    ok(json!({ "prompt": prompt, "messages": count, "transcript_chars": chars, "prompt_chars": prompt.chars().count() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_requests_carry_channel_and_dm_flag() {
        let data = json!({ "content": { "Request": {
            "user": "@Riya (DiscordId: 42)",
            "content": { "silent_read": "koto was hard" },
            "metadata": { "discord_channel_id": "555", "is_dm": false }
        }}})
        .to_string();
        let m = parse_message("discord__555", 1_789_000_000_000, &data, 42).unwrap();
        assert_eq!((m.channel_id, m.is_dm, m.text.as_str()), (Some(555), false, "koto was hard"));
        let dm = data.replace("\"is_dm\":false", "\"is_dm\":true");
        assert!(parse_message("discord__9", 0, &dm, 42).unwrap().is_dm);
        assert!(parse_message("discord__555", 0, &data, 7).is_none());
    }

    #[test]
    fn game_points_leave_out_mods_and_weekly() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(super::super::super::super::points::SCHEMA).unwrap();
        let add = |u: i64, house: &str, source: &str, pts: i64, ts: i64| {
            conn.execute("INSERT INTO ledger (user_id, house, source, points, day, ts) VALUES (?1, ?2, ?3, ?4, 'd', ?5)", params![u, house, source, pts, ts])
                .unwrap();
        };
        add(1, "gryffindor", "quiz", 5, 100);
        add(1, "gryffindor", "mod", 50, 100);
        add(1, "gryffindor", "weekly", 3, 100);
        add(2, "slytherin", "snitch", 4, 10);
        let mut got = HashMap::new();
        ledger_points(&conn, 50, &mut |u, h, n| {
            got.insert(u, (h, n));
        })
        .unwrap();
        assert_eq!(got.get(&1), Some(&("gryffindor".to_string(), 5)));
        assert!(!got.contains_key(&2), "before the window");
    }
}
