//! Richer scheduled posts on the panel: templates to start from, the values the
//! preview fills in, trying an AI prompt, sending a test post, checking the new
//! reminder fields, and how member-reminder and picture changes read in the
//! activity log.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::super::posts as post;
use super::super::reminders::{self, Reminder};
use super::{ApiError, ApiResult, Caller, Panel, ok};

const MAX_IMAGES: usize = 20;
const MAX_REACTIONS: usize = 5;
const MAX_AI_PROMPT: usize = 1000;
/// Least time between two test posts of one reminder.
const TEST_GAP: Duration = Duration::from_secs(10);

static LAST_TEST: LazyLock<Mutex<HashMap<i64, Instant>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub async fn templates() -> ApiResult {
    ok(post::templates(chrono::Utc::now().timestamp()))
}

/// What `{date}`, `{leader}` and the rest read as right now.
pub async fn placeholders(State(panel): State<Panel>) -> ApiResult {
    let now = chrono::Utc::now().timestamp();
    ok(post::preview_values(now, &panel.data.house_standings(now)))
}

#[derive(Deserialize)]
pub struct AiPreviewBody {
    #[serde(default)]
    prompt: String,
}

/// Writes one post from a prompt, the way the scheduler would, without sending it.
pub async fn ai_preview(State(panel): State<Panel>, body: axum::body::Bytes) -> ApiResult {
    let body: AiPreviewBody = serde_json::from_slice(&body).map_err(|_| ApiError::bad("Send {\"prompt\": \"...\"}."))?;
    let prompt = body.prompt.trim();
    if prompt.is_empty() {
        return Err(ApiError::bad("Write what the AI should post first."));
    }
    if prompt.chars().count() > MAX_AI_PROMPT {
        return Err(ApiError::bad(format!("Keep the AI prompt under {} characters.", MAX_AI_PROMPT)));
    }
    let now = chrono::Utc::now().timestamp();
    let table = if post::needs_houses([prompt]) { panel.data.house_standings(now) } else { Vec::new() };
    let mut roll = || rand::random::<f64>();
    let filled = post::fill_extras(prompt, now, &table, &mut roll);
    let asked = tokio::time::timeout(Duration::from_secs(90), panel.data.ask_model(post::ai_prompt(&filled, &[], now))).await;
    match asked {
        Ok(Ok((reply, model))) => match post::sanitise_ai(&reply) {
            Some(text) => ok(json!({ "text": text, "model": model })),
            None => Err(ApiError::bad("The AI didn't give a usable post. When that happens, one of the lines goes out instead.")),
        },
        Ok(Err(err)) => {
            tracing::warn!("panel: AI preview failed: {}", err);
            Err(ApiError::bad("The AI isn't answering right now. When that happens, one of the lines goes out instead."))
        }
        Err(_) => Err(ApiError::bad("The AI took too long. When that happens, one of the lines goes out instead.")),
    }
}

/// "Send a test now": posts the saved reminder once without counting it.
pub async fn send_test(
    State(panel): State<Panel>,
    Path(id): Path<String>,
    axum::Extension(Caller(user)): axum::Extension<Caller>,
) -> ApiResult {
    let id = super::reminder_id(&id)?;
    let r = reminders::get(id).ok_or_else(|| ApiError::not_found("No such reminder."))?;
    {
        let mut last = LAST_TEST.lock();
        if last.get(&id).is_some_and(|at| at.elapsed() < TEST_GAP) {
            return Err(ApiError(axum::http::StatusCode::TOO_MANY_REQUESTS, "A test just went out. Give it a few seconds.".into()));
        }
        last.insert(id, Instant::now());
    }
    tracing::info!("panel: {} sent a test of reminder {}", user, id);
    panel.data.send_test_post(&r).await.map_err(ApiError::bad)?;
    ok(json!({ "ok": true, "channel_id": r.channel_id }))
}

