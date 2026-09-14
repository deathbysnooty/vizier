//! The Members page: a profile of one member drawn from everything the bot
//! keeps (Discord, house points, activity counts, games, the join log, what the
//! AI has read from them and what it remembers), plus the mods' private notes
//! the AI is handed when it answers them.

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use parking_lot::Mutex;
use regex::Regex;
use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::super::super::house;
use super::super::super::points;
use super::super::members::{self as notes, MemberNote, Tone};
use super::{ApiError, ApiResult, Caller, MemberInfo, Panel, ok, parse_id};

// --- what the data layer hands over ---------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct MemberDetail {
    pub info: MemberInfo,
    pub joined_at: Option<i64>,
    pub created_at: i64,
    pub role_ids: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct JoinSummary {
    pub joins: u32,
    pub leaves: u32,
    pub first_join: Option<String>,
    pub last_join: Option<String>,
    pub last_leave: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct MemberStats {
    pub house: Option<String>,
    pub captain: bool,
    pub muggle: bool,
    /// Source key and points, today (India day) and this month.
    pub today: Vec<(String, i64)>,
    pub month: Vec<(String, i64)>,
    pub all_time: i64,
    /// Their place among their house's scorers this month, and how many scored.
    pub house_rank: Option<(usize, usize)>,
    /// Weekly-scan points since Monday.
    pub weekly_this_week: i64,
    pub messages_today: i64,
    /// Today's messages as the chat point counts them (excluded channels left out).
    pub chat_counted_today: i64,
    pub messages_7d: i64,
    pub messages_30d: i64,
    /// Channel id and messages over 30 days, most first, up to five.
    pub top_channels: Vec<(u64, i64)>,
    /// Messages per India hour over 30 days, 24 entries.
    pub hours: Vec<i64>,
    pub voice_today_secs: i64,
    /// Voice today that counts towards voice points (with company, when that rule is on).
    #[serde(default)]
    pub voice_points_today_secs: i64,
    pub voice_7d_secs: i64,
    pub quiz_all: i64,
    pub quiz_month: i64,
    pub fights: i64,
    pub wins: i64,
    pub crowns: i64,
    pub snitch_month: i64,
    pub joins: Option<JoinSummary>,
}

/// One message of theirs from the AI's stored history.
#[derive(Clone, Debug, Serialize)]
pub struct SeenMessage {
    pub ts: i64,
    pub channel_id: Option<u64>,
    /// "chat" when the bot was asked, "silent_read" when it only read along.
    pub kind: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct MemoryEntry {
    pub slug: String,
    pub title: String,
    pub content: String,
    pub ts: i64,
    pub tags: Vec<String>,
    pub keywords: Vec<String>,
}

// --- the live readers --------------------------------------------------------------

const IST: i64 = 5 * 3600 + 30 * 60;

fn today_start(now: i64) -> i64 {
    (now + IST).div_euclid(86_400) * 86_400 - IST
}

/// Everything but Discord and the AI history, read from the bot's own databases.
/// Each database lock is taken once and dropped before the next.
pub async fn read_stats(deps: Option<&crate::dependencies::VizierDependencies>, user: u64, now: i64) -> MemberStats {
    let joins = match deps {
        Some(deps) => {
            let log = super::super::super::joinlog_get(&deps.storage, user).await;
            (log.joins > 0 || log.leaves > 0).then(|| JoinSummary {
                joins: log.joins,
                leaves: log.leaves,
                first_join: log.first_join,
                last_join: log.last_join,
                last_leave: log.last_leave,
            })
        }
        None => None,
    };
    let stats = tokio::task::spawn_blocking(move || read_stats_blocking(user, now)).await.unwrap_or_default();
    MemberStats { joins, ..stats }
}

fn read_stats_blocking(user: u64, now: i64) -> MemberStats {
    let mut out = MemberStats { hours: vec![0; 24], ..Default::default() };
    // These take the house lock themselves: before it is taken here.
    let their_house = house::house_of(user);
    let optouts = house::optout_set();
    out.muggle = optouts.contains(&user);
    out.house = their_house.map(|h| h.key.to_string());
    out.captain = their_house.is_some_and(|h| house::captain_id(h.key) == Some(user));
    let today = today_start(now);
    let month = points::month_start(now);
    let week_day = points::week_start_day(now);
    if let Some(db) = house::db() {
        let conn = db.lock();
        let _ = ledger_for(&conn, user, today, month, &week_day, their_house.map(|h| h.key), &optouts, &mut out);
    }
    if let Some(db) = super::super::super::stats::db() {
        let conn = db.lock();
        let _ = activity_for(&conn, user, now, &mut out);
    }
    (out.quiz_all, out.quiz_month) = super::super::super::quiz::points_of(user, month);
    (out.fights, out.wins, out.crowns) = super::super::super::battle::record_of(user);
    out
}

fn by_source(conn: &Connection, sql: &str, args: &[&dyn rusqlite::ToSql]) -> rusqlite::Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows: Vec<(String, i64)> =
        stmt.query_map(args, |r| Ok((r.get(0)?, r.get(1)?)))?.flatten().filter(|(_, n): &(String, i64)| *n != 0).collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    Ok(rows)
}

fn ledger_for(
    conn: &Connection,
    user: u64,
    today: i64,
    month: i64,
    week_day: &str,
    house_key: Option<&str>,
    optouts: &HashSet<u64>,
    out: &mut MemberStats,
) -> rusqlite::Result<()> {
    let u = user as i64;
    let sql = "SELECT source, SUM(points) FROM ledger WHERE user_id = ?1 AND ts >= ?2 GROUP BY source";
    out.today = by_source(conn, sql, &[&u, &today])?;
    out.month = by_source(conn, sql, &[&u, &month])?;
    out.all_time = conn.query_row("SELECT COALESCE(SUM(points), 0) FROM ledger WHERE user_id = ?1", params![u], |r| r.get(0))?;
    out.weekly_this_week = conn.query_row(
        "SELECT COALESCE(SUM(points), 0) FROM ledger WHERE user_id = ?1 AND source = 'weekly' AND day >= ?2",
        params![u, week_day],
        |r| r.get(0),
    )?;
    out.snitch_month = conn.query_row(
        "SELECT COUNT(*) FROM ledger WHERE user_id = ?1 AND source IN ('snitch', 'golden_snitch') AND points > 0 AND ts >= ?2",
        params![u, month],
        |r| r.get(0),
    )?;
    if let Some(key) = house_key {
        let scorers: Vec<u64> =
            points::top_members(conn, key, month, i64::MAX)?.into_iter().map(|(id, _)| id).filter(|id| !optouts.contains(id)).collect();
        out.house_rank = scorers.iter().position(|id| *id == user).map(|i| (i + 1, scorers.len()));
    }
    Ok(())
}

fn activity_for(conn: &Connection, user: u64, now: i64, out: &mut MemberStats) -> rusqlite::Result<()> {
    let day = |ts: i64| points::ist_day(ts);
    let today = day(now);
    let (d7, d30) = (day(now - 6 * 86_400), day(now - 29 * 86_400));
    let mut stmt = conn.prepare(
        "SELECT channel_id, day, hour, SUM(count) FROM msg_counts WHERE user_id = ?1 AND day >= ?2 GROUP BY channel_id, day, hour",
    )?;
    let mut channels: HashMap<u64, i64> = HashMap::new();
    for (channel, d, hour, n) in stmt
        .query_map(params![user as i64, d30], |r| {
            Ok((r.get::<_, i64>(0)? as u64, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?))
        })?
        .flatten()
    {
        out.messages_30d += n;
        if d >= d7 {
            out.messages_7d += n;
        }
        if d == today {
            out.messages_today += n;
        }
        *channels.entry(channel).or_insert(0) += n;
        if let Some(slot) = out.hours.get_mut(hour.clamp(0, 23) as usize) {
            *slot += n;
        }
    }
    let mut top: Vec<(u64, i64)> = channels.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1));
    top.truncate(5);
    out.top_channels = top;
    out.chat_counted_today =
        super::super::super::activity::messages_on(conn, &today, Some(user))?.get(&user).copied().unwrap_or(0);
    let start = today_start(now);
    let voice = |from: i64| {
        super::super::super::activity::voice_between(conn, Some(user), from, start + 86_400, now)
            .map(|m| m.get(&user).copied().unwrap_or(0))
    };
    out.voice_today_secs = voice(start)?;
    out.voice_points_today_secs = super::super::super::activity::voice_points_between(conn, Some(user), start, start + 86_400, now)?
        .get(&user)
        .copied()
        .unwrap_or(0);
    out.voice_7d_secs = voice(start - 6 * 86_400)?;
    Ok(())
}

