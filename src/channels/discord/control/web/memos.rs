//! Members' own reminders on the panel ("remind me in 2 hours…", `/remind`):
//! see them, set one for a member, cancel one. Changes go to the activity log
//! as `memo:<id>`, without the reminder's text.
//!
//! A reminder set somewhere private - a DM, #safe-corner or a thread under it,
//! or a channel the panel can't place - is listed without its text.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::memos::{self, Memo};
use super::{ApiError, ApiResult, Caller, ChannelKind, Panel, ok};

const LIST_LIMIT: usize = 500;

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    status: String,
}

/// What the list needs to describe many reminders without asking Discord for each.
struct Lookup {
    channels: Vec<super::ChannelInfo>,
    sensitive: Vec<u64>,
    people: HashMap<String, Value>,
    /// Members not in the cache that may still be fetched.
    fetches_left: usize,
}

impl Lookup {
    fn new(panel: &Panel) -> Self {
        Self { channels: panel.data.channels(), sensitive: panel.data.sensitive_channels(), people: HashMap::new(), fetches_left: 20 }
    }

    /// Where a reminder was set, and whether its text may be shown.
    fn place(&self, panel: &Panel, channel: &str) -> (Value, bool) {
        let Ok(id) = channel.parse::<u64>() else {
            return (Value::Null, true);
        };
        let known = |cid: u64| self.channels.iter().find(|c| c.id == cid.to_string() && c.kind != ChannelKind::Category);
        if let Some(c) = known(id) {
            return (json!({ "id": c.id, "name": c.name }), self.sensitive.contains(&id));
        }
        if let Some(parent) = panel.data.thread_parent(id) {
            let name = known(parent).map(|c| c.name.clone());
            return (json!({ "id": id.to_string(), "name": name, "thread": true }), self.sensitive.contains(&parent));
        }
        // A DM, or somewhere the panel can't see: treated as private.
        (json!({ "id": id.to_string(), "name": Value::Null }), true)
    }

    async fn person(&mut self, panel: &Panel, id: &str) -> Value {
        let Some(n) = id.parse::<u64>().ok().filter(|n| *n != 0) else {
            return Value::Null;
        };
        if let Some(found) = self.people.get(id) {
            return found.clone();
        }
        let info = match panel.data.cached_member(n) {
            Some(info) => Some(info),
            None if self.fetches_left > 0 => {
                self.fetches_left -= 1;
                panel.data.member(n).await
            }
            None => None,
        };
        let value = match info {
            Some(info) => json!({ "id": id, "name": info.name, "avatar": info.avatar }),
            None => json!({ "id": id, "name": Value::Null, "avatar": Value::Null }),
        };
        self.people.insert(id.to_string(), value.clone());
        value
    }
}

async fn memo_json(panel: &Panel, look: &mut Lookup, m: &Memo, now: i64) -> Value {
    let (channel, private) = look.place(panel, &m.channel_id);
    let user = look.person(panel, &m.user_id).await;
    let set_by = if m.set_by != "0" && m.set_by != m.user_id { look.person(panel, &m.set_by).await } else { Value::Null };
    json!({
        "id": m.id,
        "user": user,
        "set_by": set_by,
        "text": if private { Value::Null } else { json!(m.text) },
        "private": private,
        "channel": channel,
        "via": m.via,
        "status": m.status,
        "due_ts": m.due_ts,
        "due_words": memos::describe(m.due_ts, now),
        "created_ts": m.created_ts,
        "sent_ts": m.sent_ts,
    })
}

pub async fn list(State(panel): State<Panel>, Query(q): Query<ListQuery>) -> ApiResult {
    let now = chrono::Utc::now().timestamp();
    let all = memos::all(LIST_LIMIT);
    let pending = all.iter().filter(|m| m.status == "pending").count();
    let wanted: Vec<&Memo> = match q.status.as_str() {
        "" | "pending" => all.iter().filter(|m| m.status == "pending").collect(),
        "done" => all.iter().filter(|m| m.status != "pending").collect(),
        "all" => all.iter().collect(),
        _ => return Err(ApiError::bad("Status is pending, done or all.")),
    };
    let mut look = Lookup::new(&panel);
    let mut items = Vec::with_capacity(wanted.len());
    for m in wanted {
        items.push(memo_json(&panel, &mut look, m, now).await);
    }
    ok(json!({
        "enabled": super::super::on("VIZIER_MEMBER_REMINDERS", true),
        "pending": pending,
        "items": items,
    }))
}