/// Checks and tidies the picture, card, reaction and AI fields.
pub fn check_extras(r: &mut Reminder) -> Result<(), ApiError> {
    let mut images: Vec<String> = Vec::new();
    for id in r.images.iter().map(|i| i.trim()).filter(|i| !i.is_empty()) {
        if super::super::media::info(id).is_none() {
            return Err(ApiError::bad("One of the pictures is no longer in the library. Remove it and pick another."));
        }
        if !images.iter().any(|i| i == id) {
            images.push(id.to_string());
        }
    }
    if images.len() > MAX_IMAGES {
        return Err(ApiError::bad(format!("Use at most {} pictures.", MAX_IMAGES)));
    }
    r.images = images;

    r.title = r.title.trim().to_string();
    r.footer = r.footer.trim().to_string();
    if r.title.chars().count() > 256 {
        return Err(ApiError::bad("Keep the card title under 256 characters."));
    }
    if r.footer.chars().count() > 300 {
        return Err(ApiError::bad("Keep the card footer under 300 characters."));
    }
    let colour = r.colour.trim();
    r.colour = if colour.is_empty() {
        String::new()
    } else {
        let value = post::parse_colour(colour).ok_or_else(|| ApiError::bad("The card colour should look like #8b93ff."))?;
        format!("#{:06x}", value)
    };

    let mut reactions: Vec<String> = Vec::new();
    for e in r.reactions.iter().map(|e| e.trim()).filter(|e| !e.is_empty()) {
        if super::super::autoreplies::reaction(e).is_none() {
            return Err(ApiError::bad(format!("“{}” isn't an emoji the bot can react with.", e)));
        }
        if !reactions.iter().any(|x| x == e) {
            reactions.push(e.to_string());
        }
    }
    if reactions.len() > MAX_REACTIONS {
        return Err(ApiError::bad(format!("Use at most {} reactions.", MAX_REACTIONS)));
    }
    r.reactions = reactions;

    r.ai_prompt = r.ai_prompt.trim().to_string();
    if r.ai_prompt.chars().count() > MAX_AI_PROMPT {
        return Err(ApiError::bad(format!("Keep the AI prompt under {} characters.", MAX_AI_PROMPT)));
    }
    Ok(())
}

/// How a member-reminder (`memo:<id>`) or picture (`media:<id>`) change reads.
/// Never the reminder's text.
pub fn audit_entry(panel: &Panel, e: &super::super::AuditEntry) -> Map<String, Value> {
    let body = |b: &Option<String>| b.as_deref().and_then(|t| serde_json::from_str::<Value>(t).ok()).unwrap_or(Value::Null);
    let (old, new) = (body(&e.old), body(&e.new));
    let facts = if new.is_null() { &old } else { &new };
    let mut obj = Map::new();
    obj.insert("section".into(), json!({ "id": "reminders", "title": "Reminders", "icon": "⏰" }));
    obj.insert("old".into(), Value::Null);
    obj.insert("new".into(), Value::Null);
    if e.key.starts_with("memo:") {
        let uid = facts.get("user_id").and_then(Value::as_str).unwrap_or("");
        let name = uid
            .parse::<u64>()
            .ok()
            .and_then(|id| panel.data.cached_member(id))
            .map(|m| m.name)
            .unwrap_or_else(|| if uid.is_empty() { "a member".to_string() } else { uid.to_string() });
        let change = match facts.get("action").and_then(Value::as_str) {
            Some("created") => match facts.get("due_ts").and_then(Value::as_i64) {
                Some(due) => format!("Created · due {}", super::super::memos::describe(due, e.ts)),
                None => "Created".to_string(),
            },
            Some("cancelled") => "Cancelled".to_string(),
            Some(other) => other.to_string(),
            None => "Changed".to_string(),
        };
        obj.insert("label".into(), json!(format!("Member reminder for @{}", name)));
        if !uid.is_empty() {
            obj.insert("member_id".into(), json!(uid));
        }
        obj.insert("change".into(), json!(change));
    } else {
        let name = facts.get("name").and_then(Value::as_str).unwrap_or("a picture");
        obj.insert("label".into(), json!(format!("Picture “{}”", name)));
        obj.insert("change".into(), json!(if new.is_null() { "Deleted" } else { "Uploaded" }));
    }
    obj
}