/// A read-only connection of its own to vizier.db, so a slow history search
/// never holds the lock the bot writes its conversations through.
pub(super) fn history_conn(deps: &crate::dependencies::VizierDependencies) -> Option<&'static Mutex<Connection>> {
    static CONN: OnceLock<Option<Mutex<Connection>>> = OnceLock::new();
    CONN.get_or_init(|| {
        if !matches!(deps.config.storage, crate::config::storage::StorageConfig::Sqlite) {
            return None;
        }
        let path = crate::utils::build_path(&deps.config.workspace, &[".runtime", "vizier.db"]);
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()?;
        let _ = conn.busy_timeout(Duration::from_secs(3));
        Some(Mutex::new(conn))
    })
    .as_ref()
}

/// How far back the history search looks. Bounded so a member who never
/// talks can't make it read the whole table.
pub const SEEN_DAYS: i64 = 60;
pub const SEEN_LIMIT: usize = 30;

pub async fn read_seen(deps: &crate::dependencies::VizierDependencies, agent_id: &str, user: u64, now: i64) -> Vec<SeenMessage> {
    static CACHE: LazyLock<Mutex<HashMap<u64, (Instant, Vec<SeenMessage>)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
    if let Some((at, rows)) = CACHE.lock().get(&user) {
        if at.elapsed() < Duration::from_secs(60) {
            return rows.clone();
        }
    }
    let Some(conn) = history_conn(deps) else {
        return Vec::new();
    };
    let agent = agent_id.to_string();
    let rows = tokio::task::spawn_blocking(move || {
        let conn = conn.lock();
        query_seen(&conn, &agent, user, (now - SEEN_DAYS * 86_400) * 1000, SEEN_LIMIT).unwrap_or_default()
    })
    .await
    .unwrap_or_default();
    let mut cache = CACHE.lock();
    cache.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(60));
    cache.insert(user, (Instant::now(), rows.clone()));
    rows
}

