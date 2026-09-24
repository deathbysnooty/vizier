//! Deleted and edited messages on the panel: the log `msglog.rs` keeps, a page
//! at a time, newest first, with member, channel, period and text filters, and
//! the saved pictures of deleted messages. Every look is in the activity log.

use std::collections::{HashMap, HashSet};

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::super::msglog::{self, ListFilter, Place};
use super::super::super::weekly;
use super::{ApiError, ApiResult, Caller, ChannelInfo, Panel, ok, parse_id, search};

pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 100;
pub const MAX_QUERY: usize = 100;
pub const PERIODS: [(&str, i64); 5] = [("1", 1), ("7", 7), ("30", 30), ("90", 90), ("365", 365)];
const DAY_MS: i64 = 86_400_000;

#[derive(Deserialize)]
pub struct LogQuery {
    #[serde(default)]
    member: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    days: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    before: Option<String>,
    #[serde(default)]
    limit: Option<String>,
}

struct Asked {
    member: Option<u64>,
    channel: Option<u64>,
    days_key: String,
    days: i64,
    q: Option<String>,
    before: Option<i64>,
    limit: usize,
}

fn read_query(panel: &Panel, q: &LogQuery) -> Result<Asked, ApiError> {
    let blank = |v: &Option<String>| v.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(String::from);
    let member = match blank(&q.member) {
        Some(raw) => Some(parse_id(&raw).ok_or_else(|| ApiError::bad("That isn't a member id."))?),
        None => None,
    };
    let channel = match blank(&q.channel) {
        Some(raw) => {
            let id = parse_id(&raw).ok_or_else(|| ApiError::bad("That isn't a channel id."))?;
            let channels = panel.data.channels();
            let name = channels.iter().find(|c| c.id == id.to_string()).map(|c| c.name.clone()).unwrap_or_default();
            if never_shown(panel).contains(&id) || weekly::is_safe_corner(id, &name) {
                return Err(ApiError::bad("That channel is never logged."));
            }
            Some(id)
        }
        None => None,
    };
    let days_key = blank(&q.days).unwrap_or_else(|| "30".to_string());
    let days = PERIODS.iter().find(|(k, _)| *k == days_key).map(|(_, d)| *d).ok_or_else(|| ApiError::bad("The period is 1, 7, 30, 90 or 365 days."))?;
    let text = blank(&q.q);
    if text.as_ref().is_some_and(|t| t.chars().count() > MAX_QUERY) {
        return Err(ApiError::bad(format!("Keep the search under {} characters.", MAX_QUERY)));
    }
    let before = match blank(&q.before) {
        Some(raw) => Some(raw.parse::<i64>().ok().filter(|b| *b > 0).ok_or_else(|| ApiError::bad("That isn't a place to carry on from."))?),
        None => None,
    };
    let limit = match blank(&q.limit) {
        Some(raw) => raw.parse::<usize>().map_err(|_| ApiError::bad("The limit is a number."))?.clamp(1, MAX_LIMIT),
        None => DEFAULT_LIMIT,
    };
    Ok(Asked { member, channel, days_key, days, q: text, before, limit })
}

/// Channels whose messages are never shown on either page: the ones no analysis
/// may read, and the ones the owner has taken off the log. A channel added to
/// the skip list hides what was kept from it before it was added, too.
pub(super) fn never_shown(panel: &Panel) -> Vec<u64> {
    let mut out = panel.data.sensitive_channels();
    out.extend(msglog::skip_channels());
    out.sort_unstable();
    out.dedup();
    out
}

fn matches(text: &str, q: &str) -> bool {
    search::find_ci(text, q).is_some()
}

fn filter(panel: &Panel, a: &Asked) -> ListFilter {
    ListFilter {
        member: a.member,
        channel: a.channel,
        since_ms: chrono::Utc::now().timestamp_millis() - a.days * DAY_MS,
        before: a.before,
        q: a.q.clone(),
        matches,
        limit: a.limit,
        sensitive: never_shown(panel),
    }
}

