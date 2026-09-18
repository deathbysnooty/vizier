//! The Messages page: what a member has said, newest first, across every
//! channel the bot can see, and a search inside it.
//!
//! This reads `msglog`'s own store — a copy of every message in every channel
//! except #safe-corner and the skip list, kept for `VIZIER_MSGLOG_TEXT_DAYS`.
//! That is a different store from the one [`super::search`] reads, which holds
//! the AI's stored history: forever, but only for the channels the bot chats in.
//! The two are never mixed into one list; the page asks for one at a time and
//! says which, because a message missing from a search means something quite
//! different in each.
//!
//! Admins only, like every panel page, and every look goes in the activity log,
//! at most once every fifteen minutes for the same one.

use axum::extract::{Query, State};
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::super::msglog::{self, SaidFilter, SaidRow};
use super::super::super::weekly;
use super::msglog::never_shown;
use super::{ApiError, ApiResult, Caller, Panel, ok, parse_id, search};

pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 100;
const DAY_MS: i64 = 86_400_000;

#[derive(Deserialize)]
pub struct MessagesQuery {
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
    days: Option<i64>,
    q: Option<String>,
    before: Option<u64>,
    limit: usize,
}

fn read_query(panel: &Panel, q: &MessagesQuery) -> Result<Asked, ApiError> {
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
                return Err(ApiError::bad("That channel is never kept."));
            }
            Some(id)
        }
        None => None,
    };
    let days_key = blank(&q.days).unwrap_or_else(|| "30".to_string());
    let days = search::PERIODS
        .iter()
        .find(|(k, _)| *k == days_key)
        .map(|(_, d)| *d)
        .ok_or_else(|| ApiError::bad("The period is 1, 7, 30, 90 or all (days)."))?;
    let text = blank(&q.q);
    if let Some(t) = &text {
        let len = t.chars().count();
        if len < search::MIN_QUERY {
            return Err(ApiError::bad(format!("Type at least {} characters to search.", search::MIN_QUERY)));
        }
        if len > search::MAX_QUERY {
            return Err(ApiError::bad(format!("Keep the search under {} characters.", search::MAX_QUERY)));
        }
    }
    let before = match blank(&q.before) {
        Some(raw) => Some(raw.parse::<u64>().ok().filter(|b| *b > 0).ok_or_else(|| ApiError::bad("That isn't a place to carry on from."))?),
        None => None,
    };
    let limit = match blank(&q.limit) {
        Some(raw) => raw.parse::<usize>().map_err(|_| ApiError::bad("The limit is a number."))?.clamp(1, MAX_LIMIT),
        None => DEFAULT_LIMIT,
    };
    Ok(Asked { member, channel, days_key, days, q: text, before, limit })
}

fn matches(text: &str, q: &str) -> bool {
    search::find_ci(text, q).is_some()
}

/// Writes the look to the activity log, naming who and where was asked for.
/// A search says so; a plain read of someone's messages reads as a look.
async fn log_look(panel: &Panel, user: u64, a: &Asked) -> (Option<String>, Option<String>) {
    let member_name = match a.member {
        Some(id) => Some(match panel.data.cached_member(id) {
            Some(m) => m.name,
            None => panel.data.member(id).await.map(|m| m.name).unwrap_or_else(|| id.to_string()),
        }),
        None => None,
    };
    let channels = panel.data.channels();
    let channel_name = a.channel.map(|c| channels.iter().find(|x| x.id == c.to_string()).map(|x| x.name.clone()).unwrap_or_else(|| format!("channel {}", c)));
    let mut parts = Vec::new();
    if let Some(q) = &a.q {
        parts.push(format!("“{}”", q));
    }
    parts.push(member_name.as_ref().map(|n| format!("@{}", n)).unwrap_or_else(|| "any member".into()));
    parts.push(channel_name.as_ref().map(|n| format!("#{}", n)).unwrap_or_else(|| "all channels".into()));
    parts.push(match a.days {
        Some(1) => "last day".to_string(),
        Some(d) => format!("last {} days", d),
        None => "all time".to_string(),
    });
    let key = if a.q.is_some() { "messages:search" } else { "messages:look" };
    search::log_quietly(key, user, &parts.join(" · "));
    (member_name, channel_name)
}

/// One kept message as the page shows it: the same shape a search result has, so
/// the page draws a timeline and a search with one piece of code.
fn result_json(panel: &Panel, row: &SaidRow, channel: Value, house: Value, guild: Option<&String>, q: Option<&str>) -> Value {
    // With no words to find, the snippet is simply the opening of the message,
    // so a long one is folded away the same way whether or not it was searched.
    let (start, end) = q.and_then(|q| search::find_ci(&row.content, q)).unwrap_or((0, 0));
    let (snip, s_start, s_end) = search::snippet(&row.content, start, end);
    let channel_id = row.channel_id;
    json!({
        "id": row.message_id.to_string(),
        "ts": row.created_ms / 1000,
        "ts_ms": row.created_ms,
        "member": super::msglog::member_json(panel, row.author_id, &row.author_name, &row.avatar),
        "house": house,
        "channel": channel,
        "text": row.content,
        "match": [start, end],
        "snippet": { "text": snip, "start": s_start, "end": s_end },
        "reply_to": row.reply_to.map(|id| json!({ "id": id.to_string(), "author": row.reply_author, "text": row.reply_text })),
        "files": row.attachments.iter().map(|a| json!({
            "name": a.filename,
            "size": a.size,
            "image": msglog::image_ext(a.content_type.as_deref(), &a.filename).is_some(),
        })).collect::<Vec<_>>(),
        "url": guild.map(|g| search::jump_url(g, channel_id, row.message_id)),
        "source": "log",
    })
}