/// The member's latest requests in the AI history. Uses the (agent_id,
/// timestamp) index newest first and stops at `limit` matches.
pub fn query_seen(conn: &Connection, agent_id: &str, user: u64, since_ms: i64, limit: usize) -> rusqlite::Result<Vec<SeenMessage>> {
    let mut stmt = conn.prepare(
        "SELECT channel, timestamp, data FROM session_history INDEXED BY idx_sh_agent_time
         WHERE agent_id = ?1 AND timestamp >= ?2 AND content_type = 'Request' AND data LIKE ?3
         ORDER BY timestamp DESC LIMIT ?4",
    )?;
    let pattern = format!("%(DiscordId: {})%", user);
    let rows = stmt.query_map(params![agent_id, since_ms, pattern, limit as i64], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?))
    })?;
    Ok(rows.flatten().filter_map(|(channel, ts, data)| parse_seen(&channel, ts, &data, user)).collect())
}

/// One stored request as a message: the text they sent, without the notes
/// block the bot puts in front of answered messages.
pub fn parse_seen(channel: &str, ts_ms: i64, data: &str, user: u64) -> Option<SeenMessage> {
    let v: Value = serde_json::from_str(data).ok()?;
    let req = v.get("content")?.get("Request")?;
    let who = req.get("user")?.as_str()?;
    if !who.ends_with(&format!("(DiscordId: {})", user)) {
        return None;
    }
    let content = req.get("content")?.as_object()?;
    let (kind, text) = content.iter().next()?;
    let text = text.as_str()?;
    let text = match (text.find("[Private notes from the server mods"), text.find("[End of notes]")) {
        (Some(0), Some(end)) => text[end + "[End of notes]".len()..].trim_start(),
        _ => text,
    };
    let text: String = text.trim().chars().take(800).collect();
    if text.is_empty() {
        return None;
    }
    let channel_id = channel.strip_prefix("discord__").and_then(|rest| rest.split("__").next()).and_then(|id| id.parse().ok());
    Some(SeenMessage { ts: ts_ms / 1000, channel_id, kind: kind.to_string(), text })
}