/// Writes the look to the activity log (once per 15 minutes for the same one) and
/// names the member and channel asked for.
async fn log_look(panel: &Panel, user: u64, what: &str, a: &Asked) -> (Option<String>, Option<String>) {
    let member_name = match a.member {
        Some(id) => Some(super::members::name_or_id(panel, id).await),
        None => None,
    };
    let channel_name = a.channel.map(|c| panel.data.channels().iter().find(|x| x.id == c.to_string()).map(|x| x.name.clone()).unwrap_or_else(|| format!("channel {}", c)));
    let mut parts = vec![what.to_string()];
    if let Some(q) = &a.q {
        parts.push(format!("“{}”", q));
    }
    parts.push(member_name.as_ref().map(|n| format!("@{}", n)).unwrap_or_else(|| "any member".into()));
    parts.push(channel_name.as_ref().map(|n| format!("#{}", n)).unwrap_or_else(|| "all channels".into()));
    parts.push(if a.days == 1 { "last day".to_string() } else { format!("last {} days", a.days) });
    search::log_quietly(&format!("msglog:{}", what.to_lowercase()), user, &parts.join(" · "));
    (member_name, channel_name)
}

/// Where a row was posted, as the page shows it, unless it's somewhere never shown.
pub(super) fn channel_json(place: &Place, channels: &[ChannelInfo], sensitive: &[u64]) -> Option<Value> {
    let listed = |id: u64| channels.iter().find(|c| c.id == id.to_string());
    let parent_name = place.parent_id.and_then(|p| listed(p).map(|c| c.name.clone()));
    if msglog::excluded(place, parent_name.as_deref(), sensitive) {
        return None;
    }
    let name = listed(place.channel_id).map(|c| c.name.clone()).unwrap_or_else(|| place.channel_name.clone());
    let voice = listed(place.channel_id).is_some_and(|c| c.kind == super::ChannelKind::Voice);
    Some(json!({
            "id": place.channel_id.to_string(),
            "name": name,
            "thread": place.parent_id.is_some(),
            "voice": voice,
            "gone": listed(place.channel_id).is_none() && place.parent_id.is_none(),
            "parent": place.parent_id.map(|p| json!({ "id": p.to_string(), "name": parent_name.clone().unwrap_or_default() })),
    }))
}

pub(super) async fn houses_for(panel: &Panel, ids: Vec<u64>) -> HashMap<u64, &'static super::super::super::house::House> {
    let ids: Vec<u64> = ids.into_iter().collect::<HashSet<_>>().into_iter().collect();
    let data = panel.data.clone();
    tokio::task::spawn_blocking(move || ids.into_iter().filter_map(|id| data.member_house(id).map(|h| (id, h))).collect()).await.unwrap_or_default()
}

pub(super) fn member_json(panel: &Panel, id: u64, stored_name: &str, stored_avatar: &str) -> Value {
    let who = panel.data.cached_member(id);
    json!({
        "id": id.to_string(),
        "name": who.as_ref().map(|m| m.name.clone()).unwrap_or_else(|| stored_name.to_string()),
        "avatar": who.as_ref().map(|m| m.avatar.clone()).or_else(|| (!stored_avatar.is_empty()).then(|| stored_avatar.to_string())),
        "in_server": who.is_some(),
    })
}

pub(super) fn house_json(h: Option<&&'static super::super::super::house::House>) -> Value {
    h.map(|h| json!({ "key": h.key, "name": h.name, "crest": h.crest, "colour": format!("#{:06x}", h.colour) })).unwrap_or(Value::Null)
}

pub(super) fn unavailable(e: anyhow::Error) -> ApiError {
    tracing::warn!("panel: the message log couldn't be read: {}", e);
    ApiError(StatusCode::SERVICE_UNAVAILABLE, "The message log can't be read right now. Try again in a moment.".into())
}