/// What the store can see, so the page can say how far back it reaches instead
/// of quietly showing less than the reader expects.
fn coverage_json(panel: &Panel, cover: Option<msglog::Coverage>) -> Value {
    let listed = panel.data.channels().len();
    json!({
        "enabled": msglog::enabled(),
        "text_days": msglog::text_days(),
        "picture_days": msglog::keep_days(),
        "rows": cover.map(|c| c.rows),
        "oldest_ts": cover.and_then(|c| c.oldest_ms).map(|ms| ms / 1000),
        "newest_ts": cover.and_then(|c| c.newest_ms).map(|ms| ms / 1000),
        "skipped": msglog::skip_channels().len(),
        "channels": listed,
    })
}

pub async fn list(State(panel): State<Panel>, axum::Extension(Caller(user)): axum::Extension<Caller>, Query(q): Query<MessagesQuery>) -> ApiResult {
    let asked = read_query(&panel, &q)?;
    let (member_name, channel_name) = log_look(&panel, user, &asked).await;

    let now_ms = chrono::Utc::now().timestamp_millis();
    let since_ms = asked.days.map(|d| now_ms - d * DAY_MS).unwrap_or(0);
    let filter = SaidFilter {
        member: asked.member,
        channel: asked.channel,
        since_id: msglog::first_id_at(since_ms),
        before: asked.before,
        q: asked.q.clone(),
        matches,
        limit: asked.limit,
        budget: msglog::SAID_BUDGET,
        sensitive: never_shown(&panel),
    };
    let page = panel.data.msglog_said(filter).await.map_err(super::msglog::unavailable)?;
    let cover = panel.data.msglog_coverage().await;

    let channels = panel.data.channels();
    let sensitive = never_shown(&panel);
    let houses = super::msglog::houses_for(&panel, page.rows.iter().map(|r| r.author_id).collect()).await;
    let guild = panel.data.guild().map(|g| g.id);
    let results: Vec<Value> = page
        .rows
        .iter()
        .filter_map(|row| {
            let place = msglog::Place { channel_id: row.channel_id, parent_id: row.parent_id, channel_name: row.channel_name.clone() };
            let channel = super::msglog::channel_json(&place, &channels, &sensitive)?;
            let house = super::msglog::house_json(houses.get(&row.author_id));
            Some(result_json(&panel, row, channel, house, guild.as_ref(), asked.q.as_deref()))
        })
        .collect();

    ok(json!({
        "source": "log",
        "q": asked.q,
        "member": asked.member.map(|m| json!({ "id": m.to_string(), "name": member_name })),
        "channel": asked.channel.map(|c| json!({ "id": c.to_string(), "name": channel_name })),
        "days": asked.days_key,
        "limit": asked.limit,
        "count": results.len(),
        "results": results,
        "next_before": page.next_before.map(|b| b.to_string()),
        "complete": page.complete,
        "scanned": page.scanned,
        "coverage": coverage_json(&panel, cover),
    }))
}

/// How a look at the Messages page reads in the activity log. A search says what
/// was searched for; a plain read says whose messages were read.
pub fn audit_entry(e: &super::super::AuditEntry) -> serde_json::Map<String, Value> {
    let mut obj = serde_json::Map::new();
    let searched = e.key == "messages:search";
    obj.insert("label".into(), json!(if searched { "Searched messages" } else { "Looked at messages" }));
    obj.insert("section".into(), json!({ "id": "messages", "title": "Messages", "icon": "💬" }));
    obj.insert("change".into(), json!(e.new.clone().unwrap_or_else(|| if searched { "Searched" } else { "Looked" }.into())));
    obj.insert("old".into(), Value::Null);
    obj.insert("new".into(), Value::Null);
    obj
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A look at someone's messages and a search read differently in the log.
    #[test]
    fn the_activity_log_tells_a_look_from_a_search() {
        let entry = |key: &str| super::super::super::AuditEntry {
            ts: 0,
            user_id: "1".into(),
            key: key.into(),
            old: None,
            new: Some("@Riya · all channels · last 30 days".into()),
        };
        let look = audit_entry(&entry("messages:look"));
        assert_eq!(look["label"], json!("Looked at messages"));
        assert_eq!(look["change"], json!("@Riya · all channels · last 30 days"));
        assert_eq!(look["section"]["id"], json!("messages"));
        assert_eq!(audit_entry(&entry("messages:search"))["label"], json!("Searched messages"));
    }
}