pub async fn read_memories(deps: &crate::dependencies::VizierDependencies, agent_id: &str) -> Vec<MemoryEntry> {
    use crate::storage::memory::MemoryStorage;
    static CACHE: LazyLock<Mutex<Option<(Instant, Vec<MemoryEntry>)>>> = LazyLock::new(|| Mutex::new(None));
    if let Some((at, list)) = CACHE.lock().as_ref() {
        if at.elapsed() < Duration::from_secs(60) {
            return list.clone();
        }
    }
    let list: Vec<MemoryEntry> = deps
        .storage
        .get_all_agent_memory(agent_id.to_string())
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|m| MemoryEntry {
            slug: m.slug,
            title: m.title,
            content: m.content,
            ts: m.timestamp.timestamp(),
            tags: m.tags,
            keywords: m.keywords,
        })
        .collect();
    *CACHE.lock() = Some((Instant::now(), list.clone()));
    list
}

// --- matching memories to a member ----------------------------------------------------

/// The words a memory can name a member by: their id, and each name of three
/// characters or more (display name, username), matched as a whole word.
pub fn memory_terms(id: u64, names: &[&str]) -> (String, Vec<(String, Regex)>) {
    let mut seen = HashSet::new();
    let words = names
        .iter()
        .map(|n| n.trim().trim_start_matches('@').to_string())
        .filter(|n| n.chars().count() >= 3 && seen.insert(n.to_lowercase()))
        .filter_map(|n| {
            let re = Regex::new(&format!(r"(?i)(?:^|[^\p{{L}}\p{{N}}_]){}(?:$|[^\p{{L}}\p{{N}}_])", regex::escape(&n))).ok()?;
            Some((n, re))
        })
        .collect();
    (id.to_string(), words)
}

/// What in a memory names the member, if anything.
pub fn memory_matches(entry: &MemoryEntry, terms: &(String, Vec<(String, Regex)>)) -> Vec<String> {
    let hay = format!("{}\n{}\n{}\n{}", entry.title, entry.content, entry.tags.join(" "), entry.keywords.join(" "));
    let mut found = Vec::new();
    if hay.contains(&terms.0) {
        found.push(terms.0.clone());
    }
    for (name, re) in &terms.1 {
        if re.is_match(&hay) {
            found.push(name.clone());
        }
    }
    found
}

fn snippet(content: &str, terms: &[String]) -> String {
    let lower = content.to_lowercase();
    let at = terms.iter().filter_map(|t| lower.find(&t.to_lowercase())).min().unwrap_or(0);
    let chars: Vec<(usize, char)> = content.char_indices().collect();
    let pos = chars.iter().position(|(i, _)| *i >= at).unwrap_or(0);
    let start = pos.saturating_sub(90);
    let end = (pos + 180).min(chars.len());
    let mut out: String = chars[start..end].iter().map(|(_, c)| *c).collect();
    out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("{}{}{}", if start > 0 { "…" } else { "" }, out, if end < chars.len() { "…" } else { "" })
}

// --- handlers ----------------------------------------------------------------------------

fn member_id(raw: &str) -> Result<u64, ApiError> {
    parse_id(raw).ok_or_else(|| ApiError::bad("That isn't a member id."))
}

#[derive(Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    q: String,
}

pub async fn search(State(panel): State<Panel>, Query(q): Query<SearchQuery>) -> ApiResult {
    let query: String = q.q.trim().chars().take(64).collect();
    let noted: HashSet<String> = notes::list().into_iter().map(|n| n.user_id).collect();
    let found: Vec<Value> = panel
        .data
        .search_members(&query, 25)
        .await
        .into_iter()
        .map(|m| {
            let has_note = noted.contains(&m.id);
            let mut v = json!(m);
            v["has_note"] = json!(has_note);
            v
        })
        .collect();
    ok(found)
}