#[derive(Deserialize)]
pub struct WhenQuery {
    #[serde(default)]
    text: String,
}

fn unreadable(when: &str) -> String {
    format!(
        "I couldn't read “{}” as a time. Try “in 2 hours”, “30m”, “at 9pm”, “tomorrow 9am” or “2026-09-20 18:00” (India time).",
        when.trim()
    )
}

/// What a "when" reads as, for the form's live preview.
pub async fn when(Query(q): Query<WhenQuery>) -> ApiResult {
    let now = chrono::Utc::now().timestamp();
    let text: String = q.text.chars().take(80).collect();
    if text.trim().is_empty() {
        return ok(json!({ "ok": false, "error": Value::Null }));
    }
    match memos::parse_when(&text, now) {
        Some(due) => ok(json!({ "ok": true, "due_ts": due, "words": memos::describe(due, now), "in_secs": due - now })),
        None => ok(json!({ "ok": false, "error": unreadable(&text) })),
    }
}

#[derive(Deserialize)]
pub struct CreateBody {
    #[serde(default)]
    user_id: String,
    #[serde(default)]
    channel_id: String,
    #[serde(default)]
    when: String,
    #[serde(default)]
    text: String,
}

pub async fn create(
    State(panel): State<Panel>,
    axum::Extension(Caller(admin)): axum::Extension<Caller>,
    body: axum::body::Bytes,
) -> ApiResult {
    let body: CreateBody = serde_json::from_slice(&body).map_err(|_| ApiError::bad("That isn't valid JSON."))?;
    let user = super::parse_id(&body.user_id).ok_or_else(|| ApiError::bad("Pick the member to remind."))?;
    if panel.data.member(user).await.is_none() {
        return Err(ApiError::bad("That member isn't in the server."));
    }
    if body.channel_id.trim().is_empty() {
        return Err(ApiError::bad("Pick the channel the reminder goes to."));
    }
    let channel: u64 = super::check_channel(&panel, &body.channel_id, false).map_err(ApiError::bad)?.parse().unwrap_or(0);
    if body.when.trim().is_empty() {
        return Err(ApiError::bad("Say when, like “in 2 hours” or “tomorrow 9am”."));
    }
    let now = chrono::Utc::now().timestamp();
    let due = memos::parse_when(&body.when, now).ok_or_else(|| ApiError::bad(unreadable(&body.when)))?;
    let memo = memos::create_by_admin(user, channel, &body.text, due, admin).map_err(|e| {
        ApiError::bad(if user != admin { e.replace("You already have", "They already have") } else { e })
    })?;
    let facts = json!({ "action": "created", "user_id": memo.user_id, "channel_id": memo.channel_id, "due_ts": memo.due_ts, "via": "panel" });
    super::super::log_change(&format!("memo:{}", memo.id), None, Some(&facts.to_string()), admin).map_err(ApiError::internal)?;
    tracing::info!("panel: {} set member reminder {} for {}", admin, memo.id, user);
    Ok((StatusCode::CREATED, axum::Json(memo_json(&panel, &mut Lookup::new(&panel), &memo, now).await)).into_response())
}

pub async fn cancel(
    State(panel): State<Panel>,
    Path(id): Path<String>,
    axum::Extension(Caller(admin)): axum::Extension<Caller>,
) -> ApiResult {
    let id = id.parse::<i64>().ok().filter(|n| *n > 0).ok_or_else(|| ApiError::not_found("No such reminder."))?;
    let memo = memos::get(id).ok_or_else(|| ApiError::not_found("No such reminder."))?;
    if memo.status != "pending" {
        return Err(ApiError(StatusCode::CONFLICT, format!("That reminder was already {}.", if memo.status == "failed" { "given up on" } else { memo.status.as_str() })));
    }
    if !memos::cancel(id, None) {
        return Err(ApiError(StatusCode::CONFLICT, "That reminder just went out or was cancelled.".into()));
    }
    let facts = json!({ "action": "cancelled", "user_id": memo.user_id, "due_ts": memo.due_ts });
    super::super::log_change(&format!("memo:{}", id), Some(&facts.to_string()), None, admin).map_err(ApiError::internal)?;
    tracing::info!("panel: {} cancelled member reminder {}", admin, id);
    let now = chrono::Utc::now().timestamp();
    let fresh = memos::get(id).unwrap_or(memo);
    ok(memo_json(&panel, &mut Lookup::new(&panel), &fresh, now).await)
}