fn envelope(a: &Asked, names: (Option<String>, Option<String>), results: Vec<Value>, next_before: Option<i64>) -> Value {
    json!({
        "days": a.days_key,
        "limit": a.limit,
        "q": a.q,
        "member": a.member.map(|m| json!({ "id": m.to_string(), "name": names.0 })),
        "channel": a.channel.map(|c| json!({ "id": c.to_string(), "name": names.1 })),
        "count": results.len(),
        "results": results,
        "next_before": next_before,
        "keep_days": msglog::keep_days(),
        "log_days": msglog::log_days(),
        "enabled": msglog::enabled(),
    })
}

pub async fn deleted(State(panel): State<Panel>, axum::Extension(Caller(user)): axum::Extension<Caller>, Query(q): Query<LogQuery>) -> ApiResult {
    let asked = read_query(&panel, &q)?;
    let names = log_look(&panel, user, "Deleted", &asked).await;
    let page = panel.data.msglog_deleted(filter(&panel, &asked)).await.map_err(unavailable)?;
    let channels = panel.data.channels();
    let sensitive = never_shown(&panel);
    let houses = houses_for(&panel, page.rows.iter().filter_map(|r| r.author_id).collect()).await;
    let guild = panel.data.guild().map(|g| g.id);
    let results: Vec<Value> = page
        .rows
        .iter()
        .filter_map(|r| {
            let place = Place { channel_id: r.channel_id, parent_id: r.parent_id, channel_name: r.channel_name.clone() };
            let shown = channel_json(&place, &channels, &sensitive)?;
            let saved: HashSet<&str> = r.files.iter().map(|f| f.name.as_str()).collect();
            Some(json!({
                "id": r.id,
                "message_id": r.message_id.to_string(),
                "member": r.author_id.map(|a| member_json(&panel, a, r.author_name.as_deref().unwrap_or(""), r.avatar.as_deref().unwrap_or(""))),
                "house": house_json(r.author_id.and_then(|a| houses.get(&a))),
                "channel": shown,
                "sent_ts": r.created_ms / 1000,
                "deleted_ts": r.deleted_ms / 1000,
                "text": r.content,
                "reason": r.reason,
                "bulk": r.bulk,
                // Who deleted it: the audit log's executor, or (checked, none) the author or unknown.
                "deleter": r.deleter,
                "deleter_checked": r.deleter_checked,
                "reply_to": r.reply_to.map(|id| json!({ "id": id.to_string(), "author": r.reply_author, "text": r.reply_text })),
                "images": r.files.iter().map(|f| json!({ "n": f.n, "name": f.name, "url": format!("/api/msglog/file/{}/{}", r.message_id, f.n) })).collect::<Vec<_>>(),
                "files": r.attachments.iter().filter(|a| !saved.contains(a.filename.as_str())).map(|a| json!({
                    "name": a.filename,
                    "size": a.size,
                    "image": msglog::image_ext(a.content_type.as_deref(), &a.filename).is_some(),
                })).collect::<Vec<_>>(),
                // The message is gone: the link opens the channel (a thread opens itself).
                "url": guild.as_ref().map(|g| format!("https://discord.com/channels/{}/{}", g, r.channel_id)),
            }))
        })
        .collect();
    ok(envelope(&asked, names, results, page.next_before))
}

pub async fn edited(State(panel): State<Panel>, axum::Extension(Caller(user)): axum::Extension<Caller>, Query(q): Query<LogQuery>) -> ApiResult {
    let asked = read_query(&panel, &q)?;
    let names = log_look(&panel, user, "Edited", &asked).await;
    let page = panel.data.msglog_edited(filter(&panel, &asked)).await.map_err(unavailable)?;
    let channels = panel.data.channels();
    let sensitive = never_shown(&panel);
    let houses = houses_for(&panel, page.rows.iter().map(|r| r.author_id).collect()).await;
    let guild = panel.data.guild().map(|g| g.id);
    let results: Vec<Value> = page
        .rows
        .iter()
        .filter_map(|r| {
            let place = Place { channel_id: r.channel_id, parent_id: r.parent_id, channel_name: r.channel_name.clone() };
            let shown = channel_json(&place, &channels, &sensitive)?;
            Some(json!({
                "id": r.id,
                "message_id": r.message_id.to_string(),
                "member": member_json(&panel, r.author_id, &r.author_name, &r.avatar),
                "house": house_json(houses.get(&r.author_id)),
                "channel": shown,
                "sent_ts": r.created_ms / 1000,
                "edited_ts": r.edited_ms / 1000,
                "before": r.before,
                "after": r.after,
                "url": guild.as_ref().map(|g| search::jump_url(g, r.channel_id, r.message_id)),
            }))
        })
        .collect();
    ok(envelope(&asked, names, results, page.next_before))
}