pub async fn noted(State(panel): State<Panel>) -> ApiResult {
    let list: Vec<Value> = notes::list()
        .into_iter()
        .map(|n| {
            let who = n.user_id.parse::<u64>().ok().and_then(|id| panel.data.cached_member(id));
            json!({
                "user_id": n.user_id,
                "name": who.as_ref().map(|m| m.name.clone()).unwrap_or_else(|| n.name.clone()),
                "avatar": who.map(|m| m.avatar),
                "tone": n.tone,
                "use_in_replies": n.use_in_replies,
                "notes": n.notes,
                "updated_ts": n.updated_ts,
                "updated_by": n.updated_by,
                "source": n.source,
                "reviewed": n.reviewed,
                "filled_ts": n.filled_ts,
                "awaits_review": n.awaits_review(),
            })
        })
        .collect();
    ok(list)
}

fn tones() -> Value {
    let all = [
        (Tone::Normal, "Normal", "No special instruction; only the notes."),
        (Tone::Gentle, "Gentle", "Warm and kind. Never roasts them."),
        (Tone::LightRoast, "Light roast", "Friendly teasing is fine, nothing harsh."),
        (Tone::Roast, "Roast", "They enjoy it: roast freely, but nothing hurtful or personal."),
        (Tone::Respectful, "Respectful", "Polite, no jokes at their expense."),
        (Tone::Brief, "Brief", "Short replies, doesn't engage much."),
    ];
    Value::Array(
        all.iter()
            .map(|(t, label, about)| json!({ "value": t, "label": label, "about": about, "instruction": t.instruction() }))
            .collect(),
    )
}

pub async fn profile(State(panel): State<Panel>, Path(id): Path<String>) -> ApiResult {
    let id = member_id(&id)?;
    let detail = panel.data.member_detail(id).await;
    let note = notes::get(id);
    let now = chrono::Utc::now().timestamp();
    let stats = panel.data.member_stats(id, now).await;
    // Someone who left may still have points, a house or a note; anyone else is unknown.
    if detail.is_none() && note.is_none() && stats.all_time == 0 && stats.house.is_none() && stats.joins.is_none() {
        return Err(ApiError::not_found("No such member."));
    }
    let roles = panel.data.roles();
    let channels = panel.data.channels();
    let admins = panel.data.admins();
    let name = detail
        .as_ref()
        .map(|d| d.info.name.clone())
        .or_else(|| note.as_ref().map(|n| n.name.clone()))
        .unwrap_or_else(|| format!("Member {}", id));
    let house_meta = stats.house.as_deref().and_then(house::house);
    let role_list: Vec<Value> = detail
        .as_ref()
        .map(|d| {
            let mut list: Vec<&super::RoleInfo> = roles.iter().filter(|r| d.role_ids.contains(&r.id)).collect();
            list.sort_by_key(|r| std::cmp::Reverse(r.position));
            list.iter().map(|r| json!({ "id": r.id, "name": r.name, "color": r.color })).collect()
        })
        .unwrap_or_default();
    let channel_name = |cid: u64| channels.iter().find(|c| c.id == cid.to_string()).map(|c| c.name.clone());
    let today = super::scorers::activity_chips(
        &stats.today.iter().cloned().collect(),
        stats.chat_counted_today,
        stats.voice_points_today_secs,
        stats.weekly_this_week,
    );
    let sources = |list: &[(String, i64)]| list.iter().map(|(k, n)| json!({ "source": k, "points": n })).collect::<Vec<_>>();
    ok(json!({
        "id": id.to_string(),
        "name": name,
        "username": detail.as_ref().map(|d| d.info.username.clone()),
        "avatar": detail.as_ref().map(|d| d.info.avatar.clone()),
        "bot": detail.as_ref().is_some_and(|d| d.info.bot),
        "in_server": detail.is_some(),
        "admin": admins.contains(&id),
        "joined_at": detail.as_ref().and_then(|d| d.joined_at),
        "created_at": detail.as_ref().map(|d| d.created_at),
        "roles": role_list,
        "house": house_meta.map(|h| json!({ "key": h.key, "name": h.name, "crest": h.crest, "colour": format!("#{:06x}", h.colour) })),
        "captain": stats.captain,
        "muggle": stats.muggle,
        "points": {
            "today": sources(&stats.today),
            "today_total": stats.today.iter().map(|(_, n)| n).sum::<i64>(),
            "month": sources(&stats.month),
            "month_total": stats.month.iter().map(|(_, n)| n).sum::<i64>(),
            "all_time": stats.all_time,
            "house_rank": stats.house_rank.map(|(rank, of)| json!({ "rank": rank, "of": of })),
            "activities": today,
        },
        "activity": {
            "messages_today": stats.messages_today,
            "messages_7d": stats.messages_7d,
            "messages_30d": stats.messages_30d,
            "top_channels": stats.top_channels.iter().map(|(c, n)| json!({ "id": c.to_string(), "name": channel_name(*c), "messages": n })).collect::<Vec<_>>(),
            "hours": stats.hours,
            "voice_today_min": stats.voice_today_secs / 60,
            "voice_7d_min": stats.voice_7d_secs / 60,
        },
        "games": {
            "quiz_all": stats.quiz_all,
            "quiz_month": stats.quiz_month,
            "fights": stats.fights,
            "wins": stats.wins,
            "crowns": stats.crowns,
            "snitch_month": stats.snitch_month,
        },
        "joins": stats.joins,
        "note": note,
        "tones": tones(),
        "max_note_chars": notes::MAX_NOTE_CHARS,
    }))
}

pub async fn seen(State(panel): State<Panel>, Path(id): Path<String>) -> ApiResult {
    let id = member_id(&id)?;
    let channels = panel.data.channels();
    let rows: Vec<Value> = panel
        .data
        .member_seen(id, chrono::Utc::now().timestamp())
        .await
        .into_iter()
        .map(|m| {
            let name = m.channel_id.and_then(|c| channels.iter().find(|x| x.id == c.to_string()).map(|x| x.name.clone()));
            json!({ "ts": m.ts, "channel_id": m.channel_id.map(|c| c.to_string()), "channel": name, "kind": m.kind, "text": m.text })
        })
        .collect();
    ok(json!({ "messages": rows, "days": SEEN_DAYS, "limit": SEEN_LIMIT }))
}

pub async fn memories(State(panel): State<Panel>, Path(id): Path<String>) -> ApiResult {
    let id = member_id(&id)?;
    let who = match panel.data.cached_member(id) {
        Some(m) => Some(m),
        None => panel.data.member(id).await,
    };
    let note_name = notes::get(id).map(|n| n.name);
    let mut names: Vec<&str> = Vec::new();
    if let Some(m) = &who {
        names.push(&m.name);
        names.push(&m.username);
    }
    if let Some(n) = &note_name {
        names.push(n);
    }
    let terms = memory_terms(id, &names);
    let all = panel.data.memories().await;
    let total = all.len();
    let mut hits: Vec<Value> = all
        .iter()
        .filter_map(|m| {
            let matched = memory_matches(m, &terms);
            if matched.is_empty() {
                return None;
            }
            let content: String = m.content.chars().take(6000).collect();
            Some(json!({
                "slug": m.slug,
                "title": m.title,
                "ts": m.ts,
                "tags": m.tags,
                "matched": matched,
                "snippet": snippet(&m.content, &matched),
                "content": content,
                "truncated": m.content.chars().count() > 6000,
            }))
        })
        .collect();
    hits.sort_by(|a, b| b["ts"].as_i64().cmp(&a["ts"].as_i64()));
    hits.truncate(50);
    ok(json!({ "memories": hits, "searched": total, "terms": terms.1.iter().map(|(n, _)| n.clone()).chain(std::iter::once(terms.0.clone())).collect::<Vec<_>>() }))
}