/// Messages Discord's AutoMod blocked: never seen in the channel they were
/// aimed at, so the alert is the only record. Filed under that channel.
pub async fn blocked(State(panel): State<Panel>, axum::Extension(Caller(user)): axum::Extension<Caller>, Query(q): Query<LogQuery>) -> ApiResult {
    let asked = read_query(&panel, &q)?;
    let names = log_look(&panel, user, "Blocked", &asked).await;
    let page = panel.data.msglog_blocked(filter(&panel, &asked)).await.map_err(unavailable)?;
    let channels = panel.data.channels();
    let sensitive = never_shown(&panel);
    let houses = houses_for(&panel, page.rows.iter().map(|r| r.author_id).collect()).await;
    let guild = panel.data.guild().map(|g| g.id);
    let results: Vec<Value> = page
        .rows
        .iter()
        .filter_map(|r| {
            let place = Place { channel_id: r.channel_id, parent_id: r.parent_id, channel_name: r.channel_name.clone() };
            let shown = channel_json(&place, &channels, &sensitive)?;
            Some(json!({
                "id": r.message_id.to_string(),
                "message_id": r.message_id.to_string(),
                "member": member_json(&panel, r.author_id, &r.author_name, &r.avatar),
                "house": house_json(houses.get(&r.author_id)),
                "channel": shown,
                "sent_ts": r.created_ms / 1000,
                "text": r.content,
                "rule": r.rule_name,
                "keyword": r.keyword,
                "matched": r.matched,
                "outcome": r.outcome,
                // It never reached the channel: the link opens the channel it was aimed at.
                "url": guild.as_ref().map(|g| format!("https://discord.com/channels/{}/{}", g, r.channel_id)),
            }))
        })
        .collect();
    let mut out = envelope(&asked, names, results, page.next_before);
    out["text_days"] = json!(msglog::text_days());
    ok(out)
}

/// A saved picture of a deleted message, for admins only.
pub async fn file(State(panel): State<Panel>, Path((id, n)): Path<(String, String)>) -> ApiResult {
    let id = parse_id(&id).ok_or_else(|| ApiError::bad("That isn't a message id."))?;
    let n = (n.len() == 1).then(|| n.parse::<usize>().ok()).flatten().filter(|n| *n < msglog::MAX_IMAGES).ok_or_else(|| ApiError::bad("That isn't a picture number."))?;
    let (bytes, kind) = panel.data.msglog_file(id, n).await.ok_or_else(|| ApiError::not_found("No such picture."))?;
    let mut res = Response::new(Body::from(bytes));
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=3600"));
    h.insert(header::CONTENT_DISPOSITION, HeaderValue::from_static("inline"));
    Ok(res.into_response())
}

/// How a look at the log reads in the activity log.
pub fn audit_entry(e: &super::super::AuditEntry) -> serde_json::Map<String, Value> {
    let mut obj = serde_json::Map::new();
    let label = match e.key.as_str() {
        "msglog:edited" => "Looked at edited messages",
        "msglog:blocked" => "Looked at messages AutoMod blocked",
        _ => "Looked at deleted messages",
    };
    obj.insert("label".into(), json!(label));
    obj.insert("section".into(), json!({ "id": "msglog", "title": "Deleted messages", "icon": "🗑️" }));
    let change = e.new.clone().unwrap_or_else(|| "Looked".into());
    let change = change.split_once(" · ").map(|(_, rest)| rest.to_string()).unwrap_or(change);
    obj.insert("change".into(), json!(change));
    obj.insert("old".into(), Value::Null);
    obj.insert("new".into(), Value::Null);
    obj
}