#[derive(Deserialize)]
pub struct NoteBody {
    #[serde(default)]
    tone: Tone,
    #[serde(default)]
    notes: String,
    #[serde(default = "yes")]
    use_in_replies: bool,
}

fn yes() -> bool {
    true
}

fn note_from(body: &[u8]) -> Result<NoteBody, ApiError> {
    serde_json::from_slice(body).map_err(|e| ApiError::bad(format!("The note isn't right: {}", e)))
}

async fn display_name(panel: &Panel, id: u64) -> Result<String, ApiError> {
    match panel.data.member(id).await {
        Some(m) => Ok(m.name),
        None => notes::get(id).map(|n| n.name).ok_or_else(|| ApiError::not_found("No such member in the server.")),
    }
}

pub async fn save_note(
    State(panel): State<Panel>,
    Path(id): Path<String>,
    axum::Extension(Caller(user)): axum::Extension<Caller>,
    body: axum::body::Bytes,
) -> ApiResult {
    let id = member_id(&id)?;
    let body = note_from(&body)?;
    let name = display_name(&panel, id).await?;
    // A mod saving the note makes it theirs: the analysis never writes over it again.
    let base = notes::get(id).unwrap_or_else(|| MemberNote::blank(id, &name));
    let note = MemberNote {
        name: name.clone(),
        tone: body.tone,
        notes: body.notes.replace("\r\n", "\n"),
        use_in_replies: body.use_in_replies,
        ..base
    }
    .marked_by_mod();
    notes::validate(&note).map_err(ApiError::bad)?;
    if note.notes.trim().is_empty() && note.tone == Tone::Normal {
        return Err(ApiError::bad("Write a note or pick a tone. To remove the note, delete it."));
    }
    let saved = notes::save(&note, user).map_err(ApiError::internal)?;
    ok(json!({ "note": saved, "preview": notes::preview(&saved, &name) }))
}

pub async fn delete_note(
    State(panel): State<Panel>,
    Path(id): Path<String>,
    axum::Extension(Caller(user)): axum::Extension<Caller>,
) -> ApiResult {
    let id = member_id(&id)?;
    let _ = &panel;
    if notes::get(id).is_none() {
        return Err(ApiError::not_found("There's no note for that member."));
    }
    notes::delete(id, user).map_err(ApiError::internal)?;
    ok(json!({ "ok": true }))
}

#[derive(Deserialize)]
pub struct EnableBody {
    user_ids: Vec<String>,
}

/// Switches notes on for the bot, marking them reviewed: the review step for
/// notes the analysis filled in. Notes that are missing are reported, not made.
pub async fn enable_notes(axum::Extension(Caller(user)): axum::Extension<Caller>, body: axum::body::Bytes) -> ApiResult {
    let body: EnableBody = serde_json::from_slice(&body).map_err(|_| ApiError::bad("Send {\"user_ids\": [...]}."))?;
    if body.user_ids.is_empty() || body.user_ids.len() > 300 {
        return Err(ApiError::bad("Pick 1 to 300 members."));
    }
    let (mut enabled, mut missing) = (Vec::new(), Vec::new());
    for raw in &body.user_ids {
        let id = member_id(raw)?;
        match notes::get(id) {
            Some(n) => {
                let note = MemberNote { use_in_replies: true, reviewed: true, ..n };
                notes::save(&note, user).map_err(ApiError::internal)?;
                enabled.push(id.to_string());
            }
            None => missing.push(id.to_string()),
        }
    }
    ok(json!({ "enabled": enabled, "missing": missing }))
}

/// Exactly what the AI receives for the saved note (`context_block`), or none.
pub async fn preview_saved(State(panel): State<Panel>, Path(id): Path<String>) -> ApiResult {
    let id = member_id(&id)?;
    let name = display_name(&panel, id).await.unwrap_or_else(|_| format!("Member {}", id));
    let text = notes::context_block(&[(id, name)]);
    ok(json!({
        "text": text,
        "switched_on": super::super::on("VIZIER_MEMBER_NOTES", true),
    }))
}

/// The same for a note still being written.
pub async fn preview_draft(State(panel): State<Panel>, Path(id): Path<String>, body: axum::body::Bytes) -> ApiResult {
    let id = member_id(&id)?;
    let body = note_from(&body)?;
    let name = display_name(&panel, id).await.unwrap_or_else(|_| format!("Member {}", id));
    let note = MemberNote {
        tone: body.tone,
        notes: body.notes.chars().take(notes::MAX_NOTE_CHARS * 2).collect(),
        use_in_replies: body.use_in_replies,
        ..MemberNote::blank(id, &name)
    };
    ok(json!({
        "text": if note.use_in_replies { notes::preview(&note, &name) } else { None },
        "switched_on": super::super::on("VIZIER_MEMBER_NOTES", true),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_requests_become_messages_without_the_notes_block() {
        let data = json!({
            "uid": "x",
            "content": { "Request": {
                "timestamp": "2026-09-14T10:00:00Z",
                "user": "@Riya (DiscordId: 42)",
                "content": { "chat": "[Private notes from the server mods about people in this message. ...]\n- @Riya: RCB fan\n[End of notes]\nbot, who wins today?" },
                "metadata": {}
            }}
        })
        .to_string();
        let m = parse_seen("discord__555", 1_789_000_000_000, &data, 42).unwrap();
        assert_eq!(m.text, "bot, who wins today?");
        assert_eq!(m.kind, "chat");
        assert_eq!(m.channel_id, Some(555));
        assert_eq!(m.ts, 1_789_000_000);
        // Another member whose id merely contains 42 is not them.
        let other = data.replace("DiscordId: 42", "DiscordId: 142");
        assert!(parse_seen("discord__555", 0, &other, 42).is_none());
    }

    #[test]
    fn the_history_query_finds_only_their_requests() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE session_history (uid TEXT PRIMARY KEY, agent_id TEXT NOT NULL, channel TEXT NOT NULL, topic TEXT,
                 timestamp INTEGER NOT NULL, content_type TEXT NOT NULL, data TEXT NOT NULL);
             CREATE INDEX idx_sh_agent_time ON session_history(agent_id, timestamp);",
        )
        .unwrap();
        let row = |uid: &str, who: u64, kind: &str, text: &str, ts: i64, ctype: &str| {
            let data = json!({ "content": { "Request": { "user": format!("@x (DiscordId: {})", who), "content": { kind: text } } } }).to_string();
            conn.execute(
                "INSERT INTO session_history VALUES (?1, 'lodu', 'discord__9', NULL, ?2, ?3, ?4)",
                params![uid, ts, ctype, data],
            )
            .unwrap();
        };
        row("a", 42, "chat", "first", 1000, "Request");
        row("b", 42, "silent_read", "second", 2000, "Request");
        row("c", 7, "chat", "not them", 3000, "Request");
        row("d", 42, "chat", "too old", 10, "Request");
        let seen = query_seen(&conn, "lodu", 42, 500, 30).unwrap();
        assert_eq!(seen.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), vec!["second", "first"]);
        assert_eq!(seen[0].kind, "silent_read");
        assert_eq!(query_seen(&conn, "lodu", 42, 0, 1).unwrap().len(), 1);
    }

    #[test]
    fn memories_match_ids_and_whole_names() {
        let entry = |title: &str, content: &str| MemoryEntry {
            slug: "s".into(),
            title: title.into(),
            content: content.into(),
            ts: 0,
            tags: vec![],
            keywords: vec![],
        };
        let terms = memory_terms(42, &["Dev", "dev_k", "@Al"]);
        assert_eq!(terms.1.len(), 2, "names under three characters are left out");
        assert_eq!(memory_matches(&entry("People", "Dev loves chess."), &terms), vec!["Dev"]);
        assert!(memory_matches(&entry("Devops notes", "deploy steps"), &terms).is_empty(), "whole words only");
        assert_eq!(memory_matches(&entry("x", "user 42 said"), &terms), vec!["42"]);
        assert_eq!(memory_matches(&entry("x", "DEV_K is back"), &terms), vec!["dev_k"]);
        assert!(snippet(&"word ".repeat(100), &["Dev".into()]).len() < 300);
    }
}
